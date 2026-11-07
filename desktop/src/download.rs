//! Загрузка моделей с Hugging Face с докачкой. Токен HF_TOKEN (если задан) уходит только на сам сайт,
//! но не на CDN, на который он перенаправляет.
use crate::catalog::Model;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub fn endpoint() -> String {
    std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".into()).trim_end_matches('/').to_string()
}

pub fn model_url(m: &Model) -> String {
    format!("{}/{}/resolve/main/{}", endpoint(), m.repo, m.file)
}

pub fn mmproj_path(m: &Model) -> PathBuf {
    model_path(m).with_file_name(&m.mmproj)
}

pub fn mmproj_url(m: &Model) -> String {
    format!("{}/{}/resolve/main/{}", endpoint(), m.repo, m.mmproj)
}

/// Модель готова к запуску: скачан файл весов и (для моделей с «зрением») файл проектора.
pub fn is_ready(m: &Model) -> bool {
    model_path(m).exists() && (m.mmproj.is_empty() || mmproj_path(m).exists())
}

/// Скачивает модель и, если нужно, проектор. progress(этап, скачано, всего, байт/с).
pub fn fetch_model(m: &Model, cancel: &AtomicBool, mut progress: impl FnMut(&str, u64, u64, f64)) -> Result<(), String> {
    fetch(&model_url(m), &model_path(m), (m.size_gb * 0.9 * 1e9) as u64, cancel, |d, t, s| progress("модель", d, t, s))?;
    if !m.mmproj.is_empty() {
        fetch(&mmproj_url(m), &mmproj_path(m), 100_000_000, cancel, |d, t, s| progress("картинки", d, t, s))?;
    }
    Ok(())
}

pub fn model_path(m: &Model) -> PathBuf {
    crate::engine::data_dir().join("models").join(&m.id).join(&m.file)
}

fn host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_string()
}

fn open(url: &str, have: u64) -> Result<ureq::Response, String> {
    open_range(url, if have > 0 { Some(format!("bytes={have}-")) } else { None }, &host(url)).map(|r| r.0)
}

/// Открывает адрес (перенаправления разбираются вручную) и возвращает ответ и конечный адрес.
/// Токен уходит только на `auth_host`, а не на CDN.
fn open_range(url: &str, range: Option<String>, auth_host: &str) -> Result<(ureq::Response, String), String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .user_agent("Nexora")
        .build();
    let mut cur = url.to_string();
    for _ in 0..6 {
        let mut req = agent.get(&cur);
        if let Some(r) = &range {
            req = req.set("Range", r);
        }
        if let Ok(t) = std::env::var("HF_TOKEN") {
            if host(&cur) == auth_host {
                req = req.set("Authorization", &format!("Bearer {t}"));
            }
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(c, r)) if (300..400).contains(&c) => r,
            Err(ureq::Error::Status(c, _)) => return Err(format!("HTTP {c}")),
            Err(e) => return Err(e.to_string()),
        };
        if (300..400).contains(&resp.status()) {
            let loc = resp.header("Location").ok_or("перенаправление без адреса")?.to_string();
            cur = if loc.starts_with("http") {
                loc
            } else {
                format!("{}://{}{}", cur.split("://").next().unwrap_or("https"), host(&cur), loc)
            };
            continue;
        }
        return Ok((resp, cur));
    }
    Err("слишком много перенаправлений".into())
}

/// Скачивает в `dest` (с продолжением .part). progress(скачано, всего, байт/с). Отмена через `cancel`.
pub fn fetch(url: &str, dest: &Path, min_bytes: u64, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64, f64)) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dest.parent().ok_or("нет папки")?).map_err(|e| e.to_string())?;
    let part = PathBuf::from(format!("{}.part", dest.display()));
    // большие файлы качаем несколькими потоками по кускам; если сервер не умеет Range, идём обычным путём
    if let Some(r) = fetch_parallel(url, dest, min_bytes, cancel, PAR_CHUNK, PAR_WORKERS, PAR_MIN, &mut progress) {
        return r;
    }
    let mut last_err = String::new();
    for attempt in 0..5 {
        if cancel.load(Ordering::Relaxed) {
            return Err("Отменено".into());
        }
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let resp = match open(url, have) {
            Ok(r) => r,
            Err(e) => {
                if e.contains("401") || e.contains("403") || e.contains("404") {
                    return Err(format!("Загрузка не удалась ({e}). Проверьте имя файла и доступ к репозиторию."));
                }
                last_err = e;
                std::thread::sleep(Duration::from_secs(2 * (attempt + 1)));
                continue;
            }
        };
        let resumed = have > 0 && resp.status() == 206;
        let start = if resumed { have } else { 0 };
        let total = start + resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let mut f = std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).map_err(|e| e.to_string())?;
        let mut r = resp.into_reader();
        let mut buf = vec![0u8; 1 << 20];
        let (mut done, t0, mut last) = (start, Instant::now(), Instant::now());
        let mut broken = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Отменено".into());
            }
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                    done += n as u64;
                    if last.elapsed() > Duration::from_millis(500) {
                        last = Instant::now();
                        progress(done, total, (done - start) as f64 / t0.elapsed().as_secs_f64().max(1e-6));
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                    broken = true;
                    break;
                }
            }
        }
        if broken || (total > 0 && done < total) {
            std::thread::sleep(Duration::from_secs(2 * (attempt + 1)));
            continue;
        }
        drop(f);
        finish(&part, dest, min_bytes, &mut progress)?;
        return Ok(());
    }
    Err(format!("Не удалось скачать: {last_err}. Нажмите «Скачать» ещё раз, загрузка продолжится."))
}

