//! Плагины v1: скиллы (SKILL.md), агенты (роли) и правила проекта. Ставятся с GitHub по точному коммиту.
//! Только текстовые файлы: скрипты и программы не устанавливаются.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TEXT_EXT: &[&str] = &["md", "txt", "json", "yml", "yaml"];
const SCRIPT_EXT: &[&str] = &["py", "sh", "js", "ts", "ps1", "bat", "cmd", "exe", "dll", "rb", "pl"];
/// Скрипты, которые можно установить и запускать (текстовые). Остальное (exe, dll, ts) не ставится никогда.
const RUN_EXT: &[&str] = &["py", "sh", "js", "ps1", "bat", "cmd", "rb", "pl"];
const MAX_FILE: u64 = 200_000;
const MAX_FILES: usize = 3000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    pub description: String,
    pub path: String, // SKILL.md или файл агента, относительно папки плагина
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub owner: String,
    pub repo: String,
    pub sha: String,
    pub skills: Vec<Item>,
    pub agents: Vec<Item>,
    pub files: usize,
    pub enabled: bool,
    /// Установленные скрипты (пути от корня репозитория); пусто, если плагин ставился без скриптов.
    #[serde(default)]
    pub scripts: Vec<String>,
}

pub fn plugins_dir() -> PathBuf {
    let d = crate::engine::data_dir().join("plugins");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Разбор заголовка `---`...`---`: ключ: значение (в том числе `>` и `|` с отступами).
pub fn frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();
    let t = text.trim_start_matches('\u{feff}');
    let mut lines = t.lines();
    if lines.next().map(|l| l.trim()) != Some("---") {
        return (map, t.to_string());
    }
    let mut head: Vec<&str> = vec![];
    let mut body_start = None;
    let mut consumed = 1;
    for l in lines {
        consumed += 1;
        if l.trim() == "---" {
            body_start = Some(consumed);
            break;
        }
        head.push(l);
    }
    let Some(bs) = body_start else { return (map, t.to_string()) };
    let mut i = 0;
    while i < head.len() {
        let l = head[i];
        if let Some((k, v)) = l.split_once(':') {
            if !l.starts_with(' ') && !k.trim().is_empty() {
                let mut val = v.trim().to_string();
                if val == ">" || val == "|" || val == ">-" || val == "|-" {
                    val.clear();
                    while i + 1 < head.len() && (head[i + 1].starts_with(' ') || head[i + 1].trim().is_empty()) {
                        i += 1;
                        if !val.is_empty() {
                            val.push(' ');
                        }
                        val.push_str(head[i].trim());
                    }
                }
                map.insert(k.trim().to_lowercase(), val.trim_matches(|c| c == '"' || c == '\'').to_string());
            }
        }
        i += 1;
    }
    let body: String = t.lines().skip(bs).collect::<Vec<_>>().join("\n");
    (map, body)
}

/// "owner/repo", "github.com/owner/repo", ".../tree/ветка/папка" -> (owner, repo, папка)
pub fn parse_source(s: &str) -> Result<(String, String, String), String> {
    let t = s.trim().trim_end_matches('/').trim_end_matches(".git");
    let t = t.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let t = t.strip_prefix("github.com/").unwrap_or(t);
    let parts: Vec<&str> = t.split('/').collect();
    if parts.len() < 2 {
        return Err("Укажите репозиторий в виде github.com/автор/имя".into());
    }
    let ok = |x: &str| !x.is_empty() && !x.starts_with('.') && x.len() < 100 && x.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !ok(parts[0]) || !ok(parts[1]) {
        return Err("Некорректное имя репозитория".into());
    }
    let sub = if parts.len() > 4 && (parts[2] == "tree" || parts[2] == "blob") { parts[4..].join("/") } else { String::new() };
    if sub.contains("..") {
        return Err("Некорректный путь".into());
    }
    Ok((parts[0].to_string(), parts[1].to_string(), sub))
}

