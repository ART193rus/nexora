//! Автоматизация: задачи по расписанию. Агент выполняет их сам, пока программа запущена и модель готова.
//! Расписание: каждый день в HH:MM (по местному времени; если программа была закрыта, задача выполнится при
//! следующем запуске в тот же день) или каждые N минут. Задача привязана к проекту (папка или сервер), который
//! был выбран при создании. Без пользователя вопросов нет: всё, что требует подтверждения, отклоняется.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Task {
    #[serde(default)]
    pub id: u64,
    pub name: String,
    pub prompt: String,
    pub workspace: Option<String>,
    pub remote: Option<String>,
    /// "daily" или "every"
    pub kind: String,
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub every_min: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub last_run: u64,
    #[serde(default)]
    pub last_day: String,
    #[serde(default)]
    pub last_status: String,
    #[serde(default)]
    pub last_file: String,
}

fn yes() -> bool {
    true
}

pub struct Local {
    pub date: String,
    pub minutes: u32,
}

#[cfg(windows)]
pub fn local_now() -> Local {
    #[repr(C)]
    struct St {
        y: u16,
        mo: u16,
        dow: u16,
        d: u16,
        h: u16,
        mi: u16,
        s: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut St);
    }
    let mut t = St { y: 0, mo: 0, dow: 0, d: 0, h: 0, mi: 0, s: 0, ms: 0 };
    unsafe { GetLocalTime(&mut t) };
    Local { date: format!("{:04}-{:02}-{:02}", t.y, t.mo, t.d), minutes: t.h as u32 * 60 + t.mi as u32 }
}

#[cfg(not(windows))]
pub fn local_now() -> Local {
    let (y, m, d, h, mi) = civil(crate::totp::now_secs());
    Local { date: format!("{y:04}-{m:02}-{d:02}"), minutes: h * 60 + mi }
}

/// Эпоха (UTC) в гражданскую дату: (год, месяц, день, час, минута).
#[allow(dead_code)]
pub fn civil(secs: u64) -> (i64, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, rem / 3600, (rem % 3600) / 60)
}

pub fn parse_hm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if h < 24 && m < 60 { Some(h * 60 + m) } else { None }
}

pub fn validate(t: &Task) -> Result<(), String> {
    if t.name.trim().is_empty() || t.name.chars().count() > 60 {
        return Err("название от 1 до 60 символов".into());
    }
    if t.prompt.trim().is_empty() || t.prompt.chars().count() > 4000 {
        return Err("описание задачи от 1 до 4000 символов".into());
    }
    if t.workspace.is_none() && t.remote.is_none() {
        return Err("сначала выберите проект (папку или сервер): задача привязывается к нему".into());
    }
    match t.kind.as_str() {
        "daily" => {
            parse_hm(&t.at).ok_or("время в формате ЧЧ:ММ, например 09:00")?;
        }
        "every" => {
            if !(5..=10_080).contains(&t.every_min) {
                return Err("интервал от 5 минут до 7 суток".into());
            }
        }
        _ => return Err("неизвестный тип расписания".into()),
    }
    Ok(())
}

pub fn is_due(t: &Task, now: u64, local: &Local) -> bool {
    if !t.enabled {
        return false;
    }
    match t.kind.as_str() {
        "daily" => parse_hm(&t.at).map_or(false, |m| local.minutes >= m && t.last_day != local.date),
        "every" => t.every_min >= 5 && now >= t.last_run + t.every_min as u64 * 60,
        _ => false,
    }
}

pub struct Store(pub PathBuf);

pub fn default_store() -> Store {
    Store(crate::engine::data_dir().join("tasks.json"))
}

pub fn reports_dir() -> PathBuf {
    crate::engine::data_dir().join("tasks")
}