/// Проверка и перенос готового `.part` на место: заголовок GGUF и минимальный размер.
fn finish(part: &Path, dest: &Path, min_bytes: u64, progress: &mut impl FnMut(u64, u64, f64)) -> Result<(), String> {
    let bad = |m: String| {
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(map_path(part));
        Err(m)
    };
    if dest.extension().map(|e| e.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
        let mut magic = [0u8; 4];
        let ok = std::fs::File::open(part).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic)).is_ok() && &magic == b"GGUF";
        if !ok {
            return bad("Файл не является моделью GGUF. Загрузка отклонена.".into());
        }
    }
    let size = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if size < min_bytes {
        return bad(format!("Файл слишком мал ({size} байт): это не модель. Загрузка отклонена."));
    }
    std::fs::rename(part, dest).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(map_path(part));
    progress(size, size, 0.0);
    Ok(())
}

/// Размер куска, число потоков и порог размера, с которого включается параллельная загрузка.
const PAR_CHUNK: u64 = 32 << 20;
const PAR_WORKERS: usize = 6;
const PAR_MIN: u64 = 128 << 20;

fn map_path(part: &Path) -> PathBuf {
    PathBuf::from(format!("{}.map", part.display()))
}

/// Размер и поддержка Range: запрос первого байта. Возвращает (конечный адрес, размер).
fn probe(url: &str) -> Option<(String, u64)> {
    let (r, fin) = open_range(url, Some("bytes=0-0".into()), &host(url)).ok()?;
    if r.status() != 206 {
        return None;
    }
    let total = r.header("Content-Range")?.rsplit('/').next()?.trim().parse::<u64>().ok()?;
    Some((fin, total))
}

/// Один кусок [a, b] в нужное место файла.
fn download_chunk(url: &str, auth_host: &str, a: u64, b: u64, f: &mut std::fs::File, cancel: &AtomicBool, got: &std::sync::atomic::AtomicU64) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    let (resp, _) = open_range(url, Some(format!("bytes={a}-{b}")), auth_host)?;
    if resp.status() != 206 {
        return Err(format!("HTTP {}", resp.status()));
    }
    f.seek(SeekFrom::Start(a)).map_err(|e| e.to_string())?;
    let mut r = resp.into_reader().take(b - a + 1);
    let mut buf = vec![0u8; 1 << 20];
    let mut n_total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            got.fetch_sub(n_total, Ordering::Relaxed);
            return Err("Отменено".into());
        }
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                n_total += n as u64;
                got.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(e) => {
                got.fetch_sub(n_total, Ordering::Relaxed); // кусок будет скачан заново
                return Err(e.to_string());
            }
        }
    }
    if n_total != b - a + 1 {
        got.fetch_sub(n_total, Ordering::Relaxed);
        return Err("кусок пришёл не полностью".into());
    }
    Ok(())
}