fn gh_get(url: &str, accept: &str) -> Result<String, String> {
    let host_ok = url.starts_with("https://api.github.com/") || url.starts_with("https://raw.githubusercontent.com/");
    if !host_ok {
        return Err("разрешены только api.github.com и raw.githubusercontent.com".into());
    }
    let mut r = ureq::get(url).set("User-Agent", "Nexora").set("Accept", accept).timeout(Duration::from_secs(15));
    if let Ok(t) = std::env::var("GITHUB_TOKEN") {
        r = r.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = r.call().map_err(|e| match e {
        ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => "GitHub временно ограничил запросы (лимит без входа). Подождите минуту.".to_string(),
        ureq::Error::Status(404, _) => "Репозиторий или файл не найден (или он закрыт)".to_string(),
        ureq::Error::Status(c, _) => format!("GitHub ответил {c}"),
        o => format!("нет связи с GitHub: {o}"),
    })?;
    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(resp.into_reader(), 3_000_000), &mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

/// Параллельно скачать файлы репозитория (до 8 потоков) с общим сроком. Порядок результатов сохраняется.
fn fetch_many(owner: &str, repo: &str, sha: &str, paths: &[String], deadline: Instant) -> Vec<Result<String, String>> {
    let n = paths.len();
    let results: Mutex<Vec<Option<Result<String, String>>>> = Mutex::new((0..n).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let aborted = AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..8.min(n) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= n {
                    break;
                }
                let r = if aborted.load(Ordering::SeqCst) || Instant::now() > deadline {
                    Err("время ожидания GitHub истекло: проверьте интернет или VPN и повторите".to_string())
                } else {
                    raw(owner, repo, sha, &paths[i])
                };
                // нет связи совсем: не ждём таймаутов остальных файлов
                if r.as_ref().err().map_or(false, |e| e.starts_with("нет связи")) {
                    aborted.store(true, Ordering::SeqCst);
                }
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|o| o.unwrap_or_else(|| Err("не скачано".to_string()))).collect()
}

fn raw(owner: &str, repo: &str, sha: &str, path: &str) -> Result<String, String> {
    let p: Vec<String> = path.split('/').map(|s| s.replace(' ', "%20")).collect();
    gh_get(&format!("https://raw.githubusercontent.com/{owner}/{repo}/{sha}/{}", p.join("/")), "text/plain")
}

/// Коммит HEAD через протокол git (не расходует лимит API GitHub); при неудаче запасной путь через API.
fn head_sha_git(owner: &str, repo: &str) -> Option<String> {
    let url = format!("https://github.com/{owner}/{repo}.git/info/refs?service=git-upload-pack");
    let resp = ureq::get(&url).set("User-Agent", "git/2.45.0").timeout(Duration::from_secs(10)).call().ok()?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(resp.into_reader(), 200_000), &mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let i = text.find(" HEAD")?;
    let sha = text.get(i.checked_sub(40)?..i)?;
    if sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha.to_string())
    } else {
        None
    }
}

pub fn head_sha(owner: &str, repo: &str) -> Result<String, String> {
    if let Some(s) = head_sha_git(owner, repo) {
        return Ok(s);
    }
    let v: Value = serde_json::from_str(&gh_get(&format!("https://api.github.com/repos/{owner}/{repo}/commits/HEAD"), "application/vnd.github+json")?).map_err(|e| e.to_string())?;
    let sha = v["sha"].as_str().ok_or("нет данных о коммите")?.to_string();
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("некорректный коммит".into());
    }
    Ok(sha)
}

pub struct Found {
    pub sha: String,
    pub files: Vec<(String, u64)>, // что будет установлено
    pub skills: Vec<Item>,
    pub agents: Vec<Item>,
    pub skipped_scripts: usize,
    pub truncated: bool,
    /// Скрипты репозитория, которые можно установить (текстовые, до 200 КБ).
    pub scripts: Vec<(String, u64)>,
}

