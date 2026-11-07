//! Сохранённые разговоры: несколько чатов, список, открыть, переименовать, удалить. Каждый разговор лежит в
//! `%LOCALAPPDATA%\Nexora\chats\<номер>.json` целиком (с историей инструментов), поэтому после открытия модель
//! продолжает с тем же контекстом. Картинки в файл не пишутся (вместо них пометка), чтобы файлы оставались небольшими.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

const MAX_CHATS: usize = 200;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Chat {
    pub id: u64,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    pub conv: Vec<Value>,
}

pub struct Store(pub PathBuf);

pub fn default_store() -> Store {
    Store(crate::engine::data_dir().join("chats"))
}

fn strip_images(conv: &[Value]) -> Vec<Value> {
    conv.iter()
        .map(|m| {
            let mut m = m.clone();
            if let Some(parts) = m["content"].as_array().cloned() {
                m["content"] = Value::Array(parts.into_iter().map(|p| if p["type"] == "image_url" { json!({"type": "text", "text": "[картинка]"}) } else { p }).collect());
            }
            m
        })
        .collect()
}

/// Название по первому сообщению пользователя.
pub fn title_of(conv: &[Value]) -> String {
    let t = conv.iter().find(|m| m["role"] == "user").map(crate::router::text_of).unwrap_or_default();
    let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = one.chars().take(60).collect();
    if cut.is_empty() { "Новый разговор".to_string() } else if one.chars().count() > 60 { format!("{cut}…") } else { cut }
}

