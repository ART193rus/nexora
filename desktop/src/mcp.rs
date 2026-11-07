//! Клиент MCP (Model Context Protocol) по stdio: внешние серверы инструментов для агента.
//! Сервер MCP это отдельная программа, запускаемая с правами пользователя, поэтому: список серверов задаёт только
//! пользователь (вставка JSON с подтверждением), значения env (ключи) лежат в зашифрованном хранилище и в интерфейс
//! не возвращаются, первый вызов каждого инструмента за сеанс спрашивает подтверждение (см. agent.rs).
use crate::secure::Vault;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const MAX_TOOLS: usize = 40;

fn yes() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct ServerCfg {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Имена переменных окружения; сами значения хранятся в зашифрованном хранилище.
    #[serde(default)]
    pub env_keys: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

pub struct McpTool {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

pub struct Client {
    pub name: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, Sender<Value>>>>,
    next: AtomicU64,
    pub tools: Mutex<Vec<McpTool>>,
    pub stderr_tail: Arc<Mutex<String>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn spawn(cfg: &ServerCfg, env: &HashMap<String, String>) -> Result<Child, String> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    let mk = |cmd: &str, args: &[String]| {
        let mut c = Command::new(cmd);
        c.args(args).envs(env).current_dir(&home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c
    };
    match mk(&cfg.command, &cfg.args).spawn() {
        Ok(ch) => Ok(ch),
        // npx, uvx и подобные на Windows это .cmd-файлы: запускаем через cmd
        Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::NotFound => {
            let mut a = vec!["/c".to_string(), cfg.command.clone()];
            a.extend(cfg.args.iter().cloned());
            mk("cmd", &a).spawn().map_err(|e| format!("не удалось запустить «{}»: {e}", cfg.command))
        }
        Err(e) => Err(format!("не удалось запустить «{}»: {e}", cfg.command)),
    }
}

impl Client {
    pub fn start(cfg: &ServerCfg, env: &HashMap<String, String>) -> Result<Arc<Client>, String> {
        let mut child = spawn(cfg, env)?;
        let stdin = child.stdin.take().ok_or("нет stdin")?;
        let stdout = child.stdout.take().ok_or("нет stdout")?;
        let stderr = child.stderr.take().ok_or("нет stderr")?;
        let pending: Arc<Mutex<HashMap<u64, Sender<Value>>>> = Arc::new(Mutex::new(HashMap::new()));
        let tail = Arc::new(Mutex::new(String::new()));
        let t2 = tail.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut t = t2.lock().unwrap();
                t.push_str(&line);
                t.push('\n');
                if t.len() > 4000 {
                    let cut = t.len() - 3000;
                    let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                    *t = t[cut..].to_string();
                }
            }
        });
        let client = Arc::new(Client { name: cfg.name.clone(), child: Mutex::new(child), stdin: Mutex::new(stdin), pending: pending.clone(), next: AtomicU64::new(1), tools: Mutex::new(vec![]), stderr_tail: tail });
        // читатель ответов; запросы сервера к клиенту (ping и прочее) отклоняем или подтверждаем сами
        let weak = Arc::downgrade(&client);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
                let id = m.get("id").and_then(|x| x.as_u64());
                if m.get("method").is_some() {
                    if let (Some(id), Some(c)) = (m.get("id").cloned(), weak.upgrade()) {
                        let reply = if m["method"] == "ping" { json!({"jsonrpc": "2.0", "id": id, "result": {}}) } else { json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported"}}) };
                        let _ = c.write_line(&reply);
                    }
                    continue;
                }
                if let Some(id) = id {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(m);
                    }
                }
            }
            // сервер завершился: будим всех ожидающих
            for (_, tx) in pending.lock().unwrap().drain() {
                let _ = tx.send(json!({"error": {"message": "MCP-сервер завершился"}}));
            }
        });
        let init = client.request(
            "initialize",
            json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "Nexora", "version": env!("CARGO_PKG_VERSION")}}),
            Duration::from_secs(60),
        );
        if let Err(e) = init {
            let tail = client.stderr_tail.lock().unwrap().trim().to_string();
            return Err(if tail.is_empty() { e } else { format!("{e}. Вывод сервера: {}", tail.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect::<String>()) });
        }
        client.write_line(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        let mut tools = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = client.request("tools/list", params, Duration::from_secs(30))?;
            for t in r["tools"].as_array().cloned().unwrap_or_default() {
                if let Some(n) = t["name"].as_str() {
                    tools.push(McpTool { name: n.to_string(), description: t["description"].as_str().unwrap_or("").to_string(), schema: t["inputSchema"].clone() });
                }
            }
            cursor = r["nextCursor"].as_str().map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        *client.tools.lock().unwrap() = tools;
        Ok(client)
    }

    fn write_line(&self, v: &Value) -> Result<(), String> {
        let mut s = self.stdin.lock().unwrap();
        s.write_all(format!("{v}\n").as_bytes()).and_then(|_| s.flush()).map_err(|e| format!("запись в MCP-сервер: {e}"))
    }

    /// Запрос JSON-RPC; возвращает поле result или текст ошибки.
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.write_line(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(m) => match m.get("error") {
                Some(e) => Err(e["message"].as_str().unwrap_or("ошибка MCP-сервера").to_string()),
                None => Ok(m["result"].clone()),
            },
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("MCP-сервер не ответил за {} с", timeout.as_secs()))
            }
        }
    }

    pub fn call_tool(&self, tool: &str, args: &Value, timeout_secs: u64) -> Result<String, String> {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}), Duration::from_secs(timeout_secs))?;
        let mut out = String::new();
        for b in r["content"].as_array().cloned().unwrap_or_default() {
            match b["type"].as_str() {
                Some("text") => {
                    out.push_str(b["text"].as_str().unwrap_or(""));
                    out.push('\n');
                }
                Some("image") => out.push_str("[изображение, не показано]\n"),
                Some("resource") => out.push_str("[ресурс, не показан]\n"),
                _ => {}
            }
        }
        let out: String = out.trim().chars().take(12_000).collect();
        if r["isError"].as_bool() == Some(true) {
            Err(if out.is_empty() { "инструмент вернул ошибку".into() } else { out })
        } else {
            Ok(if out.is_empty() { "(пустой ответ)".into() } else { out })
        }
    }
}