/// Ищет в репозитории на коммите `sha` скиллы и агентов и список файлов к установке.
pub fn discover(owner: &str, repo: &str, sub: &str, sha: &str, with_scripts: bool) -> Result<Found, String> {
    let tree: Value = serde_json::from_str(&gh_get(&format!("https://api.github.com/repos/{owner}/{repo}/git/trees/{sha}?recursive=1"), "application/vnd.github+json")?).map_err(|e| e.to_string())?;
    let entries = tree["tree"].as_array().cloned().unwrap_or_default();
    let in_sub = |p: &str| sub.is_empty() || p == sub || p.starts_with(&format!("{sub}/"));
    let mut skill_dirs: Vec<String> = vec![];
    let mut agent_files: Vec<String> = vec![];
    let mut skipped = 0;
    let all: Vec<(String, u64)> = entries
        .iter()
        .filter(|e| e["type"] == "blob")
        .filter_map(|e| Some((e["path"].as_str()?.to_string(), e["size"].as_u64().unwrap_or(0))))
        .filter(|(p, _)| in_sub(p) && !p.contains("node_modules/") && !p.contains(".git/") && !p.contains(".."))
        .collect();
    for (p, _) in &all {
        let name = p.rsplit('/').next().unwrap_or("");
        if name == "SKILL.md" {
            skill_dirs.push(p.trim_end_matches("SKILL.md").trim_end_matches('/').to_string());
        } else if name.ends_with(".md") && p.rsplit('/').nth(1) == Some("agents") {
            agent_files.push(p.clone());
        }
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if SCRIPT_EXT.contains(&ext.as_str()) {
            skipped += 1;
        }
    }
    let mut files: Vec<(String, u64)> = vec![];
    for (p, sz) in &all {
        let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
        if !TEXT_EXT.contains(&ext.as_str()) || *sz > MAX_FILE {
            continue;
        }
        let in_skill = skill_dirs.iter().any(|d| if d.is_empty() { true } else { p.starts_with(&format!("{d}/")) });
        if in_skill || agent_files.contains(p) {
            files.push((p.clone(), *sz));
        }
    }
    let truncated = tree["truncated"].as_bool().unwrap_or(false) || files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    let mut scripts: Vec<(String, u64)> = vec![];
    let mut unsupported = 0usize;
    for (p, sz) in &all {
        let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
        if !SCRIPT_EXT.contains(&ext.as_str()) {
            continue;
        }
        // скрипты берём из всего репозитория (навыки ссылаются на hooks/ и подобные папки через ../)
        if RUN_EXT.contains(&ext.as_str()) && *sz <= MAX_FILE && scripts.len() < 1000 {
            scripts.push((p.clone(), *sz));
        } else {
            unsupported += 1;
        }
    }
    if with_scripts {
        files.extend(scripts.iter().cloned());
    }
    // имена и описания: все SKILL.md и файлы агентов скачиваем параллельно
    let mut targets: Vec<(String, String, bool)> = vec![]; // (путь, имя по умолчанию, это скилл)
    for d in skill_dirs.iter().take(400) {
        let default = d.rsplit('/').next().filter(|x| !x.is_empty()).unwrap_or(repo).to_string();
        targets.push((if d.is_empty() { "SKILL.md".to_string() } else { format!("{d}/SKILL.md") }, default, true));
    }
    for f in agent_files.iter().take(400) {
        targets.push((f.clone(), f.rsplit('/').next().unwrap_or("agent").trim_end_matches(".md").to_string(), false));
    }
    let paths: Vec<String> = targets.iter().map(|t| t.0.clone()).collect();
    let texts = fetch_many(owner, repo, sha, &paths, Instant::now() + Duration::from_secs(60 + paths.len() as u64 / 2));
    if let (Some(Err(e)), true) = (texts.first(), texts.iter().all(|t| t.is_err())) {
        return Err(e.clone()); // ни один файл не скачался: это не «пустой репозиторий», а проблема связи
    }
    let mut skills = vec![];
    let mut agents = vec![];
    for ((path, default, is_skill), res) in targets.into_iter().zip(texts) {
        let (name, desc) = match res {
            Ok(t) => {
                let (m, body) = frontmatter(&t);
                let d = m.get("description").cloned().unwrap_or_else(|| body.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#')).unwrap_or("").trim().to_string());
                (m.get("name").cloned().unwrap_or(default), d)
            }
            Err(_) => (default, String::new()),
        };
        let item = Item { name, description: desc.chars().take(300).collect(), path };
        if is_skill { skills.push(item) } else { agents.push(item) }
    }
    let _ = skipped;
    Ok(Found { sha: sha.to_string(), files, skills, agents, skipped_scripts: unsupported, truncated, scripts })
}

pub fn preview(source: &str) -> Result<Value, String> {
    let (o, r, sub) = parse_source(source)?;
    let sha = head_sha(&o, &r)?;
    let f = discover(&o, &r, &sub, &sha, false)?;
    if f.skills.is_empty() && f.agents.is_empty() {
        return Err("В репозитории не найдено скиллов (SKILL.md) и агентов (папка agents/ с файлами .md)".into());
    }
    Ok(json!({"owner": o, "repo": r, "sub": sub, "sha": f.sha, "files": f.files.len(), "bytes": f.files.iter().map(|x| x.1).sum::<u64>(),
        "skills": f.skills, "agents": f.agents, "skipped_scripts": f.skipped_scripts, "truncated": f.truncated,
        "scripts": f.scripts.iter().map(|x| x.0.clone()).collect::<Vec<_>>(), "script_bytes": f.scripts.iter().map(|x| x.1).sum::<u64>()}))
}

fn manifest_path(id: &str) -> PathBuf {
    plugins_dir().join(id).join("plugin.json")
}

pub fn list() -> Vec<Plugin> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(plugins_dir()) {
        for e in rd.flatten() {
            if let Ok(s) = std::fs::read_to_string(e.path().join("plugin.json")) {
                if let Ok(p) = serde_json::from_str::<Plugin>(&s) {
                    out.push(p);
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Установка порциями по 30 файлов: большие репозитории ставятся целиком, `progress(скачано, всего)` сообщает ход.
pub fn install(source: &str, sha: &str, with_scripts: bool, progress: impl Fn(usize, usize)) -> Result<Plugin, String> {
    let (o, r, sub) = parse_source(source)?;
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Некорректный коммит".into());
    }
    let f = discover(&o, &r, &sub, sha, with_scripts)?;
    let id = format!("{o}__{r}");
    let dir = plugins_dir().join(&id);
    let target = dir.join(&sha[..10]);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    let all_total = f.files.len();
    let mut done = 0usize;
    progress(0, all_total);
    for chunk in f.files.chunks(30) {
    let paths: Vec<String> = chunk.iter().map(|x| x.0.clone()).collect();
    let texts = fetch_many(&o, &r, sha, &paths, Instant::now() + Duration::from_secs(90));
    for ((p, _), text) in chunk.iter().zip(texts) {
        let text = match text {
            Ok(t) => t,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir); // не оставляем полуустановленный плагин
                return Err(e);
            }
        };
        total += text.len() as u64;
        if total > 50_000_000 {
            return Err("Слишком большой объём файлов плагина".into());
        }
        let dest = target.join(p);
        if !dest.starts_with(&target) {
            return Err("Некорректный путь файла".into());
        }
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, text).map_err(|e| e.to_string())?;
    }
    done += chunk.len();
    progress(done, all_total);
    }
    let p = Plugin { id: id.clone(), owner: o, repo: r, sha: sha.to_string(), skills: f.skills, agents: f.agents, files: f.files.len(), enabled: true, scripts: if with_scripts { f.scripts.iter().map(|x| x.0.clone()).collect() } else { vec![] } };
    std::fs::write(manifest_path(&id), serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(p)
}

pub fn set_enabled(id: &str, on: bool) -> Result<(), String> {
    let mut p = list().into_iter().find(|p| p.id == id).ok_or("Плагин не найден")?;
    p.enabled = on;
    std::fs::write(manifest_path(id), serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn remove(id: &str) -> Result<(), String> {
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("Некорректный идентификатор".into());
    }
    std::fs::remove_dir_all(plugins_dir().join(id)).map_err(|e| e.to_string())
}

fn root_of(p: &Plugin) -> PathBuf {
    plugins_dir().join(&p.id).join(&p.sha[..10])
}

/// Включённые скиллы: (имя, описание).
pub fn skill_index() -> Vec<(String, String)> {
    list().into_iter().filter(|p| p.enabled).flat_map(|p| p.skills.into_iter().map(|s| (s.name, s.description))).collect()
}

fn find_skill(name: &str) -> Option<(Plugin, Item)> {
    list().into_iter().filter(|p| p.enabled).find_map(|p| p.skills.iter().find(|s| s.name == name).cloned().map(|s| (p.clone(), s)))
}

/// Текст скилла и список файлов рядом с ним (их можно прочитать через read_skill_file).
pub fn read_skill(name: &str) -> Result<String, String> {
    let (p, s) = find_skill(name).ok_or_else(|| format!("Скилл «{name}» не найден или отключён"))?;
    let path = root_of(&p).join(&s.path);
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (_, body) = frontmatter(&text);
    let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let mut others = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n != "SKILL.md" && e.path().is_file() {
                others.push(n);
            }
        }
    }
    let mut out: String = body.chars().take(12_000).collect();
    let skill_dir = s.path.rsplit_once('/').map(|x| format!("{}/", x.0)).unwrap_or_default();
    let mine: Vec<String> = p.scripts.iter().filter(|x| x.starts_with(&skill_dir)).map(|x| x[skill_dir.len()..].to_string()).collect();
    if !mine.is_empty() {
        out.push_str(&format!("\n\n[Установленные скрипты навыка: {}. Запуск: run_skill_script с именем навыка и этим путём; пользователь подтверждает первый запуск каждого скрипта.]", mine.join(", ")));
    }
    if !others.is_empty() {
        out.push_str(&format!("\n\n[Файлы рядом со скиллом: {}. Прочитать: read_skill_file]", others.join(", ")));
    }
    Ok(out)
}

pub fn read_skill_file(name: &str, rel: &str) -> Result<String, String> {
    let (p, s) = find_skill(name).ok_or_else(|| format!("Скилл «{name}» не найден или отключён"))?;
    let base = root_of(&p).join(&s.path);
    let dir = base.parent().unwrap_or(Path::new("")).to_path_buf();
    let target = crate::tools::resolve(&dir, rel)?;
    let t = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
    Ok(t.chars().take(12_000).collect())
}

/// Путь скрипта внутри плагина (относительно корня репозитория), если он установлен для этого навыка.
/// `rel` задаётся от папки навыка; выйти за корень плагина нельзя, запускать можно только то, что в манифесте.
fn script_rel(p: &Plugin, item: &Item, rel: &str) -> Result<String, String> {
    let mut parts: Vec<String> = item.path.rsplit_once('/').map(|x| x.0.split('/').map(String::from).collect()).unwrap_or_default();
    for seg in rel.replace('\\', "/").split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("путь выходит за пределы плагина".into());
                }
            }
            s => parts.push(s.to_string()),
        }
    }
    let r = parts.join("/");
    if p.scripts.iter().any(|s| *s == r) {
        Ok(r)
    } else {
        Err(format!("у навыка нет установленного скрипта «{rel}»; установленные: {}", if p.scripts.is_empty() { "нет (плагин ставился без скриптов)".to_string() } else { p.scripts.join(", ") }))
    }
}