impl Store {
    fn load(&self) -> Vec<Task> {
        std::fs::read_to_string(&self.0).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(&self, v: &[Task]) -> Result<(), String> {
        if let Some(d) = self.0.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = self.0.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.0).map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Vec<Task> {
        let _g = LOCK.lock().unwrap();
        self.load()
    }

    pub fn get(&self, id: u64) -> Option<Task> {
        self.list().into_iter().find(|t| t.id == id)
    }

    /// Новая задача; для интервала отсчёт начинается с момента создания.
    pub fn add(&self, mut t: Task, now: u64) -> Result<u64, String> {
        validate(&t)?;
        let _g = LOCK.lock().unwrap();
        let mut all = self.load();
        if all.len() >= 50 {
            return Err("задач уже 50: удалите лишние".into());
        }
        t.id = all.iter().map(|x| x.id).max().unwrap_or(0) + 1;
        t.enabled = true;
        t.last_run = now;
        t.last_day = String::new();
        t.last_status = String::new();
        t.last_file = String::new();
        let id = t.id;
        all.push(t);
        self.save(&all)?;
        Ok(id)
    }

    pub fn remove(&self, id: u64) -> Result<(), String> {
        let _g = LOCK.lock().unwrap();
        let mut all = self.load();
        all.retain(|t| t.id != id);
        self.save(&all)
    }

    pub fn set_enabled(&self, id: u64, on: bool) -> Result<(), String> {
        let _g = LOCK.lock().unwrap();
        let mut all = self.load();
        if let Some(t) = all.iter_mut().find(|t| t.id == id) {
            t.enabled = on;
        }
        self.save(&all)
    }

    pub fn record(&self, id: u64, now: u64, day: &str, status: &str, file: &str) -> Result<(), String> {
        let _g = LOCK.lock().unwrap();
        let mut all = self.load();
        if let Some(t) = all.iter_mut().find(|t| t.id == id) {
            t.last_run = now;
            t.last_day = day.to_string();
            t.last_status = status.chars().take(300).collect();
            t.last_file = file.to_string();
        }
        self.save(&all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(kind: &str) -> Task {
        Task {
            id: 0,
            name: "Проверка логов".into(),
            prompt: "Посмотри логи за сутки".into(),
            workspace: Some("C:\\p".into()),
            remote: None,
            kind: kind.into(),
            at: "09:00".into(),
            every_min: 60,
            enabled: true,
            last_run: 0,
            last_day: String::new(),
            last_status: String::new(),
            last_file: String::new(),
        }
    }

    fn at(date: &str, h: u32, m: u32) -> Local {
        Local { date: date.into(), minutes: h * 60 + m }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil(951_782_400 + 3600 * 13 + 60 * 7), (2000, 2, 29, 13, 7)); // високосный день
        assert_eq!(civil(1_793_999_999 + 1), civil(1_794_000_000));
    }

    #[test]
    fn hm_parsing() {
        assert_eq!(parse_hm("09:30"), Some(570));
        assert_eq!(parse_hm(" 0:05 "), Some(5));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("9"), None);
        assert_eq!(parse_hm("12:60"), None);
    }

    #[test]
    fn daily_due_rules() {
        let mut t = task("daily");
        assert!(!is_due(&t, 0, &at("2026-11-06", 8, 59)), "рано");
        assert!(is_due(&t, 0, &at("2026-11-06", 9, 0)));
        assert!(is_due(&t, 0, &at("2026-11-06", 17, 30)), "программа была закрыта: выполнится при запуске в тот же день");
        t.last_day = "2026-11-06".into();
        assert!(!is_due(&t, 0, &at("2026-11-06", 17, 30)), "сегодня уже выполнена");
        assert!(is_due(&t, 0, &at("2026-11-07", 9, 1)), "на следующий день снова");
        t.enabled = false;
        assert!(!is_due(&t, 0, &at("2026-11-08", 10, 0)));
    }

    #[test]
    fn interval_due_rules() {
        let mut t = task("every");
        t.every_min = 30;
        t.last_run = 1000;
        assert!(!is_due(&t, 1000 + 29 * 60, &at("d", 0, 0)));
        assert!(is_due(&t, 1000 + 30 * 60, &at("d", 0, 0)));
    }

    #[test]
    fn validation() {
        assert!(validate(&task("daily")).is_ok());
        assert!(validate(&Task { at: "25:00".into(), ..task("daily") }).is_err());
        assert!(validate(&Task { every_min: 2, ..task("every") }).is_err());
        assert!(validate(&Task { name: " ".into(), ..task("daily") }).is_err());
        assert!(validate(&Task { prompt: "".into(), ..task("daily") }).is_err());
        assert!(validate(&Task { workspace: None, remote: None, ..task("daily") }).is_err());
        assert!(validate(&task("weekly")).is_err());
    }

    #[test]
    fn store_flow() {
        let d = std::env::temp_dir().join(format!("nexora-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = Store(d.join("tasks.json"));
        let id = s.add(task("every"), 500).unwrap();
        assert_eq!(s.get(id).unwrap().last_run, 500, "интервал считается от создания");
        s.set_enabled(id, false).unwrap();
        assert!(!s.get(id).unwrap().enabled);
        s.record(id, 900, "2026-11-06", "ок", "r.md").unwrap();
        let t = s.get(id).unwrap();
        assert_eq!((t.last_run, t.last_day.as_str(), t.last_file.as_str()), (900, "2026-11-06", "r.md"));
        let id2 = s.add(task("daily"), 0).unwrap();
        assert_eq!(id2, id + 1);
        s.remove(id).unwrap();
        assert_eq!(s.list().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