/// Параллельная загрузка кусками. None = не подходит (малый файл, нет Range): качать обычным способом.
/// Готовые куски записываются в `.part.map`, поэтому после обрыва докачиваются только недостающие.
#[allow(clippy::too_many_arguments)]
fn fetch_parallel(url: &str, dest: &Path, min_bytes: u64, cancel: &AtomicBool, chunk: u64, workers: usize, par_min: u64, progress: &mut impl FnMut(u64, u64, f64)) -> Option<Result<(), String>> {
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    use std::sync::Mutex;
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mapf = map_path(&part);
    let (fin, total) = probe(url)?;
    if total < par_min || chunk == 0 {
        return None;
    }
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let n = total.div_ceil(chunk) as usize;
    let mut ready = vec![false; n];
    match std::fs::read_to_string(&mapf) {
        // карта верна, только если файл уже растянут до полного размера
        Ok(t) if have == total => {
            for i in t.lines().filter_map(|l| l.trim().parse::<usize>().ok()) {
                if i < n {
                    ready[i] = true;
                }
            }
        }
        Ok(_) => {}
        // остаток обычной последовательной загрузки: целые куски в начале файла готовы
        Err(_) if have > 0 && have < total => {
            for (i, d) in ready.iter_mut().enumerate() {
                *d = (i as u64 + 1) * chunk <= have;
            }
        }
        Err(_) => {}
    }
    let prep = (|| -> std::io::Result<std::fs::File> {
        let f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part)?;
        if f.metadata()?.len() != total {
            f.set_len(total)?;
        }
        let mut m = std::fs::File::create(&mapf)?;
        for i in (0..n).filter(|i| ready[*i]) {
            writeln!(m, "{i}")?;
        }
        std::fs::OpenOptions::new().append(true).open(&mapf)
    })();
    let map_file = match prep {
        Ok(m) => Mutex::new(m),
        Err(e) => return Some(Err(e.to_string())),
    };
    let initial: u64 = (0..n).filter(|i| ready[*i]).map(|i| (total - i as u64 * chunk).min(chunk)).sum();
    let todo: Vec<usize> = (0..n).filter(|i| !ready[*i]).collect();
    let next = AtomicUsize::new(0);
    let got = AtomicU64::new(initial);
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let cur_url = Mutex::new(fin);
    let stop = AtomicBool::new(false);
    let alive = AtomicUsize::new(0);
    let auth = host(url);
    let t0 = Instant::now();
    let nw = workers.max(1).min(todo.len().max(1));
    alive.store(nw, Ordering::SeqCst);
    std::thread::scope(|sc| {
        for _ in 0..nw {
            sc.spawn(|| {
                let worker = || {
                    let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&part) else {
                        *failed.lock().unwrap() = Some("не удалось открыть файл загрузки".into());
                        stop.store(true, Ordering::Relaxed);
                        return;
                    };
                    loop {
                        if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let Some(&i) = todo.get(next.fetch_add(1, Ordering::SeqCst)) else { return };
                        let (a, b) = (i as u64 * chunk, ((i as u64 + 1) * chunk).min(total) - 1);
                        let mut err = String::new();
                        let mut ok = false;
                        for attempt in 0..5u64 {
                            if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let u = cur_url.lock().unwrap().clone();
                            match download_chunk(&u, &auth, a, b, &mut f, cancel, &got) {
                                Ok(()) => {
                                    ok = true;
                                    break;
                                }
                                Err(e) => {
                                    err = e;
                                    // подписанный адрес CDN мог устареть: получаем заново
                                    if let Some((nu, _)) = probe(url) {
                                        *cur_url.lock().unwrap() = nu;
                                    }
                                    if (err.contains("401") || err.contains("404")) && attempt >= 1 {
                                        break;
                                    }
                                    std::thread::sleep(Duration::from_secs(2 * (attempt + 1)));
                                }
                            }
                        }
                        if !ok {
                            let mut g = failed.lock().unwrap();
                            if g.is_none() {
                                *g = Some(err);
                            }
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                        let _ = writeln!(map_file.lock().unwrap(), "{i}");
                    }
                };
                worker();
                alive.fetch_sub(1, Ordering::SeqCst);
            });
        }
        // отчёт о ходе загрузки идёт из вызывающего потока
        let mut last = Instant::now();
        while alive.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(50));
            if last.elapsed() > Duration::from_millis(500) {
                last = Instant::now();
                let d = got.load(Ordering::Relaxed);
                progress(d, total, d.saturating_sub(initial) as f64 / t0.elapsed().as_secs_f64().max(1e-6));
            }
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return Some(Err("Отменено".into()));
    }
    if let Some(e) = failed.lock().unwrap().take() {
        if e.contains("401") || e.contains("403") || e.contains("404") {
            return Some(Err(format!("Загрузка не удалась ({e}). Проверьте имя файла и доступ к репозиторию.")));
        }
        return Some(Err(format!("Не удалось скачать: {e}. Нажмите «Скачать» ещё раз, докачаются только недостающие куски.")));
    }
    Some(finish(&part, dest, min_bytes, progress))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    /// Локальный сервер с поддержкой Range. Первые `fail_first` запросов кусков обрываются посреди ответа.
    fn serve(data: Arc<Vec<u8>>, range: bool, fail_first: usize) -> (String, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/m.gguf", l.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h2 = hits.clone();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { continue };
                let (data, h) = (data.clone(), h2.clone());
                std::thread::spawn(move || {
                    let mut rd = BufReader::new(c.try_clone().unwrap());
                    let mut rng: Option<(u64, u64)> = None;
                    loop {
                        let mut line = String::new();
                        if rd.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                            let mut p = v.trim().split('-');
                            let a: u64 = p.next().unwrap().parse().unwrap();
                            let b: u64 = p.next().and_then(|x| x.parse().ok()).unwrap_or(data.len() as u64 - 1);
                            rng = Some((a, b.min(data.len() as u64 - 1)));
                        }
                    }
                    let n = h.fetch_add(1, Ordering::SeqCst);
                    match rng {
                        Some((a, b)) if range => {
                            let body = &data[a as usize..=b as usize];
                            let head = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{b}/{}\r\nConnection: close\r\n\r\n", body.len(), data.len());
                            let _ = c.write_all(head.as_bytes());
                            if b > a && n > 0 && n <= fail_first {
                                let _ = c.write_all(&body[..body.len() / 2]); // обрыв
                            } else {
                                let _ = c.write_all(body);
                            }
                        }
                        _ => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len());
                            let _ = c.write_all(head.as_bytes());
                            let _ = c.write_all(&data);
                        }
                    }
                });
            }
        });
        (url, hits)
    }

    fn sample(n: usize) -> Arc<Vec<u8>> {
        let mut v: Vec<u8> = (0..n).map(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8).collect();
        v[..4].copy_from_slice(b"GGUF");
        Arc::new(v)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexora-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("m.gguf")
    }

    #[test]
    fn parallel_download_is_identical() {
        let data = sample(5_000_000);
        let (url, hits) = serve(data.clone(), true, 0);
        let dest = tmp("ok");
        let cancel = AtomicBool::new(false);
        let r = fetch_parallel(&url, &dest, 1000, &cancel, 1 << 20, 4, 1 << 20, &mut |_, _, _| {});
        assert!(matches!(r, Some(Ok(()))), "{r:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
        assert!(hits.load(Ordering::SeqCst) >= 6, "ожидались запросы по кускам");
        assert!(!map_path(&PathBuf::from(format!("{}.part", dest.display()))).exists());
    }

    #[test]
    fn parallel_download_retries_broken_chunks() {
        let data = sample(4_000_000);
        let (url, _) = serve(data.clone(), true, 3);
        let dest = tmp("retry");
        let cancel = AtomicBool::new(false);
        let r = fetch_parallel(&url, &dest, 1000, &cancel, 1 << 20, 3, 1 << 20, &mut |_, _, _| {});
        assert!(matches!(r, Some(Ok(()))), "{r:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
    }

    #[test]
    fn no_range_support_falls_back() {
        let (url, _) = serve(sample(2_000_000), false, 0);
        let dest = tmp("norange");
        let cancel = AtomicBool::new(false);
        assert!(fetch_parallel(&url, &dest, 1000, &cancel, 1 << 20, 4, 1 << 20, &mut |_, _, _| {}).is_none());
    }

    #[test]
    fn small_file_stays_sequential() {
        let (url, _) = serve(sample(500_000), true, 0);
        let dest = tmp("small");
        let cancel = AtomicBool::new(false);
        assert!(fetch_parallel(&url, &dest, 1000, &cancel, 1 << 20, 4, 1 << 20, &mut |_, _, _| {}).is_none());
    }

    #[test]
    fn resumes_from_sequential_leftover() {
        let data = sample(5_000_000);
        let (url, hits) = serve(data.clone(), true, 0);
        let dest = tmp("resume");
        let part = PathBuf::from(format!("{}.part", dest.display()));
        // остаток обычной загрузки: первые 2,5 МБ (2 целых куска по 1 МиБ)
        std::fs::write(&part, &data[..2_500_000]).unwrap();
        let cancel = AtomicBool::new(false);
        let r = fetch_parallel(&url, &dest, 1000, &cancel, 1 << 20, 2, 1 << 20, &mut |_, _, _| {});
        assert!(matches!(r, Some(Ok(()))), "{r:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
        // 5 000 000 байт = 5 кусков по 1 МиБ; 2 готовы, значит 1 проба + 3 куска
        assert_eq!(hits.load(Ordering::SeqCst), 1 + 3);
    }

    #[test]
    fn rejects_non_gguf_and_cancel() {
        let mut v = (*sample(3_000_000)).clone();
        v[..4].copy_from_slice(b"XXXX");
        let (url, _) = serve(Arc::new(v), true, 0);
        let dest = tmp("bad");
        let cancel = AtomicBool::new(false);
        let r = fetch_parallel(&url, &dest, 1000, &cancel, 1 << 20, 2, 1 << 20, &mut |_, _, _| {});
        assert!(matches!(&r, Some(Err(e)) if e.contains("GGUF")), "{r:?}");
        assert!(!dest.exists());
        let dest2 = tmp("cancel");
        let c2 = AtomicBool::new(true);
        let r2 = fetch_parallel(&url, &dest2, 1000, &c2, 1 << 20, 2, 1 << 20, &mut |_, _, _| {});
        assert!(matches!(&r2, Some(Err(e)) if e.contains("Отменено")), "{r2:?}");
    }
}