/// Полный путь установленного скрипта навыка и его расширение.
pub fn script_for(skill: &str, rel: &str) -> Result<(PathBuf, String), String> {
    let (p, item) = find_skill(skill).ok_or_else(|| format!("Скилл «{skill}» не найден или отключён"))?;
    let r = script_rel(&p, &item, rel)?;
    let full = root_of(&p).join(&r);
    if !full.is_file() {
        return Err("файл скрипта отсутствует на диске: переустановите плагин".into());
    }
    let ext = r.rsplit('.').next().unwrap_or("").to_lowercase();
    Ok((full, ext))
}

/// Команда PowerShell для запуска скрипта нужным интерпретатором. Все значения в одинарных кавычках.
pub fn script_command(full: &Path, ext: &str, args: &[String]) -> Result<String, String> {
    if args.len() > 20 || args.iter().any(|a| a.chars().count() > 2000) {
        return Err("слишком много или слишком длинные аргументы".into());
    }
    let q = |s: &str| format!("'{}'", crate::tools::ps_quote(s));
    let sp = q(&full.display().to_string());
    let a = args.iter().map(|x| q(x)).collect::<Vec<_>>().join(" ");
    Ok(match ext {
        "py" => format!("if (Get-Command python -ErrorAction SilentlyContinue) {{ & python {sp} {a} }} else {{ & py -3 {sp} {a} }}"),
        "js" => format!("& node {sp} {a}"),
        "ps1" => format!("& powershell -NoProfile -ExecutionPolicy Bypass -File {sp} {a}"),
        "bat" | "cmd" => format!("& cmd /c {sp} {a}"),
        "sh" => format!("& bash {sp} {a}"),
        "rb" => format!("& ruby {sp} {a}"),
        "pl" => format!("& perl {sp} {a}"),
        _ => return Err(format!("тип скрипта .{ext} не поддерживается")),
    })
}

