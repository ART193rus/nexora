//! Задачи для мобильного клиента: очередь, журнал событий, подтверждения, отмена.
//!
//! Агент сам живёт в программе (agent::run). Здесь только учёт: задача получает номер, выполняется по одной,
//! все события агента (токены ответа, вызовы инструментов, запросы подтверждения) складываются в журнал, который телефон
//! читает через длинный опрос. Прогресс в процентах не выдумывается: у агента нет измеримого «всего шагов», поэтому
//! телефону отдаётся только счётчик выполненных шагов.
use crate::agent::Approvals;
use crate::totp;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const MAX_EVENTS: usize = 4000;
const MAX_JOBS: usize = 50;
const MAX_ACTIVE: usize = 10;
pub const MAX_PROMPT: usize = 64 * 1024;
const APPROVAL_TTL_SECS: u64 = 15 * 60;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Queued,
    Running,
    Waiting, // ждёт подтверждения
    Done,
    Error,
    Cancelled,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Waiting => "waiting",
            Status::Done => "done",
            Status::Error => "error",
            Status::Cancelled => "cancelled",
        }
    }
    fn finished(&self) -> bool {
        matches!(self, Status::Done | Status::Error | Status::Cancelled)
    }
}

struct Event {
    seq: u64,
    ts: u64,
    kind: &'static str,
    data: Value,
}

struct Job {
    id: u64,
    title: String,
    prompt: String,
    device: String,
    status: Status,
    created: u64,
    started: u64,
    finished: u64,
    result: String,
    error: String,
    cancel: Arc<AtomicBool>,
    events: VecDeque<Event>,
    next_seq: u64,
    pending: Option<(Value, u64)>, // запрос подтверждения и время его появления
    steps: u32,
    history: Vec<(String, String)>, // предыдущие реплики диалога: (user|assistant, текст)
    fast: bool,                     // быстрый чат: короткая подсказка, без инструментов
    serial: String,                 // серийный номер сертификата устройства (имя не уникально и задаётся самим устройством)
}

impl Job {
    fn push(&mut self, kind: &'static str, data: Value) {
        self.next_seq += 1;
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(Event { seq: self.next_seq, ts: totp::now_secs(), kind, data });
    }

    fn summary(&self) -> Value {
        json!({
            "id": self.id, "title": self.title, "status": self.status.as_str(), "device": self.device,
            "created": self.created, "started": self.started, "finished": self.finished,
            "steps": self.steps, "progress": Value::Null, // процентов нет: измеримого показателя у агента нет
            "has_result": !self.result.is_empty(), "error": self.error,
            "pending": self.pending.as_ref().map(|p| p.0.clone()),
        })
    }
}

/// Как выполнять задачу: возвращает итоговый ответ. Устанавливает программа (там живёт доступ к движку).
pub type Exec = Arc<dyn Fn(&Arc<Hub>, u64, &str, Arc<AtomicBool>) -> Result<String, String> + Send + Sync>;

pub struct Hub {
    jobs: Mutex<Vec<Job>>,
    cv: Condvar,
    next_id: AtomicU64,
    pub approvals: Arc<Approvals>,
    exec: Mutex<Option<Exec>>,
    stop: AtomicBool,
    trusted: Mutex<std::collections::HashSet<String>>,
}

impl Hub {
    pub fn new(approvals: Arc<Approvals>) -> Arc<Hub> {
        Arc::new(Hub { jobs: Mutex::new(vec![]), cv: Condvar::new(), next_id: AtomicU64::new(1), approvals, exec: Mutex::new(None), stop: AtomicBool::new(false), trusted: Mutex::new(Default::default()) })
    }

