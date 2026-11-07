//! Подбор инструментов и навыков под запрос, чтобы вводная часть запроса к модели была короткой.
//! На процессоре без видеокарты модель читает вводную часть со скоростью около 15 токенов в секунду, поэтому
//! 6500 токенов (все инструменты и список навыков) это 7 минут молчания. Здесь:
//!  - обычный вопрос: без инструментов, только служебный `need_tools` (модель вызовет его, если инструменты нужны);
//!  - задача: базовые инструменты + группы по ключевым словам (браузер, реестр, поиск программ, MCP, скрипты);
//!  - навык: по тексту запроса (в том числе по-русски) подбирается лучший, его инструкции подключаются автоматически.
//! Выбор делается без вызова модели, правилами. Раз перейдя в режим задачи, диалог в нём остаётся.
use serde_json::Value;
use std::collections::HashSet;

/// Слова, по которым запрос считается задачей, а не разговором.
const TASK_WORDS: &[&str] = &[
    "файл", "папк", "код", "программ", "скрипт", "команд", "запусти", "запуск", "установ", "созда", "сдела", "напиши", "написа", "исправ", "почини", "ошибк", "баг", "тест",
    "собер", "сборк", "проект", "приложен", "игр", "сайт", "страниц", "репозитор", "git", "сервер", "ssh", "удали", "измени", "поправ", "добавь", "открой", "найди", "проверь",
    "отлад", "настрой", "реестр", "браузер", "mcp", "мцп", "навык", "скилл", "плагин", "диск", "разработ", "функци", "модул", "скомпил", "билд", "build", "docker", "python",
    "rust", "javascript", "html", "css", "flutter", "android", "apk", ".exe", ".py", ".js", ".rs", ".md", ".json", "c:\\", "d:\\", "```",
];

/// Базовые инструменты задачи.
const CORE: &[&str] = &["list_dir", "read_file", "write_file", "edit_file", "delete_file", "run_command", "start_app", "list_apps", "stop_app", "use_skill", "list_skills", "remember", "forget"];

/// Группы по требованию: (слова-триггеры, инструменты).
const GROUPS: &[(&[&str], &[&str])] = &[
    (&["браузер", "сайт", "страниц", "http", "url", "веб", "интернет", "ссылк"], &["open_url", "browser_open", "browser_read", "browser_click", "browser_type", "browser_screenshot", "browser_close"]),
    (&["реестр", "registry", "regedit", "hkcu", "hklm"], &["registry_read", "registry_write", "registry_delete"]),
    (&["программ", "приложен", "установлен", "найди файл", "поиск файл", "диск", "где лежит", "администратор", "права админ", ".exe"], &["find_apps", "search_files", "run_elevated"]),
    (&["mcp", "мцп"], &["mcp_list", "mcp_add", "mcp_remove"]),
    (&["скрипт"], &["run_skill_script", "read_skill_file"]),
];

/// Приблизительный перевод частых русских тем в английские слова: описания навыков обычно английские.
const RU_EN: &[(&str, &[&str])] = &[
    ("игр", &["game"]), ("сайт", &["website", "web", "frontend"]), ("веб", &["web"]), ("приложен", &["app", "mobile"]), ("мобил", &["mobile"]), ("тест", &["test"]), ("исправ", &["fix"]), ("почини", &["fix"]),
    ("отлад", &["debug"]), ("баг", &["bug", "debug"]), ("ошибк", &["error", "debug", "bug"]), ("безопасн", &["security"]), ("база данн", &["database", "sql"]), ("баз данн", &["database", "sql"]),
    ("дизайн", &["design"]), ("интерфейс", &["interface", "design"]), ("документ", &["document", "docs"]), ("презентац", &["presentation", "slides", "pptx"]),
    ("таблиц", &["spreadsheet", "xlsx"]), ("рефактор", &["refactor"]), ("ревью", &["review"]), ("план", &["plan"]), ("архитектур", &["architecture"]),
    ("производительн", &["performance"]), ("оптимиз", &["performance", "optimize"]), ("релиз", &["release", "ship"]), ("коммит", &["git", "commit"]),
    ("музык", &["music", "audio"]), ("звук", &["audio", "sound"]), ("видео", &["video"]), ("картинк", &["image"]), ("персонаж", &["character"]), ("уровн", &["level"]),
    ("механик", &["mechanic", "gameplay"]), ("прототип", &["prototype"]), ("сюжет", &["narrative", "story"]), ("анимац", &["animation"]), ("баланс", &["balance"]),
    ("идея", &["idea", "brainstorm"]), ("придума", &["brainstorm", "idea"]), ("стиль", &["style"]), ("график", &["art", "graphics"]), ("сервер", &["server", "backend"]),
];