// ---------- имена ----------
pub fn clean_ident(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// Имя функции для модели: mcp__сервер__инструмент (допустимые символы, не длиннее 64).
pub fn mangled(server: &str, tool: &str) -> String {
    let mut n = format!("mcp__{}__{}", clean_ident(server), clean_ident(tool));
    n.truncate(64);
    n
}

// ---------- настройки ----------
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn cfg_path() -> std::path::PathBuf {
    crate::engine::data_dir().join("mcp.json")
}

fn vault() -> Result<Vault, String> {
    Vault::open(&crate::engine::data_dir().join("vault"), None)
}

pub fn load() -> Vec<ServerCfg> {
    let _g = FILE_LOCK.lock().unwrap();
    std::fs::read_to_string(cfg_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(v: &[ServerCfg]) -> Result<(), String> {
    let _g = FILE_LOCK.lock().unwrap();
    let p = cfg_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn validate(c: &ServerCfg) -> Result<(), String> {
    if c.name.is_empty() || c.name.len() > 30 || !c.name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') {
        return Err("имя сервера: латиница в нижнем регистре, цифры, - и _ (до 30 символов)".into());
    }
    if c.command.trim().is_empty() || c.command.len() > 300 || c.command.chars().any(|ch| ch.is_control()) {
        return Err("не указана команда запуска".into());
    }
    if c.args.len() > 50 || c.args.iter().any(|a| a.len() > 1000 || a.chars().any(|ch| ch == '\0')) {
        return Err("слишком много или слишком длинные аргументы".into());
    }
    if c.env_keys.iter().any(|k| k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
        return Err("имена переменных окружения: латиница, цифры и _".into());
    }
    Ok(())
}

/// Разбор вставленного JSON из инструкции сервера: `{"mcpServers": {...}}` или просто `{имя: {command, args, env}}`.
pub fn parse_import(text: &str) -> Result<Vec<(ServerCfg, HashMap<String, String>)>, String> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("это не JSON: {e}"))?;
    let map = v.get("mcpServers").or_else(|| v.get("servers")).unwrap_or(&v).as_object().ok_or("ожидается объект с серверами")?;
    if map.is_empty() || map.len() > 20 {
        return Err("серверов должно быть от 1 до 20".into());
    }
    let mut out = vec![];
    for (name, e) in map {
        if e.get("url").is_some() && e.get("command").is_none() {
            return Err(format!("«{name}»: удалённые MCP-серверы (url) не поддерживаются, только локальные (command)"));
        }
        let command = e["command"].as_str().ok_or_else(|| format!("«{name}»: нет поля command"))?.to_string();
        let args: Vec<String> = e["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let env: HashMap<String, String> = e["env"].as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default();
        let mut keys: Vec<String> = env.keys().cloned().collect();
        keys.sort();
        let cfg = ServerCfg { name: name.to_lowercase().replace([' ', '.'], "-"), command, args, env_keys: keys, enabled: true };
        validate(&cfg)?;
        out.push((cfg, env));
    }
    Ok(out)
}

fn get_env(name: &str) -> HashMap<String, String> {
    vault().ok().and_then(|v| v.get(&format!("mcp_env_{name}")).ok().flatten()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

// ---------- запущенные серверы ----------
static CLIENTS: LazyLock<Mutex<Vec<Arc<Client>>>> = LazyLock::new(|| Mutex::new(vec![]));
static STATUS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn set_status(name: &str, s: &str) {
    STATUS.lock().unwrap().insert(name.to_string(), s.to_string());
}

/// Запустить (перезапустить) сервер; блокирует до готовности. Результат виден в статусе.
pub fn start_one(name: &str) {
    stop(name);
    let Some(cfg) = load().into_iter().find(|c| c.name == name) else { return };
    if !cfg.enabled {
        set_status(name, "выключен");
        return;
    }
    set_status(name, "запускается…");
    match Client::start(&cfg, &get_env(name)) {
        Ok(c) => {
            let n = c.tools.lock().unwrap().len();
            CLIENTS.lock().unwrap().push(c);
            set_status(name, &format!("готов, инструментов: {n}"));
        }
        Err(e) => set_status(name, &format!("ошибка: {}", e.chars().take(300).collect::<String>())),
    }
}

pub fn start_enabled() {
    for c in load().into_iter().filter(|c| c.enabled) {
        let n = c.name.clone();
        std::thread::spawn(move || start_one(&n));
    }
}

pub fn stop(name: &str) {
    CLIENTS.lock().unwrap().retain(|c| c.name != name); // Drop завершает процесс
}

pub fn stop_all() {
    CLIENTS.lock().unwrap().clear();
}

/// Собрать настройки сервера из аргументов инструмента агента (name, command, args, env).
pub fn cfg_from_args(a: &Value) -> Result<(ServerCfg, HashMap<String, String>), String> {
    let name = a["name"].as_str().unwrap_or("").trim().to_lowercase().replace([' ', '.'], "-");
    let command = a["command"].as_str().unwrap_or("").trim().to_string();
    let args: Vec<String> = a["args"].as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let env: HashMap<String, String> = a["env"].as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default();
    let mut keys: Vec<String> = env.keys().cloned().collect();
    keys.sort();
    let cfg = ServerCfg { name, command, args, env_keys: keys, enabled: true };
    validate(&cfg)?;
    Ok((cfg, env))
}

/// Добавить сервер и дождаться запуска (до ~90 с); возвращает итоговый статус. Для агента: инструменты готовы сразу.
pub fn add_wait(cfg: ServerCfg, env: HashMap<String, String>) -> Result<String, String> {
    store(&cfg, env)?;
    start_one(&cfg.name);
    Ok(STATUS.lock().unwrap().get(&cfg.name).cloned().unwrap_or_default())
}

fn store(cfg: &ServerCfg, env: HashMap<String, String>) -> Result<(), String> {
    validate(cfg)?;
    if !env.is_empty() {
        vault()?.put(&format!("mcp_env_{}", cfg.name), serde_json::to_string(&env).map_err(|e| e.to_string())?.as_bytes())?;
    }
    let mut all = load();
    match all.iter_mut().find(|c| c.name == cfg.name) {
        Some(x) => *x = cfg.clone(),
        None => all.push(cfg.clone()),
    }
    save(&all)
}

pub fn exists(name: &str) -> bool {
    load().iter().any(|c| c.name == name)
}

pub fn add(cfg: ServerCfg, env: HashMap<String, String>) -> Result<(), String> {
    store(&cfg, env)?;
    let n = cfg.name.clone();
    std::thread::spawn(move || start_one(&n));
    Ok(())
}

pub fn set_enabled(name: &str, on: bool) -> Result<(), String> {
    let mut all = load();
    all.iter_mut().find(|c| c.name == name).ok_or("такого сервера нет")?.enabled = on;
    save(&all)?;
    let n = name.to_string();
    if on {
        std::thread::spawn(move || start_one(&n));
    } else {
        stop(&n);
        set_status(&n, "выключен");
    }
    Ok(())
}

pub fn remove(name: &str) -> Result<(), String> {
    stop(name);
    let mut all = load();
    all.retain(|c| c.name != name);
    save(&all)?;
    if let Ok(v) = vault() {
        let _ = v.delete(&format!("mcp_env_{name}"));
    }
    STATUS.lock().unwrap().remove(name);
    Ok(())
}

pub fn restart(name: &str) {
    let n = name.to_string();
    std::thread::spawn(move || start_one(&n));
}

/// Для страницы настроек: серверы со статусом и списком инструментов (значения env не возвращаются).
pub fn status() -> Vec<Value> {
    let st = STATUS.lock().unwrap().clone();
    let clients = CLIENTS.lock().unwrap();
    load()
        .into_iter()
        .map(|c| {
            let tools: Vec<String> = clients.iter().find(|x| x.name == c.name).map(|x| x.tools.lock().unwrap().iter().map(|t| t.name.clone()).collect()).unwrap_or_default();
            let status = st.get(&c.name).cloned().unwrap_or_else(|| if c.enabled { "ожидает запуска".into() } else { "выключен".into() });
            json!({"name": c.name, "command": c.command, "args": c.args, "env_keys": c.env_keys, "enabled": c.enabled, "status": status, "tools": tools})
        })
        .collect()
}

/// Описания инструментов запущенных серверов для модели (не больше MAX_TOOLS).
pub fn schemas() -> Vec<Value> {
    let mut out = vec![];
    for c in CLIENTS.lock().unwrap().iter() {
        for t in c.tools.lock().unwrap().iter() {
            if out.len() >= MAX_TOOLS {
                return out;
            }
            let mut params = t.schema.clone();
            if !params.is_object() {
                params = json!({"type": "object", "properties": {}});
            }
            if params.get("type").is_none() {
                params["type"] = json!("object");
            }
            let desc: String = format!("[MCP {}] {}", c.name, t.description).chars().take(400).collect();
            out.push(json!({"type": "function", "function": {"name": mangled(&c.name, &t.name), "description": desc, "parameters": params}}));
        }
    }
    out
}

/// Найти клиента и исходное имя инструмента по имени, которое вызвала модель.
pub fn resolve(mangled_name: &str) -> Option<(Arc<Client>, String)> {
    for c in CLIENTS.lock().unwrap().iter() {
        for t in c.tools.lock().unwrap().iter() {
            if mangled(&c.name, &t.name) == mangled_name {
                return Some((c.clone(), t.name.clone()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = r#"
import sys, json
def out(o):
    print(json.dumps(o), flush=True)
for line in sys.stdin:
    m = json.loads(line)
    mid = m.get("id"); meth = m.get("method")
    if meth == "initialize":
        out({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}})
    elif meth == "tools/list":
        out({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}})
    elif meth == "tools/call":
        t = m["params"]["arguments"].get("text","")
        if t == "boom":
            out({"jsonrpc":"2.0","id":mid,"result":{"isError":True,"content":[{"type":"text","text":"bad"}]}})
        elif t == "silent":
            pass
        else:
            out({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo: "+t},{"type":"image","data":"x"}]}})
"#;

    fn python_ok() -> bool {
        Command::new("python").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    #[test]
    fn names() {
        assert_eq!(mangled("fetch", "get page"), "mcp__fetch__get_page");
        assert_eq!(mangled("a", "тест"), "mcp__a______");
        assert!(mangled("s", &"x".repeat(100)).len() <= 64);
    }

    #[test]
    fn import_forms() {
        let a = parse_import(r#"{"mcpServers":{"Fetch":{"command":"uvx","args":["mcp-server-fetch"]},"db":{"command":"node","args":["s.js"],"env":{"API_KEY":"secret123","B":"1"}}}}"#).unwrap();
        assert_eq!(a.len(), 2);
        let db = a.iter().find(|x| x.0.name == "db").unwrap();
        assert_eq!(db.0.env_keys, vec!["API_KEY", "B"]);
        assert_eq!(db.1["API_KEY"], "secret123");
        assert!(a.iter().any(|x| x.0.name == "fetch"), "имя приводится к нижнему регистру");
        let b = parse_import(r#"{"x":{"command":"npx","args":["-y","pkg"]}}"#).unwrap();
        assert_eq!(b[0].0.command, "npx");
        assert!(parse_import("не json").is_err());
        assert!(parse_import(r#"{"mcpServers":{"r":{"url":"https://x"}}}"#).unwrap_err().contains("удалённые"));
        assert!(parse_import(r#"{"mcpServers":{}}"#).is_err());
        assert!(parse_import(r#"{"mcpServers":{"bad!name":{"command":"x"}}}"#).is_err(), "недопустимые символы в имени");
        assert!(parse_import(r#"{"mcpServers":{"s":{"command":"x","env":{"BAD-KEY":"1"}}}}"#).is_err(), "имя переменной окружения проверяется");
    }

    #[test]
    fn protocol_with_fake_server() {
        if !python_ok() {
            eprintln!("python не найден: тест пропущен");
            return;
        }
        let cfg = ServerCfg { name: "fake".into(), command: "python".into(), args: vec!["-u".into(), "-c".into(), FAKE.into()], env_keys: vec![], enabled: true };
        let env: HashMap<String, String> = [("PYTHONUTF8".to_string(), "1".to_string())].into();
        let c = Client::start(&cfg, &env).expect("handshake");
        assert_eq!(c.tools.lock().unwrap().len(), 1);
        assert_eq!(c.call_tool("echo", &json!({"text": "привет"}), 10).unwrap(), "echo: привет\n[изображение, не показано]");
        assert_eq!(c.call_tool("echo", &json!({"text": "boom"}), 10).unwrap_err(), "bad");
        let e = c.call_tool("echo", &json!({"text": "silent"}), 1).unwrap_err();
        assert!(e.contains("не ответил"), "{e}");
        // после таймаута клиент жив
        assert!(c.call_tool("echo", &json!({"text": "again"}), 10).unwrap().starts_with("echo: again"));
        CLIENTS.lock().unwrap().push(c);
        let s = schemas();
        assert!(s.iter().any(|x| x["function"]["name"] == "mcp__fake__echo" && x["function"]["parameters"]["required"][0] == "text"), "{s:?}");
        let (cl, tool) = resolve("mcp__fake__echo").unwrap();
        assert_eq!((cl.name.as_str(), tool.as_str()), ("fake", "echo"));
        assert!(resolve("mcp__fake__nope").is_none());
        stop("fake");
        assert!(resolve("mcp__fake__echo").is_none());
    }

    #[test]
    fn agent_args() {
        let (c, env) = cfg_from_args(&json!({"name": "Web Fetch", "command": "uvx", "args": ["mcp-server-fetch"], "env": {"K": "v"}})).unwrap();
        assert_eq!((c.name.as_str(), c.command.as_str(), c.env_keys.clone()), ("web-fetch", "uvx", vec!["K".to_string()]));
        assert_eq!(env["K"], "v");
        assert!(cfg_from_args(&json!({"name": "x"})).is_err(), "нужна команда");
        assert!(cfg_from_args(&json!({"name": "", "command": "a"})).is_err(), "нужно имя");
        assert!(cfg_from_args(&json!({"name": "a;b", "command": "a"})).is_err());
        assert!(cfg_from_args(&json!({"name": "a", "command": "a", "env": {"BAD-KEY": "1"}})).is_err());
    }

    #[test]
    fn broken_server_reports_error() {
        let cfg = ServerCfg { name: "bad".into(), command: "definitely-not-a-real-program-xyz".into(), args: vec![], env_keys: vec![], enabled: true };
        assert!(Client::start(&cfg, &HashMap::new()).is_err());
    }
}