    /// Запустить рабочий поток (одна задача за раз) и уборщика просроченных подтверждений.
    pub fn start(self: &Arc<Self>, exec: Exec) {
        *self.exec.lock().unwrap() = Some(exec);
        let h = self.clone();
        std::thread::spawn(move || h.worker());
        let h = self.clone();
        std::thread::spawn(move || {
            while !h.stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(10));
                h.expire_approvals();
            }
        });
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for j in self.jobs.lock().unwrap().iter() {
            j.cancel.store(true, Ordering::SeqCst);
        }
        self.cv.notify_all();
    }

    pub fn running(&self) -> bool {
        self.exec.lock().unwrap().is_some()
    }

    fn worker(self: Arc<Self>) {
        loop {
            let next = {
                let mut g = self.jobs.lock().unwrap();
                loop {
                    if self.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Some(j) = g.iter_mut().filter(|j| j.status == Status::Queued).min_by_key(|j| j.id) {
                        j.status = Status::Running;
                        j.started = totp::now_secs();
                        j.push("status", json!({"text": "Задача запущена"}));
                        break (j.id, j.prompt.clone(), j.cancel.clone());
                    }
                    g = self.cv.wait(g).unwrap();
                }
            };
            let exec = self.exec.lock().unwrap().clone();
            let r = match exec {
                Some(e) => e(&self, next.0, &next.1, next.2.clone()),
                None => Err("выполнение недоступно".into()),
            };
            let mut g = self.jobs.lock().unwrap();
            if let Some(j) = g.iter_mut().find(|j| j.id == next.0) {
                j.finished = totp::now_secs();
                j.pending = None;
                if j.cancel.load(Ordering::SeqCst) {
                    j.status = Status::Cancelled;
                    j.push("done", json!({"status": "cancelled"}));
                } else {
                    match r {
                        Ok(text) => {
                            j.result = text;
                            j.status = Status::Done;
                            j.push("done", json!({"status": "done"}));
                        }
                        Err(e) => {
                            j.error = e.chars().take(500).collect();
                            j.status = Status::Error;
                            let err = j.error.clone();
                            j.push("done", json!({"status": "error", "error": err}));
                        }
                    }
                }
            }
        }
    }

    pub fn create(&self, prompt: &str, title: &str, device: &str) -> Result<u64, String> {
        self.create_with(prompt, title, device, "", vec![], false)
    }

    /// Задача с историей диалога (для чата). Берутся последние 20 реплик, общий объём ограничен.
    pub fn create_with(&self, prompt: &str, title: &str, device: &str, serial: &str, history: Vec<(String, String)>, fast: bool) -> Result<u64, String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err("пустая задача".into());
        }
        if prompt.len() > MAX_PROMPT {
            return Err("задача слишком длинная".into());
        }
        let mut g = self.jobs.lock().unwrap();
        if g.iter().filter(|j| !j.status.finished()).count() >= MAX_ACTIVE {
            return Err("слишком много активных задач".into());
        }
        // хранится не больше MAX_JOBS: сначала удаляем самые старые завершённые
        while g.len() >= MAX_JOBS {
            match g.iter().position(|j| j.status.finished()) {
                Some(i) => {
                    g.remove(i);
                }
                None => return Err("слишком много задач".into()),
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let t: String = if title.trim().is_empty() { prompt.chars().take(60).collect() } else { title.trim().chars().take(80).collect() };
        let mut j = Job {
            id, title: t, prompt: prompt.to_string(), device: device.chars().take(64).collect(), status: Status::Queued, created: totp::now_secs(),
            started: 0, finished: 0, result: String::new(), error: String::new(), cancel: Arc::new(AtomicBool::new(false)),
            events: VecDeque::new(), next_seq: 0, pending: None, steps: 0, fast, serial: serial.to_string(),
            history: {
                let mut h: Vec<(String, String)> = history.into_iter().filter(|(r, t)| (r == "user" || r == "assistant") && !t.trim().is_empty()).collect();
                let skip = h.len().saturating_sub(20);
                h.drain(..skip);
                let mut total = 0usize;
                h.retain(|(_, t)| { total += t.len(); total <= MAX_PROMPT });
                h
            },
        };
        j.push("status", json!({"text": "Задача в очереди"}));
        g.push(j);
        self.cv.notify_all();
        Ok(id)
    }

    pub fn list(&self) -> Vec<Value> {
        let mut v: Vec<Value> = self.jobs.lock().unwrap().iter().map(|j| j.summary()).collect();
        v.reverse();
        v
    }

    pub fn active_count(&self) -> usize {
        self.jobs.lock().unwrap().iter().filter(|j| !j.status.finished()).count()
    }

    /// Состояние задачи и события после `after`. Если новых событий нет и задача идёт, ждёт до `wait` (длинный опрос).
    pub fn poll(&self, id: u64, after: u64, wait: Duration) -> Option<Value> {
        let deadline = Instant::now() + wait;
        loop {
            {
                let g = self.jobs.lock().unwrap();
                let j = g.iter().find(|j| j.id == id)?;
                let fresh = j.events.iter().any(|e| e.seq > after);
                if fresh || j.status.finished() || Instant::now() >= deadline {
                    let ev: Vec<Value> = j.events.iter().filter(|e| e.seq > after).take(500).map(|e| json!({"seq": e.seq, "ts": e.ts, "kind": e.kind, "data": e.data})).collect();
                    let last = ev.last().map_or(after, |e| e["seq"].as_u64().unwrap_or(after));
                    let mut s = j.summary();
                    s["events"] = Value::Array(ev);
                    s["last_seq"] = json!(last);
                    s["result"] = if j.status.finished() { json!(j.result) } else { Value::Null };
                    return Some(s);
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    pub fn cancel(&self, id: u64) -> bool {
        let mut g = self.jobs.lock().unwrap();
        let Some(j) = g.iter_mut().find(|j| j.id == id) else { return false };
        if j.status.finished() {
            return true;
        }
        j.cancel.store(true, Ordering::SeqCst);
        if j.status == Status::Queued {
            j.status = Status::Cancelled;
            j.finished = totp::now_secs();
            j.push("done", json!({"status": "cancelled"}));
        } else {
            j.push("status", json!({"text": "Остановка…"}));
        }
        // если агент ждёт подтверждения, снимаем ожидание отказом, чтобы он не висел
        let waiting = j.pending.take().map(|(p, _)| p["id"].as_u64().unwrap_or(0));
        drop(g);
        if let Some(a) = waiting {
            self.approvals.resolve(a, false);
        }
        true
    }

    /// Серийные номера устройств, которым владелец разрешил «режим действий».
    pub fn set_trusted(&self, serials: Vec<String>) {
        *self.trusted.lock().unwrap() = serials.into_iter().collect();
    }

    pub fn fast_of(&self, id: u64) -> bool {
        self.jobs.lock().unwrap().iter().find(|j| j.id == id).map_or(false, |j| j.fast)
    }

    pub fn history_of(&self, id: u64) -> Vec<(String, String)> {
        self.jobs.lock().unwrap().iter().find(|j| j.id == id).map(|j| j.history.clone()).unwrap_or_default()
    }

    /// Остановить все незавершённые задачи устройства (при отзыве устройства).
    pub fn cancel_device(&self, serial: &str) {
        let ids: Vec<u64> = self.jobs.lock().unwrap().iter().filter(|j| j.serial == serial && !j.status.finished()).map(|j| j.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Остановить все задачи (при выключении сервера).
    pub fn cancel_all(&self) {
        let ids: Vec<u64> = self.jobs.lock().unwrap().iter().filter(|j| !j.status.finished()).map(|j| j.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    pub fn prompt_of(&self, id: u64) -> Option<(String, String)> {
        self.jobs.lock().unwrap().iter().find(|j| j.id == id).map(|j| (j.title.clone(), j.prompt.clone()))
    }

    /// Запрос подтверждения, который сейчас ждёт ответа: (id подтверждения, опасное ли).
    pub fn pending_of(&self, job: u64) -> Option<(u64, bool)> {
        let g = self.jobs.lock().unwrap();
        let j = g.iter().find(|j| j.id == job)?;
        j.pending.as_ref().map(|(p, _)| (p["id"].as_u64().unwrap_or(0), p["dangerous"].as_bool().unwrap_or(true)))
    }

    /// Ответить на запрос подтверждения. Должен совпадать и номер задачи, и номер запроса.
    pub fn resolve(&self, job: u64, approval: u64, ok: bool) -> Result<(), String> {
        let mut g = self.jobs.lock().unwrap();
        let j = g.iter_mut().find(|j| j.id == job).ok_or("нет такой задачи")?;
        match &j.pending {
            Some((p, _)) if p["id"].as_u64() == Some(approval) => {}
            _ => return Err("этот запрос уже закрыт".into()),
        }
        j.pending = None;
        j.status = Status::Running;
        j.push("approval_resolved", json!({"id": approval, "ok": ok}));
        drop(g);
        self.approvals.resolve(approval, ok);
        Ok(())
    }

    fn expire_approvals(&self) {
        let now = totp::now_secs();
        let mut stale = vec![];
        {
            let mut g = self.jobs.lock().unwrap();
            for j in g.iter_mut() {
                if let Some((p, t)) = &j.pending {
                    if now.saturating_sub(*t) > APPROVAL_TTL_SECS {
                        stale.push(p["id"].as_u64().unwrap_or(0));
                        j.pending = None;
                        j.status = Status::Running;
                        j.push("approval_resolved", json!({"id": stale.last().copied().unwrap_or(0), "ok": false, "expired": true}));
                    }
                }
            }
        }
        for id in stale {
            self.approvals.resolve(id, false);
        }
    }

    /// Принять событие агента. Вызывается из emit контекста агента.
    pub fn emit(&self, job: u64, name: &str, p: Value) {
        let mut g = self.jobs.lock().unwrap();
        let Some(j) = g.iter_mut().find(|j| j.id == job) else { return };
        match name {
            "chat-token" => j.push("token", json!({"kind": p["kind"], "text": p["text"]})),
            "chat-status" => j.push("status", json!({"text": p["text"]})),
            "tool" => {
                if p["status"].as_str().unwrap_or("") != "running" {
                    j.steps += 1;
                }
                let out: String = p["output"].as_str().unwrap_or("").chars().take(4000).collect();
                j.push("tool", json!({"id": p["id"], "name": p["name"], "status": p["status"], "title": p["title"], "output": out}));
            }
            "approval" => {
                let kind = p["kind"].as_str().unwrap_or("");
                let trusted = !j.serial.is_empty() && self.trusted.lock().unwrap().contains(&j.serial);
                let hay = format!("{} {}", p["title"].as_str().unwrap_or(""), p["detail"].as_str().unwrap_or("")).to_lowercase();
                // Белый список: только запись и правка файла в папке проекта проходят без кода Authenticator.
                // Всё остальное (команды, скрипты, чтение вне проекта, удаление, администратор, боевой сервер, любой неизвестный вид)
                // требует свежий код. Решение зависит от вида действия, а не от текста, который формирует модель.
                let dangerous = !(kind == "write" || kind == "edit") || hay.contains("боево") || hay.contains("администратор");
                let detail: String = p["detail"].as_str().unwrap_or("").chars().take(3000).collect();
                // «Режим действий»: доверенное устройство не спрашивается об обычных действиях.
                // Удаление, права администратора и боевой сервер подтверждаются всегда, с кодом Authenticator.
                let hard = kind == "delete" || hay.contains("боево") || hay.contains("администратор");
                if trusted && !hard {
                    let aid = p["id"].as_u64().unwrap_or(0);
                    j.push("approval_auto", json!({"id": aid, "kind": kind, "title": p["title"]}));
                    drop(g);
                    self.approvals.resolve(aid, true);
                    return;
                }
                let a = json!({"id": p["id"], "kind": kind, "title": p["title"], "detail": detail, "dangerous": dangerous});
                j.pending = Some((a.clone(), totp::now_secs()));
                j.status = Status::Waiting;
                j.push("approval", a);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub_with(exec: Exec) -> Arc<Hub> {
        let h = Hub::new(Arc::new(Approvals::default()));
        h.start(exec);
        h
    }

    fn wait_done(h: &Hub, id: u64) -> Value {
        for _ in 0..100 {
            let v = h.poll(id, 0, Duration::from_millis(100)).unwrap();
            if matches!(v["status"].as_str(), Some("done" | "error" | "cancelled")) {
                return v;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("задача не завершилась");
    }

    #[test]
    fn runs_and_reports_events_without_fake_progress() {
        let h = hub_with(Arc::new(|h, id, prompt, _| {
            h.emit(id, "chat-token", json!({"kind": "content", "text": "при"}));
            h.emit(id, "tool", json!({"id": "1", "name": "x", "status": "ok", "title": "t", "output": "o"}));
            Ok(format!("ответ на {prompt}"))
        }));
        let id = h.create("привет", "", "tel").unwrap();
        let v = wait_done(&h, id);
        assert_eq!(v["status"], "done");
        assert_eq!(v["result"], "ответ на привет");
        assert!(v["progress"].is_null(), "процентов быть не должно");
        assert_eq!(v["steps"], 1);
        let kinds: Vec<_> = v["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
        assert!(kinds.contains(&"token".into()) && kinds.contains(&"tool".into()) && kinds.last() == Some(&"done".to_string()));
        // события после последнего номера не повторяются
        let last = v["last_seq"].as_u64().unwrap();
        assert!(h.poll(id, last, Duration::from_millis(10)).unwrap()["events"].as_array().unwrap().is_empty());
    }

    #[test]
    fn approval_must_match_and_unblocks_agent() {
        let (tx, rx) = std::sync::mpsc::channel::<u64>();
        let tx = Mutex::new(tx);
        let h = hub_with(Arc::new(move |h, id, _, _| {
            let (aid, rcv) = {
                // имитация agent::Ctx::ask
                let a = h.approvals.clone();
                let (aid, r) = a.create();
                (aid, r)
            };
            h.emit(id, "approval", json!({"id": aid, "kind": "delete", "title": "Удалить a.txt", "detail": "d"}));
            tx.lock().unwrap().send(aid).unwrap();
            if rcv.recv_timeout(Duration::from_secs(5)).unwrap_or(false) { Ok("ок".into()) } else { Err("отклонено".into()) }
        }));
        let id = h.create("удали", "", "tel").unwrap();
        let aid = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let v = h.poll(id, 0, Duration::from_millis(10)).unwrap();
        assert_eq!(v["status"], "waiting");
        assert_eq!(v["pending"]["dangerous"], true);
        assert_eq!(h.pending_of(id), Some((aid, true)));
        assert!(h.resolve(id, aid + 99, true).is_err(), "чужой номер запроса не подходит");
        h.resolve(id, aid, false).unwrap();
        assert!(h.resolve(id, aid, true).is_err(), "повторный ответ не принимается");
        assert_eq!(wait_done(&h, id)["status"], "error");
    }

    #[test]
    fn only_file_edits_in_project_skip_the_code() {
        let h = hub_with(Arc::new(|_, _, _, c| {
            while !c.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err("stop".into())
        }));
        let id = h.create("x", "", "t").unwrap();
        std::thread::sleep(Duration::from_millis(150));
        for (kind, title, want) in [("write", "Файл a", false), ("edit", "Файл a", false), ("run", "команда", true), ("script", "скрипт", true), ("delete", "x", true), ("network", "x", true), ("write", "БОЕВОЙ сервер", true), ("", "x", true)] {
            h.emit(id, "approval", json!({"id": 1, "kind": kind, "title": title, "detail": ""}));
            assert_eq!(h.pending_of(id).map(|p| p.1), Some(want), "{kind} {title}");
        }
        h.shutdown();
    }

    #[test]
    fn revoking_a_device_cancels_its_tasks_only() {
        let h = hub_with(Arc::new(|_, _, _, c| {
            while !c.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err("stop".into())
        }));
        let a = h.create_with("x", "", "A", "serA", vec![], false).unwrap();
        let b = h.create_with("y", "", "B", "serB", vec![], false).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        h.cancel_device("serA");
        assert_eq!(wait_done(&h, a)["status"], "cancelled");
        assert_ne!(h.poll(b, 0, Duration::from_millis(10)).unwrap()["status"], "cancelled", "чужая задача не затронута");
        h.cancel_all();
        assert_eq!(wait_done(&h, b)["status"], "cancelled");
    }

    #[test]
    fn cancel_queued_and_running() {
        let h = hub_with(Arc::new(|_, _, _, cancel| {
            for _ in 0..100 {
                if cancel.load(Ordering::SeqCst) {
                    return Err("stop".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok("x".into())
        }));
        let a = h.create("долго", "", "t").unwrap();
        let b = h.create("в очереди", "", "t").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(h.cancel(b));
        assert_eq!(h.poll(b, 0, Duration::from_millis(10)).unwrap()["status"], "cancelled");
        assert!(h.cancel(a));
        assert_eq!(wait_done(&h, a)["status"], "cancelled");
    }

    #[test]
    fn limits() {
        let h = hub_with(Arc::new(|_, _, _, c| {
            while !c.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err("stop".into())
        }));
        assert!(h.create("  ", "", "t").is_err());
        assert!(h.create(&"я".repeat(MAX_PROMPT), "", "t").is_err());
        for _ in 0..MAX_ACTIVE {
            h.create("x", "", "t").unwrap();
        }
        assert!(h.create("x", "", "t").is_err(), "очередь ограничена");
        h.shutdown();
    }
}