const STOP: &[&str] = &["that", "this", "with", "from", "have", "what", "your", "make", "write", "help", "please", "about", "need", "want", "sure", "some", "just", "like"];

const MARK: &str = "[Автоподбор навыков]";

/// Текст сообщения без автоматически подклеенных заметок о навыках.
pub fn text_of(m: &Value) -> String {
    let raw = match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    match raw.find(&format!("\n\n{MARK}")) {
        Some(i) => raw[..i].to_string(),
        None => raw,
    }
}

pub fn wants_task(text: &str) -> bool {
    let l = text.to_lowercase();
    TASK_WORDS.iter().any(|w| l.contains(w))
}

pub struct Profile {
    pub task: bool,
    /// Разрешённые имена инструментов (кроме динамических MCP).
    pub names: Vec<String>,
}

/// Профиль для текущего состояния диалога. `escalated`: модель сама вызвала need_tools.
pub fn profile_for(conv: &[Value], escalated: bool) -> Profile {
    profile_for_web(conv, escalated, crate::prefs::web())
}

/// То же с явным признаком «Веб-поиск включён» (для проверок без общих настроек).
pub fn profile_for_web(conv: &[Value], escalated: bool, web: bool) -> Profile {
    let users: Vec<String> = conv.iter().filter(|m| m["role"] == "user").map(text_of).collect();
    let all = users.join("\n").to_lowercase();
    let mut used: HashSet<String> = HashSet::new();
    for m in conv {
        if let Some(tc) = m["tool_calls"].as_array() {
            for c in tc {
                if let Some(n) = c["function"]["name"].as_str() {
                    if n != "need_tools" {
                        used.insert(n.to_string());
                    }
                }
            }
        }
    }
    let task = escalated || !used.is_empty() || users.iter().any(|u| wants_task(u));
    if !task {
        let mut n = vec!["need_tools".to_string()];
        if web {
            n.extend(["web_search".to_string(), "web_fetch".to_string()]);
        }
        return Profile { task: false, names: n };
    }
    let mut names: Vec<String> = CORE.iter().map(|s| s.to_string()).collect();
    for (trig, tools) in GROUPS {
        if trig.iter().any(|w| all.contains(w)) || tools.iter().any(|t| used.contains(*t)) {
            names.extend(tools.iter().map(|s| s.to_string()));
        }
    }
    if web {
        names.extend(["web_search".to_string(), "web_fetch".to_string()]);
    }
    Profile { task: true, names }
}

fn first_sentence(s: &str, max: usize) -> String {
    let cut = [". ", ".\n", ": "].iter().filter_map(|d| s.find(d)).min().map(|i| i + 1).unwrap_or(s.len());
    s[..cut].chars().take(max).collect::<String>().trim().to_string()
}

/// Сжать описание инструмента до первого предложения: остальное модель угадывает по имени и параметрам.
pub fn compact(t: &Value) -> Value {
    let mut t = t.clone();
    if let Some(d) = t["function"]["description"].as_str() {
        t["function"]["description"] = Value::String(first_sentence(d, 110));
    }
    t
}