pub fn agents() -> Vec<(String, String, String)> {
    list().into_iter().filter(|p| p.enabled).flat_map(|p| p.agents.into_iter().map(move |a| (a.name, a.description, p.id.clone()))).collect()
}

pub fn agent_body(name: &str) -> Option<String> {
    let p = list().into_iter().filter(|p| p.enabled).find(|p| p.agents.iter().any(|a| a.name == name))?;
    let a = p.agents.iter().find(|a| a.name == name)?;
    let text = std::fs::read_to_string(root_of(&p).join(&a.path)).ok()?;
    let (_, body) = frontmatter(&text);
    Some(body.chars().take(4000).collect())
}

/// Поиск репозиториев на GitHub. topic: "agent-skills", "claude-code" или пусто.
pub fn search(q: &str, topic: &str) -> Result<Vec<Value>, String> {
    let q = q.trim();
    let mut query = q.to_string();
    if ["agent-skills", "claude-code", "claude-skills", "claude-agents"].contains(&topic) {
        query.push_str(&format!(" topic:{topic}"));
    } else if query.is_empty() {
        return Err("Введите, что искать".into());
    }
    let enc: String = query.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let v: Value = serde_json::from_str(&gh_get(&format!("https://api.github.com/search/repositories?q={enc}&sort=stars&per_page=15"), "application/vnd.github+json")?).map_err(|e| e.to_string())?;
    Ok(v["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| json!({"full_name": r["full_name"], "stars": r["stargazers_count"], "description": r["description"], "updated": r["pushed_at"], "license": r["license"]["spdx_id"]}))
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_parsing() {
        let t = "---\nname: pdf-tools\ndescription: Работа с PDF.\nother: x\n---\n# Заголовок\nТекст";
        let (m, body) = frontmatter(t);
        assert_eq!(m["name"], "pdf-tools");
        assert_eq!(m["description"], "Работа с PDF.");
        assert!(body.starts_with("# Заголовок"));
        let (m2, b2) = frontmatter("без заголовка");
        assert!(m2.is_empty() && b2 == "без заголовка");
    }

    #[test]
    fn frontmatter_folded() {
        let t = "---\nname: a\ndescription: >\n  первая строка\n  вторая строка\n---\nтело";
        let (m, _) = frontmatter(t);
        assert_eq!(m["description"], "первая строка вторая строка");
    }

    #[test]
    fn sources() {
        assert_eq!(parse_source("github.com/anthropics/skills").unwrap(), ("anthropics".into(), "skills".into(), "".into()));
        assert_eq!(parse_source("https://github.com/a/b/tree/main/skills/x").unwrap(), ("a".into(), "b".into(), "skills/x".into()));
        assert_eq!(parse_source("a/b").unwrap().1, "b");
        assert!(parse_source("onlyone").is_err());
        assert!(parse_source("https://github.com/a/b/tree/main/../x").is_err());
        assert!(parse_source("https://evil.com/a/b").is_err() || parse_source("https://evil.com/a/b").unwrap().0 != "a");
    }
}

#[cfg(test)]
mod script_tests {
    use super::*;

    fn plugin() -> (Plugin, Item) {
        let item = Item { name: "pdf".into(), description: String::new(), path: "skills/pdf/SKILL.md".into() };
        let p = Plugin { id: "a__b".into(), owner: "a".into(), repo: "b".into(), sha: "0123456789abcdef0123456789abcdef01234567".into(), skills: vec![item.clone()], agents: vec![], files: 2, enabled: true, scripts: vec!["skills/pdf/scripts/fill.py".into(), "skills/other/run.sh".into()] };
        (p, item)
    }

    #[test]
    fn only_installed_scripts_resolve() {
        let (p, item) = plugin();
        assert_eq!(script_rel(&p, &item, "scripts/fill.py").unwrap(), "skills/pdf/scripts/fill.py");
        assert_eq!(script_rel(&p, &item, ".\\scripts\\fill.py").unwrap(), "skills/pdf/scripts/fill.py");
        assert_eq!(script_rel(&p, &item, "../other/run.sh").unwrap(), "skills/other/run.sh", "соседний навык того же плагина, но он в манифесте");
        assert!(script_rel(&p, &item, "scripts/evil.py").is_err(), "нет в манифесте");
        assert!(script_rel(&p, &item, "../../../../outside.py").is_err(), "за пределы плагина нельзя");
        assert!(script_rel(&p, &item, "../../../etc/passwd").is_err());
    }

    #[test]
    fn no_scripts_means_no_run() {
        let (mut p, item) = plugin();
        p.scripts.clear();
        let e = script_rel(&p, &item, "scripts/fill.py").unwrap_err();
        assert!(e.contains("без скриптов"), "{e}");
    }

    #[test]
    fn commands_are_quoted() {
        let f = Path::new("C:\\Users\\A B\\it's.py");
        let c = script_command(f, "py", &["a b".into(), "x'; Remove-Item *".into()]).unwrap();
        assert!(c.contains("'C:\\Users\\A B\\it''s.py'") && c.contains("'a b'") && c.contains("'x''; Remove-Item *'"), "{c}");
        assert!(script_command(f, "exe", &[]).is_err());
        assert!(script_command(f, "js", &vec!["a".to_string(); 21]).is_err());
        assert!(script_command(Path::new("x.ps1"), "ps1", &[]).unwrap().contains("-File"));
    }
}