/// Что показать в окне при открытии: реплики пользователя и ответы (инструменты кратко, перечислением).
pub fn display(conv: &[Value]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    let mut bot = String::new();
    let mut tools: Vec<String> = vec![];
    let flush = |out: &mut Vec<(String, String)>, bot: &mut String, tools: &mut Vec<String>| {
        let mut t = bot.trim().to_string();
        if !tools.is_empty() {
            if !t.is_empty() {
                t.push_str("\n\n");
            }
            t.push_str(&format!("(использованы инструменты: {})", tools.join(", ")));
        }
        if !t.is_empty() {
            out.push(("bot".into(), t));
        }
        bot.clear();
        tools.clear();
    };
    for m in conv {
        match m["role"].as_str() {
            Some("user") => {
                flush(&mut out, &mut bot, &mut tools);
                let t = crate::router::text_of(m);
                if !t.trim().is_empty() {
                    out.push(("user".into(), t));
                }
            }
            Some("assistant") => {
                if let Some(c) = m["content"].as_str() {
                    if !c.trim().is_empty() {
                        if !bot.is_empty() {
                            bot.push_str("\n\n");
                        }
                        bot.push_str(c.trim());
                    }
                }
                for c in m["tool_calls"].as_array().cloned().unwrap_or_default() {
                    if let Some(n) = c["function"]["name"].as_str() {
                        if !tools.contains(&n.to_string()) {
                            tools.push(n.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut out, &mut bot, &mut tools);
    out
}

impl Store {
    fn path(&self, id: u64) -> PathBuf {
        self.0.join(format!("{id}.json"))
    }

    fn write(&self, c: &Chat) -> Result<(), String> {
        std::fs::create_dir_all(&self.0).map_err(|e| e.to_string())?;
        let tmp = self.path(c.id).with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string(c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.path(c.id)).map_err(|e| e.to_string())
    }

    pub fn load(&self, id: u64) -> Option<Chat> {
        std::fs::read_to_string(self.path(id)).ok().and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Все разговоры, новые сверху.
    pub fn list(&self) -> Vec<Chat> {
        let mut v: Vec<Chat> = std::fs::read_dir(&self.0)
            .map(|rd| rd.flatten().filter(|e| e.path().extension().map_or(false, |x| x == "json")).filter_map(|e| std::fs::read_to_string(e.path()).ok()).filter_map(|s| serde_json::from_str::<Chat>(&s).ok()).collect())
            .unwrap_or_default();
        v.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
        v
    }

    /// Сохранить разговор; id None создаёт новый. Возвращает номер.
    pub fn save(&self, id: Option<u64>, workspace: Option<String>, remote: Option<String>, conv: &[Value]) -> Result<u64, String> {
        let now = crate::totp::now_secs();
        let existing = id.and_then(|i| self.load(i));
        let chat = match existing {
            Some(mut c) => {
                c.updated = now;
                c.workspace = workspace;
                c.remote = remote;
                c.conv = strip_images(conv);
                c
            }
            None => {
                let mut nid = id.unwrap_or(now * 1000 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_millis()).unwrap_or(0)) as u64);
                while self.path(nid).exists() {
                    nid += 1;
                }
                Chat { id: nid, title: title_of(conv), created: now, updated: now, workspace, remote, conv: strip_images(conv) }
            }
        };
        self.write(&chat)?;
        // не больше MAX_CHATS: лишние самые старые удаляются
        let all = self.list();
        for old in all.iter().skip(MAX_CHATS) {
            let _ = std::fs::remove_file(self.path(old.id));
        }
        Ok(chat.id)
    }

    pub fn delete(&self, id: u64) {
        let _ = std::fs::remove_file(self.path(id));
    }

    pub fn rename(&self, id: u64, title: &str) -> Result<(), String> {
        let mut c = self.load(id).ok_or("такого разговора нет")?;
        let t: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        if t.is_empty() {
            return Err("название не может быть пустым".into());
        }
        c.title = t;
        self.write(&c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> Store {
        let d = std::env::temp_dir().join(format!("nexora-chats-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Store(d)
    }

    fn conv(q: &str) -> Vec<Value> {
        vec![
            json!({"role": "system", "content": "s"}),
            json!({"role": "user", "content": q}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "1", "content": "данные"}),
            json!({"role": "assistant", "content": "Готово: прочитал файл."}),
        ]
    }

    #[test]
    fn titles() {
        assert_eq!(title_of(&conv("Хочу   написать\nигру")), "Хочу написать игру");
        assert!(title_of(&conv(&"а".repeat(100))).ends_with('…'));
        assert_eq!(title_of(&[json!({"role": "system", "content": "s"})]), "Новый разговор");
        let c = conv("напиши игру\n\n[Автоподбор навыков]\nПодключён навык «start»");
        assert_eq!(title_of(&c), "напиши игру", "подклеенный навык в название не попадает");
    }

    #[test]
    fn display_merges_tools_into_reply() {
        let d = display(&conv("привет"));
        assert_eq!(d.len(), 2);
        assert_eq!(d[0], ("user".to_string(), "привет".to_string()));
        assert!(d[1].1.contains("Готово: прочитал файл.") && d[1].1.contains("read_file"), "{:?}", d[1]);
    }

    #[test]
    fn save_list_open_rename_delete() {
        let s = store("a");
        let id1 = s.save(None, Some("C:\\p1".into()), None, &conv("первый")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let id2 = s.save(None, None, Some("srv".into()), &conv("второй")).unwrap();
        assert_ne!(id1, id2);
        let l = s.list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].id, id2, "новые сверху");
        // повторное сохранение обновляет тот же разговор, а не создаёт новый
        let mut c = conv("второй");
        c.push(json!({"role": "user", "content": "ещё"}));
        assert_eq!(s.save(Some(id2), None, Some("srv".into()), &c).unwrap(), id2);
        assert_eq!(s.list().len(), 2);
        let loaded = s.load(id2).unwrap();
        assert_eq!((loaded.title.as_str(), loaded.remote.as_deref(), loaded.conv.len()), ("второй", Some("srv"), 6));
        assert_eq!(s.load(id1).unwrap().workspace.as_deref(), Some("C:\\p1"));
        s.rename(id1, "  Моя   игра ").unwrap();
        assert_eq!(s.load(id1).unwrap().title, "Моя игра");
        assert!(s.rename(id1, "   ").is_err());
        assert!(s.rename(999, "x").is_err());
        s.delete(id1);
        assert!(s.load(id1).is_none());
        assert_eq!(s.list().len(), 1);
        let _ = std::fs::remove_dir_all(&s.0);
    }

    #[test]
    fn images_are_not_stored() {
        let s = store("img");
        let c = vec![json!({"role": "user", "content": [{"type": "text", "text": "смотри"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]})];
        let id = s.save(None, None, None, &c).unwrap();
        let raw = std::fs::read_to_string(s.path(id)).unwrap();
        assert!(!raw.contains("base64") && raw.contains("[картинка]"), "{raw}");
        let _ = std::fs::remove_dir_all(&s.0);
    }

    #[test]
    fn limit_keeps_newest() {
        let s = store("lim");
        for i in 0..(MAX_CHATS + 3) {
            let c = Chat { id: 1000 + i as u64, title: format!("t{i}"), created: 1, updated: 100 + i as u64, workspace: None, remote: None, conv: vec![] };
            s.write(&c).unwrap();
        }
        s.save(None, None, None, &conv("новый")).unwrap();
        let l = s.list();
        assert_eq!(l.len(), MAX_CHATS);
        assert!(l.iter().all(|c| c.title != "t0"), "самый старый удалён");
        let _ = std::fs::remove_dir_all(&s.0);
    }
}