/// Список инструментов для запроса к модели (сжатые описания), с динамическими MCP при задаче.
pub fn select(p: &Profile, mcp_extra: Vec<Value>) -> Value {
    let all = crate::tools::schemas();
    let mut out: Vec<Value> = all.as_array().cloned().unwrap_or_default().iter().filter(|t| p.names.iter().any(|n| t["function"]["name"] == n.as_str())).map(compact).collect();
    if p.task {
        out.extend(mcp_extra.iter().map(compact));
    }
    Value::Array(out)
}

/// Ключевые слова запроса: английские слова как есть и английские эквиваленты русских тем.
pub fn keywords(text: &str) -> Vec<String> {
    let l = text.to_lowercase();
    let mut k: Vec<String> = vec![];
    for (stem, en) in RU_EN {
        if l.contains(stem) {
            k.extend(en.iter().map(|s| s.to_string()));
        }
    }
    for w in l.split(|c: char| !c.is_alphanumeric()) {
        if w.len() >= 4 && w.is_ascii() && !STOP.contains(&w) {
            k.push(w.to_string());
        }
    }
    let mut seen = HashSet::new();
    k.retain(|x| seen.insert(x.clone()));
    k
}

/// Слова из текста: латиница и цифры, нижний регистр.
fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Слово запроса совпадает со словом имени или описания: равно или является началом (tests = test), но не серединой (playtest).
fn hit(k: &str, token: &str) -> bool {
    token == k || (k.len() >= 4 && token.starts_with(k))
}

/// Общие точки входа для просьбы «создать/сделать новое»: навыки с такими именами подходят почти к любому старту.
fn entry_keywords(text: &str) -> Vec<(&'static str, f32)> {
    let l = text.to_lowercase();
    if ["хочу", "созда", "нов", "придума", "начн", "сдела", "напиш", "написа"].iter().any(|w| l.contains(w)) {
        vec![("start", 2.2), ("brainstorm", 2.1), ("new", 2.0), ("init", 2.0), ("create", 1.9)]
    } else {
        vec![]
    }
}

/// Навыки по убыванию соответствия запросу: (индекс, баллы в десятых). Слово запроса в имени навыка весит 3, в описании 1;
/// слова, которые есть у трети навыков и более (например game в игровом наборе), почти не считаются.
pub fn score_skills(skills: &[(String, String)], text: &str) -> Vec<(usize, i32)> {
    let kws = keywords(text);
    let entry = entry_keywords(text);
    let toks: Vec<(Vec<String>, Vec<String>)> = skills.iter().map(|(n, d)| (words(n), words(d))).collect();
    let n = skills.len().max(1);
    let weight = |k: &str| -> f32 {
        let df = toks.iter().filter(|(nt, dt)| nt.iter().chain(dt.iter()).any(|t| hit(k, t))).count();
        if df * 3 > n { 0.3 } else { 1.0 }
    };
    let ws: Vec<f32> = kws.iter().map(|k| weight(k)).collect();
    let mut v: Vec<(usize, i32)> = toks
        .iter()
        .enumerate()
        .map(|(i, (nt, dt))| {
            let mut s = 0.0f32;
            for (k, w) in kws.iter().zip(&ws) {
                if nt.iter().any(|t| hit(k, t)) {
                    s += 3.0 * w;
                } else if dt.iter().any(|t| hit(k, t)) {
                    s += 1.0 * w;
                }
            }
            for (k, w) in &entry {
                if nt.iter().any(|t| t == k) {
                    s += w;
                }
            }
            (i, (s * 10.0).round() as i32)
        })
        .filter(|(_, s)| *s > 0)
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(skills[a.0].1.len().cmp(&skills[b.0].1.len())));
    v
}

/// Подклеить к последнему сообщению пользователя подходящий навык (инструкции до 2500 символов) и имена похожих.
/// Один раз на запрос; уже подключённый в диалоге навык повторно не вставляется.
pub fn attach_skill(conv: &mut Vec<Value>) {
    let Some(idx) = conv.iter().rposition(|m| m["role"] == "user") else { return };
    let Value::String(full) = conv[idx]["content"].clone() else { return };
    if full.contains(MARK) {
        return;
    }
    let skills = crate::plugins::skill_index();
    if skills.is_empty() {
        return;
    }
    let ranked = score_skills(&skills, &full);
    let Some(&(best, score)) = ranked.first() else { return };
    let history: String = conv.iter().filter(|m| m["role"] == "user").map(|m| m["content"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("\n");
    let mut note = String::new();
    let mut skip = 0;
    if score >= 20 && !history.contains(&format!("Подключён навык «{}»", skills[best].0)) {
        if let Ok(body) = crate::plugins::read_skill(&skills[best].0) {
            let body: String = body.chars().take(2500).collect();
            note.push_str(&format!("\nПодключён навык «{}» (подобран автоматически по запросу). Следуй его инструкциям:\n{body}", skills[best].0));
            skip = 1;
        }
    }
    let others: Vec<&str> = ranked.iter().skip(skip).take(3).map(|(i, _)| skills[*i].0.as_str()).collect();
    if !others.is_empty() {
        note.push_str(&format!("\nПохожие навыки: {} (подробности: use_skill, поиск: list_skills).", others.join(", ")));
    }
    if note.is_empty() {
        return;
    }
    conv[idx]["content"] = Value::String(format!("{full}\n\n{MARK}{note}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn u(t: &str) -> Value {
        json!({"role": "user", "content": t})
    }

    #[test]
    fn chat_vs_task() {
        for chat in ["привет", "что такое дифференциал", "расскажи про историю Рима", "как дела?", "объясни, чем кошка отличается от собаки"] {
            assert!(!wants_task(chat), "{chat}");
        }
        for task in ["хочу написать игру", "исправь ошибку в main.rs", "создай файл readme.md", "запусти тесты", "открой сайт example.com", "почини сборку"] {
            assert!(wants_task(task), "{task}");
        }
    }

    #[test]
    fn profile_chat_gets_only_gate() {
        let c = vec![json!({"role": "system", "content": "s"}), u("привет, как тебя зовут?")];
        let p = profile_for(&c, false);
        assert!(!p.task && p.names == vec!["need_tools".to_string()]);
        let sel = select(&p, vec![]);
        assert_eq!(sel.as_array().unwrap().len(), 1);
        assert!(sel.to_string().len() < 400, "вопрос без инструментов почти не добавляет токенов");
    }

    #[test]
    fn profile_task_core_and_groups() {
        let p = profile_for(&[u("хочу написать игру на Python")], false);
        assert!(p.task && p.names.contains(&"run_command".to_string()));
        assert!(!p.names.contains(&"browser_open".to_string()) && !p.names.contains(&"registry_read".to_string()), "лишние группы не подключаются");
        let p = profile_for(&[u("открой сайт и проверь страницу")], false);
        assert!(p.names.contains(&"browser_open".to_string()));
        let p = profile_for(&[u("покажи значение в реестре HKCU")], false);
        assert!(p.names.contains(&"registry_read".to_string()) && !p.names.contains(&"browser_open".to_string()));
        let p = profile_for(&[u("добавь mcp сервер")], false);
        assert!(p.names.contains(&"mcp_add".to_string()));
    }

    #[test]
    fn web_toggle_adds_search_tools_in_both_modes() {
        let chat = profile_for_web(&[u("что нового в мире")], false, true);
        assert!(!chat.task && chat.names.contains(&"web_search".to_string()) && chat.names.contains(&"web_fetch".to_string()) && chat.names.contains(&"need_tools".to_string()));
        let task = profile_for_web(&[u("создай файл")], false, true);
        assert!(task.task && task.names.contains(&"web_search".to_string()));
        let off = profile_for_web(&[u("создай файл")], false, false);
        assert!(!off.names.contains(&"web_search".to_string()), "без переключателя поиска нет");
    }

    #[test]
    fn task_sticks_and_escalation_works() {
        let c = vec![u("создай файл a.txt"), json!({"role": "assistant", "content": "ок"}), u("а как дела?")];
        assert!(profile_for(&c, false).task, "раз задача, дальше задача");
        assert!(profile_for(&[u("привет")], true).task, "need_tools включает инструменты");
        let used = vec![u("привет"), json!({"role": "assistant", "content": "", "tool_calls": [{"id": "1", "type": "function", "function": {"name": "browser_open", "arguments": "{}"}}]})];
        let p = profile_for(&used, false);
        assert!(p.task && p.names.contains(&"browser_open".to_string()), "использованный инструмент остаётся доступным");
    }

    #[test]
    fn compact_cuts_to_first_sentence_and_shrinks() {
        let full = crate::tools::schemas().to_string().len();
        let p = profile_for(&[u("создай файл")], false);
        let sel = select(&p, vec![]).to_string().len();
        assert!(sel * 2 < full, "задача без групп вдвое короче полного набора: {sel} из {full}");
        let t = compact(&json!({"function": {"name": "x", "description": "Первое предложение. Второе, длинное и лишнее."}}));
        assert_eq!(t["function"]["description"], "Первое предложение.");
    }

    #[test]
    fn keywords_translate_russian() {
        let k = keywords("хочу написать игру с боевой механикой");
        assert!(k.contains(&"game".to_string()) && k.contains(&"gameplay".to_string()));
        assert!(keywords("make a website please").contains(&"website".to_string()));
        assert!(!keywords("please help").contains(&"please".to_string()), "служебные слова отброшены");
    }

    fn gs() -> Vec<(String, String)> {
        let l = [
            ("start", "Guided project start: where are you? Sets up a new game project step by step"),
            ("brainstorm", "Generate ideas for a new game concept"),
            ("team-audio", "Audio team: coordinate sound design for the game mechanics and music"),
            ("team-combat", "Combat team: design the combat mechanics of the game"),
            ("playtest-report", "Summarize a playtest of the game"),
            ("bug-report", "File a structured bug report for the game"),
            ("bug-triage", "Triage open bugs of the game"),
            ("test-setup", "Set up the test framework for the game"),
            ("balance-check", "Check game balance numbers"),
            ("code-review", "Review code of the game"),
            ("perf-profile", "Profile game performance"),
            ("art-bible", "Create the art bible for the game"),
        ];
        l.iter().map(|(n, d)| (n.to_string(), d.to_string())).collect()
    }

    #[test]
    fn game_request_prefers_entry_skill() {
        let skills = gs();
        let r = score_skills(&skills, "хочу написать игру");
        assert_eq!(skills[r[0].0].0, "start", "для новой идеи точка входа важнее случайного совпадения «game»: {r:?}");
        assert!(r[0].1 >= 20, "достаточно уверенно, чтобы подключить автоматически");
        let top3: Vec<&str> = r.iter().take(3).map(|(i, _)| skills[*i].0.as_str()).collect();
        assert!(top3.contains(&"brainstorm"), "{top3:?}");
        assert!(!top3.contains(&"team-audio"), "{top3:?}");
    }

    #[test]
    fn bug_request_prefers_bug_skill_not_substring() {
        let skills = gs();
        let r = score_skills(&skills, "исправь ошибку в тесте");
        let top = &skills[r[0].0].0;
        assert!(top.starts_with("bug-") || top == "test-setup", "{top}");
        assert_ne!(top, "playtest-report", "test не должен совпадать с серединой слова playtest");
    }

    #[test]
    fn specific_topic_beats_generic_word() {
        let skills = gs();
        let r = score_skills(&skills, "проверь баланс цифр");
        assert_eq!(skills[r[0].0].0, "balance-check");
        assert!(score_skills(&skills, "привет как дела").is_empty());
        assert!(score_skills(&[], "хочу игру").is_empty());
    }

    #[test]
    fn text_of_ignores_attached_note() {
        let m = json!({"role": "user", "content": format!("напиши игру\n\n{MARK}\nПодключён навык «x»: браузер реестр")});
        assert_eq!(text_of(&m), "напиши игру");
        let c = vec![m, json!({"role": "assistant", "content": "ок"})];
        let p = profile_for(&c, false);
        assert!(!p.names.contains(&"browser_open".to_string()), "слова из подклеенного навыка не включают группы");
    }
}
