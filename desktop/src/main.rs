#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod agent;
mod agentclient;
mod browser;
mod catalog;
mod chat;
mod chats;
mod desktop;
mod discover;
mod download;
mod engine;
mod hf;
mod hostmode;
mod hw;
mod mcp;
mod memory;
mod overlay;
mod pki;
mod plugins;
mod prefs;
mod registry;
mod router;
mod scan;
mod selfdev;
mod secure;
mod tasks;
mod mobile;
mod server;
mod ssh;
mod ollama;
mod tools;
mod totp;
mod update;
mod web;

use agent::{Approvals, Ctx, Mode};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, RunEvent, State};

const MODEL: &str = "qwen3-coder";

/// Размер контекста, заданный вручную (0 = автоматически по объёму памяти).
static CTX_SETTING: AtomicU32 = AtomicU32::new(0);

fn effective_ctx() -> u32 {
    match CTX_SETTING.load(Ordering::SeqCst) {
        0 => catalog::default_ctx(hw::get()).min(32768),
        v => v,
    }
}

struct App {
    engine: Mutex<Option<engine::Engine>>,
    ready: AtomicBool,
    busy: AtomicBool,
    cancel: Arc<AtomicBool>,
    conv: Mutex<Vec<Value>>,
    workspace: Mutex<Option<PathBuf>>,
    mode: Mutex<Mode>,
    approvals: Arc<Approvals>,
    auto_folder: AtomicBool,
    ws_auto: AtomicBool,
    model: Mutex<String>,
    gen: AtomicU64,
    source: Mutex<String>,
    ext_url: Mutex<String>,
    profile: Mutex<String>,
    apps: Arc<Mutex<Vec<tools::AppProc>>>,
    auto_mode: AtomicBool,
    role: Mutex<Option<String>>,
    downloads: Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
    projects_root: Mutex<PathBuf>,
    remotes: Mutex<Vec<ssh::Remote>>,
    remote: Mutex<Option<String>>,
    grants: Arc<Mutex<Vec<PathBuf>>>,
    chat_trust: Arc<AtomicBool>,
    scripts_ok: Arc<Mutex<Vec<String>>>,
    chat_id: Mutex<Option<u64>>,
    agent_proxy: Mutex<Option<agentclient::Proxy>>,
    agent_pair: Mutex<Option<(agentclient::ServerRef, agentclient::PairSession)>>,
    hub: Arc<mobile::Hub>,
    host: hostmode::Host,
    handle: Mutex<Option<tauri::AppHandle>>,
}

impl App {
    fn new() -> App {
        let cfg = load_cfg();
        let approvals = Arc::new(Approvals::default());
        CTX_SETTING.store(cfg["ctx"].as_u64().unwrap_or(0) as u32, Ordering::SeqCst);
        App {
            engine: Mutex::new(None),
            ready: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            conv: Mutex::new(vec![]),
            workspace: Mutex::new(cfg["workspace"].as_str().map(PathBuf::from).filter(|p| p.is_dir())),
            mode: Mutex::new(Mode::parse(cfg["mode"].as_str().unwrap_or(""))),
            approvals: approvals.clone(),
            auto_folder: AtomicBool::new(cfg["auto_folder"].as_bool().unwrap_or(true)),
            ws_auto: AtomicBool::new(false),
            model: Mutex::new(cfg["model"].as_str().unwrap_or(MODEL).to_string()),
            gen: AtomicU64::new(0),
            source: Mutex::new(cfg["source"].as_str().unwrap_or("builtin").to_string()),
            ext_url: Mutex::new(cfg["ext_url"].as_str().unwrap_or("").to_string()),
            apps: Arc::new(Mutex::new(vec![])),
            auto_mode: AtomicBool::new(cfg["auto"].as_bool().unwrap_or(false)),
            role: Mutex::new(None),
            downloads: Mutex::new(std::collections::HashMap::new()),
            profile: Mutex::new(cfg["profile"].as_str().unwrap_or("local").to_string()),
            projects_root: Mutex::new(cfg["projects_root"].as_str().map(PathBuf::from).unwrap_or_else(default_projects_root)),
            remotes: Mutex::new(serde_json::from_value(cfg["remotes"].clone()).unwrap_or_default()),
            remote: Mutex::new(cfg["remote"].as_str().map(String::from)),
            grants: Arc::new(Mutex::new(vec![])),
            chat_trust: Arc::new(AtomicBool::new(false)),
            scripts_ok: Arc::new(Mutex::new(vec![])),
            chat_id: Mutex::new(None),
            agent_proxy: Mutex::new(None),
            agent_pair: Mutex::new(None),
            hub: mobile::Hub::new(approvals),
            host: hostmode::Host::default(),
            handle: Mutex::new(None),
        }
    }
}

fn default_projects_root() -> PathBuf {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Documents").join("Nexora")
}

/// Создаёт следующую свободную папку «Проект N» в папке проектов.
fn create_project(app: &App) -> Result<PathBuf, String> {
    let root = app.projects_root.lock().unwrap().clone();
    std::fs::create_dir_all(&root).map_err(|e| format!("не удалось создать папку проектов: {e}"))?;
    for n in 1..10_000 {
        let d = root.join(format!("Проект {n}"));
        if !d.exists() {
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            return Ok(d);
        }
    }
    Err("слишком много проектов".into())
}

fn cfg_path() -> PathBuf {
    engine::data_dir().join("config.json")
}
fn load_cfg() -> Value {
    std::fs::read_to_string(cfg_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}
fn save_cfg(app: &App) {
    let ws = app.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string());
    let mode = app.mode.lock().unwrap().as_str();
    let _ = std::fs::write(cfg_path(), json!({"auto": app.auto_mode.load(Ordering::SeqCst), "ctx": CTX_SETTING.load(Ordering::SeqCst), "profile": app.profile.lock().unwrap().clone(), "source": app.source.lock().unwrap().clone(), "ext_url": app.ext_url.lock().unwrap().clone(), "model": app.model.lock().unwrap().clone(), "workspace": ws, "mode": mode, "auto_folder": app.auto_folder.load(Ordering::SeqCst), "projects_root": app.projects_root.lock().unwrap().display().to_string(), "remotes": app.remotes.lock().unwrap().clone(), "remote": app.remote.lock().unwrap().clone()}).to_string());
}

fn settings(model: &str) -> Result<engine::Settings, String> {
    if let Some(id) = model.strip_prefix("nx:") {
        let m = catalog::load().into_iter().find(|m| m.id == id).ok_or("такой модели нет в каталоге")?;
        let path = download::model_path(&m);
        if !path.exists() {
            return Err("Модель ещё не скачана".into());
        }
        let mm = if m.mmproj.is_empty() { None } else { Some(download::mmproj_path(&m)).filter(|p| p.exists()) };
        return Ok(engine::Settings { server_exe: engine::find_server_exe()?, model: path, alias: id.into(), ctx: effective_ctx(), mmproj: mm, kv_mb_per_1k: Some(m.kv_mb_per_1k) });
    }
    Ok(engine::Settings {
        server_exe: engine::find_server_exe()?,
        model: ollama::find_gguf(model)?,
        alias: model.into(),
        ctx: effective_ctx(),
        mmproj: None,
        kv_mb_per_1k: {
            let base = model.split(':').next().unwrap_or("").to_string();
            catalog::load().iter().find(|c| !base.is_empty() && c.id.starts_with(&base)).map(|c| c.kv_mb_per_1k)
        },
    })
}

#[tauri::command]
fn get_state(st: State<'_, Arc<App>>) -> Value {
    json!({
        "workspace": st.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string()),
        "mode": st.mode.lock().unwrap().as_str(),
        "auto": st.auto_mode.load(Ordering::SeqCst),
        "ctx": CTX_SETTING.load(Ordering::SeqCst),
        "profile": st.profile.lock().unwrap().clone(),
        "source": st.source.lock().unwrap().clone(),
        "ext_url": st.ext_url.lock().unwrap().clone(),
        "model": st.model.lock().unwrap().clone(),
        "remote": st.remote.lock().unwrap().clone(),
        "auto_folder": st.auto_folder.load(Ordering::SeqCst),
        "projects_root": st.projects_root.lock().unwrap().display().to_string(),
    })
}

#[tauri::command]
fn set_workspace(st: State<'_, Arc<App>>, path: String) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_dir() {
        return Err("Такой папки нет".into());
    }
    let canon = p.canonicalize().map_err(|e| e.to_string())?;
    let shown = canon.display().to_string().trim_start_matches(r"\\?\").to_string();
    *st.workspace.lock().unwrap() = Some(PathBuf::from(&shown));
    st.conv.lock().unwrap().clear();
    *st.remote.lock().unwrap() = None;
    st.ws_auto.store(false, Ordering::SeqCst);
    save_cfg(&st);
    Ok(shown)
}

/// Создаёт новую папку проекта в папке проектов и открывает её.
#[tauri::command]
fn new_project(st: State<'_, Arc<App>>) -> Result<String, String> {
    let p = create_project(&st)?;
    *st.workspace.lock().unwrap() = Some(p.clone());
    st.conv.lock().unwrap().clear();
    *st.remote.lock().unwrap() = None;
    st.ws_auto.store(true, Ordering::SeqCst);
    save_cfg(&st);
    Ok(p.display().to_string())
}

/// Название программы зависит от режима: локально, сервер или агент (клиент сервера).
fn title_for(profile: &str) -> &'static str {
    match profile {
        "server" => "Nexora Server",
        "agent" => "Nexora Agent",
        _ => "Nexora",
    }
}

#[tauri::command]
fn set_profile(app: tauri::AppHandle, st: State<'_, Arc<App>>, profile: String) -> String {
    let p = match profile.as_str() {
        "server" => "server",
        "agent" => "agent",
        _ => "local",
    };
    *st.profile.lock().unwrap() = p.to_string();
    save_cfg(&st);
    let t = title_for(p);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_title(t);
    }
    t.to_string()
}

#[tauri::command]
fn set_auto_folder(st: State<'_, Arc<App>>, on: bool) {
    st.auto_folder.store(on, Ordering::SeqCst);
    save_cfg(&st);
}

#[tauri::command]
fn set_projects_root(st: State<'_, Arc<App>>, path: String) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    *st.projects_root.lock().unwrap() = p.clone();
    save_cfg(&st);
    Ok(p.display().to_string())
}

#[tauri::command]
fn set_mode(st: State<'_, Arc<App>>, mode: String) {
    *st.mode.lock().unwrap() = Mode::parse(&mode);
    save_cfg(&st);
}

#[tauri::command]
fn fs_list(st: State<'_, Arc<App>>, rel: String) -> Result<Vec<Value>, String> {
    let root = st.workspace.lock().unwrap().clone().ok_or("Папка не выбрана")?;
    let text = tools::list_dir(&root, &json!({"path": rel}))?;
    Ok(text
        .lines()
        .filter(|l| *l != "(пусто)")
        .map(|l| json!({"name": l.trim_end_matches('/'), "dir": l.ends_with('/')}))
        .collect())
}

#[tauri::command]
fn approve(st: State<'_, Arc<App>>, id: u64, ok: bool) {
    st.approvals.resolve(id, ok);
}

#[tauri::command]
fn chat_reset(st: State<'_, Arc<App>>) {
    st.chat_trust.store(false, Ordering::SeqCst);
    st.conv.lock().unwrap().clear();
}

/// Запускает движок и в фоне ждёт готовности; статус уходит в интерфейс событием "engine".
#[tauri::command]
fn engine_start(app: tauri::AppHandle, st: State<'_, Arc<App>>) -> Result<(), String> {
    start_engine(app, st.inner().clone())
}

/// Каталог моделей с оценкой под это железо и состоянием загрузки.
#[tauri::command]
fn get_catalog() -> Value {
    let h = hw::get();
    let ctx = catalog::default_ctx(h).min(32768);
    let items: Vec<Value> = catalog::estimate_all(h, ctx)
        .into_iter()
        .map(|(m, e)| {
            let have = download::is_ready(&m);
            json!({"model": m, "est": e, "downloaded": have})
        })
        .collect();
    let best = catalog::estimate_all(h, ctx);
    let rec = catalog::pick(&best, 6.0).map(|(m, _)| m.id.clone());
    json!({"hw": h, "ctx": ctx, "items": items, "recommended": rec, "ram_used_gb": hw::ram_used_gb()})
}

#[tauri::command]
fn model_download(app: tauri::AppHandle, st: State<'_, Arc<App>>, id: String) -> Result<(), String> {
    let m = catalog::load().into_iter().find(|m| m.id == id).ok_or("такой модели нет в каталоге")?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut g = st.downloads.lock().unwrap();
        if g.contains_key(&id) {
            return Err("Эта модель уже скачивается".into());
        }
        g.insert(id.clone(), cancel.clone());
    }
    let st2: Arc<App> = st.inner().clone();
    std::thread::spawn(move || {
        let emit = |state: &str, done: u64, total: u64, speed: f64, msg: &str| {
            let _ = app.emit("download", json!({"id": id, "state": state, "done": done, "total": total, "speed": speed, "msg": msg}));
        };
        emit("running", 0, 0, 0.0, "модель");
        let r = download::fetch_model(&m, &cancel, |phase, d, t, sp| emit("running", d, t, sp, phase));
        st2.downloads.lock().unwrap().remove(&id);
        match r {
            Ok(()) => emit("done", 0, 0, 0.0, ""),
            Err(e) => emit(if e == "Отменено" { "cancelled" } else { "error" }, 0, 0, 0.0, &e),
        }
    });
    Ok(())
}

/// Размер контекста: 0 = автоматически. Встроенный движок перезапускается с новым значением.
#[tauri::command]
fn set_ctx(app: tauri::AppHandle, st: State<'_, Arc<App>>, ctx: u32) -> Result<u32, String> {
    if st.busy.load(Ordering::SeqCst) {
        return Err("Дождитесь окончания ответа или нажмите «Стоп»".into());
    }
    let v = if ctx == 0 { 0 } else { ctx.clamp(2048, 262_144) };
    CTX_SETTING.store(v, Ordering::SeqCst);
    save_cfg(&st);
    let own = st.engine.lock().unwrap().as_ref().map(|e| e.child.is_some()).unwrap_or(false);
    if own && st.source.lock().unwrap().as_str() != "external" {
        st.ready.store(false, Ordering::SeqCst);
        if let Some(mut e) = st.engine.lock().unwrap().take() {
            engine::stop(&mut e);
        }
        start_engine(app, st.inner().clone())?;
    }
    Ok(effective_ctx())
}

/// Сколько памяти займёт контекст для текущей модели и поместится ли всё в ОЗУ.
#[tauri::command]
fn ctx_info(st: State<'_, Arc<App>>, ctx: u32) -> Value {
    let h = hw::get();
    let eff = if ctx > 0 { ctx } else { catalog::default_ctx(h).min(32768) };
    let model = st.model.lock().unwrap().clone();
    let cat = catalog::load();
    let (size_gb, kv_mb) = if let Some(id) = model.strip_prefix("nx:") {
        cat.iter().find(|m| m.id == id).map(|m| (m.size_gb, m.kv_mb_per_1k)).unwrap_or((8.0, 50.0))
    } else {
        let base = model.split(':').next().unwrap_or("").to_string();
        let size = ollama::find_gguf(&model).ok().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() as f64 / 1e9).unwrap_or(8.0);
        let kv = cat.iter().find(|m| !base.is_empty() && m.id.starts_with(&base)).map(|m| m.kv_mb_per_1k).unwrap_or(50.0);
        (size, kv)
    };
    // кэш в каталоге посчитан для q8_0; движок берёт несжатый f16 (вдвое больше), когда памяти хватает: оценка по тому же правилу
    let kv_gb = kv_mb * (eff as f64 / 1024.0) / 1024.0 * engine::kv_factor(size_gb, eff, h.ram_gb, Some(kv_mb));
    let need = size_gb + kv_gb + 0.5;
    let budget = (h.ram_gb - 4.0).min(h.ram_gb * 0.85).max(0.0);
    json!({"ctx": eff, "auto": ctx == 0, "kv_gb": kv_gb, "need_gb": need, "ram_gb": h.ram_gb, "budget_gb": budget, "fits": need <= budget})
}

/// Сведения для окна «О системе».
#[tauri::command]
fn app_info() -> Value {
    json!({"version": env!("CARGO_PKG_VERSION"), "data_dir": engine::data_dir().display().to_string(), "hw": hw::get()})
}

// ---------- Hugging Face ----------
#[tauri::command]
async fn hf_search(q: String) -> Result<Vec<Value>, String> {
    blocking(move || hf::search(&q)).await
}

#[tauri::command]
async fn hf_files(repo: String) -> Result<Value, String> {
    blocking(move || hf::files(&repo)).await
}

/// Регистрирует модель с Hugging Face в списке (скачивание запускается отдельно, как у каталога).
#[tauri::command]
async fn hf_add(repo: String, file: String, mmproj: Option<String>, size_gb: f64) -> Result<String, String> {
    blocking(move || hf::add(&repo, &file, mmproj, size_gb)).await
}

#[tauri::command]
fn model_delete(st: State<'_, Arc<App>>, id: String) -> Result<(), String> {
    if st.model.lock().unwrap().as_str() == format!("nx:{id}") && st.engine.lock().unwrap().is_some() {
        return Err("Эта модель сейчас запущена: выберите другую и повторите".into());
    }
    if st.downloads.lock().unwrap().contains_key(&id) {
        return Err("Модель сейчас скачивается".into());
    }
    let m = catalog::load().into_iter().find(|m| m.id == id).ok_or("Такой модели нет")?;
    let p = download::model_path(&m);
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(format!("{}.part", p.display()));
    if !m.mmproj.is_empty() {
        let _ = std::fs::remove_file(download::mmproj_path(&m));
    }
    if let Some(d) = p.parent() {
        let _ = std::fs::remove_dir(d);
    }
    if m.custom {
        hf::remove(&id)?;
    }
    Ok(())
}

// ---------- плагины ----------
#[tauri::command]
async fn plugin_preview(source: String) -> Result<Value, String> {
    blocking(move || plugins::preview(&source)).await
}

#[tauri::command]
async fn plugin_install(app: tauri::AppHandle, source: String, sha: String, with_scripts: Option<bool>) -> Result<Value, String> {
    blocking(move || {
        let p = plugins::install(&source, &sha, with_scripts.unwrap_or(false), |done, total| {
            let _ = app.emit("plugin-progress", json!({"done": done, "total": total}));
        })?;
        serde_json::to_value(p).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn plugin_list() -> Value {
    serde_json::to_value(plugins::list()).unwrap_or(json!([]))
}

#[tauri::command]
fn plugin_toggle(st: State<'_, Arc<App>>, id: String, on: bool) -> Result<(), String> {
    st.conv.lock().unwrap().clear();
    plugins::set_enabled(&id, on)
}

#[tauri::command]
fn plugin_remove(st: State<'_, Arc<App>>, id: String) -> Result<(), String> {
    st.conv.lock().unwrap().clear();
    *st.role.lock().unwrap() = None;
    plugins::remove(&id)
}

#[tauri::command]
async fn plugin_search(q: String, topic: String) -> Result<Vec<Value>, String> {
    blocking(move || plugins::search(&q, &topic)).await
}

#[tauri::command]
fn agents_list() -> Vec<Value> {
    plugins::agents().into_iter().map(|(n, d, p)| json!({"name": n, "description": d, "plugin": p})).collect()
}

/// Роль агента для новых диалогов (None = без роли). Текущий диалог начинается заново.
#[tauri::command]
fn set_role(st: State<'_, Arc<App>>, name: Option<String>) {
    *st.role.lock().unwrap() = name.filter(|n| !n.is_empty());
    st.conv.lock().unwrap().clear();
}

#[tauri::command]
fn model_cancel(st: State<'_, Arc<App>>, id: String) {
    if let Some(c) = st.downloads.lock().unwrap().get(&id) {
        c.store(true, Ordering::SeqCst);
    }
}

/// [{id, label}]: модели Ollama и скачанные из каталога (nx:<id>) либо модели внешнего движка.
#[tauri::command]
fn list_models(st: State<'_, Arc<App>>) -> Vec<Value> {
    list_models_for(&st)
}

fn list_models_for(st: &App) -> Vec<Value> {
    let plain = |v: Vec<String>| v.into_iter().map(|n| json!({"id": n, "label": n})).collect::<Vec<_>>();
    if st.source.lock().unwrap().as_str() == "external" {
        let c = st.engine.lock().unwrap().as_ref().map(|e| (e.base.clone(), e.token.clone()));
        return plain(c.and_then(|(b, k)| engine::probe(&b, &k).ok()).unwrap_or_default());
    }
    let mut out = vec![];
    for m in catalog::load() {
        if download::is_ready(&m) {
            out.push(json!({"id": format!("nx:{}", m.id), "label": format!("{} (скачана)", m.name)}));
        }
    }
    out.extend(plain(ollama::list_models()));
    out
}

#[allow(dead_code)]
fn list_models_old(st: State<'_, Arc<App>>) -> Vec<String> {
    if st.source.lock().unwrap().as_str() == "external" {
        let c = st.engine.lock().unwrap().as_ref().map(|e| (e.base.clone(), e.token.clone()));
        return c.and_then(|(b, k)| engine::probe(&b, &k).ok()).unwrap_or_default();
    }
    ollama::list_models()
}

/// Подключение к внешнему движку (LM Studio, Ollama, любой OpenAI-совместимый сервер).
fn connect_external(app: &tauri::AppHandle, st: &Arc<App>, url: &str, key: &str, want: Option<&str>) -> Result<Value, String> {
    if st.busy.load(Ordering::SeqCst) {
        return Err("Дождитесь окончания ответа или нажмите «Стоп»".into());
    }
    let base = url.trim().trim_end_matches('/').trim_end_matches("/v1").to_string();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("Адрес должен начинаться с http:// или https://".into());
    }
    let models = engine::probe(&base, key).map_err(|e| format!("Движок не отвечает: {e}"))?;
    if models.is_empty() {
        return Err("На движке нет доступных моделей".into());
    }
    let saved = st.model.lock().unwrap().clone();
    let model = want
        .filter(|m| models.iter().any(|x| x == m))
        .map(String::from)
        .or_else(|| models.iter().find(|x| **x == saved).cloned())
        .unwrap_or_else(|| models[0].clone());
    st.ready.store(false, Ordering::SeqCst);
    st.gen.fetch_add(1, Ordering::SeqCst);
    if let Some(mut e) = st.engine.lock().unwrap().take() {
        engine::stop(&mut e);
    }
    *st.engine.lock().unwrap() = Some(engine::Engine { child: None, port: 0, base: base.clone(), token: key.to_string(), alias: model.clone(), vision: true });
    *st.source.lock().unwrap() = "external".into();
    *st.ext_url.lock().unwrap() = base.clone();
    *st.model.lock().unwrap() = model.clone();
    st.conv.lock().unwrap().clear();
    save_cfg(st);
    st.ready.store(true, Ordering::SeqCst);
    let kind = engine::kind(&base);
    let label = match kind {
        "ollama" => "Ollama",
        "lmstudio" => "LM Studio",
        _ => "внешний движок",
    };
    let _ = app.emit("engine", json!({"state": "ready", "msg": format!("{label} · {model}")}));
    let mut warn: Vec<&str> = vec![];
    if !engine::is_private_host(&base) {
        warn.push("Адрес не в вашем компьютере и не в частной сети: чужой сервер увидит ваш код и запросы.");
    } else if base.starts_with("http://") && !base.contains("127.0.0.1") && !base.contains("localhost") {
        warn.push("Соединение не шифруется (http): используйте только в доверенной домашней сети.");
    }
    if kind == "ollama" {
        warn.push("Ollama по умолчанию даёт модели контекст 4096 токенов, агенту этого мало. Задайте переменную OLLAMA_CONTEXT_LENGTH=16384 и перезапустите Ollama.");
    }
    Ok(json!({"kind": kind, "models": models, "model": model, "warn": warn}))
}

#[tauri::command]
async fn use_external(app: tauri::AppHandle, st: State<'_, Arc<App>>, url: String, key: String) -> Result<Value, String> {
    let a: Arc<App> = st.inner().clone();
    blocking(move || connect_external(&app, &a, &url, &key, None)).await
}

#[tauri::command]
fn use_builtin(app: tauri::AppHandle, st: State<'_, Arc<App>>) -> Result<(), String> {
    if st.busy.load(Ordering::SeqCst) {
        return Err("Дождитесь окончания ответа или нажмите «Стоп»".into());
    }
    st.ready.store(false, Ordering::SeqCst);
    st.gen.fetch_add(1, Ordering::SeqCst);
    if let Some(mut e) = st.engine.lock().unwrap().take() {
        engine::stop(&mut e);
    }
    *st.source.lock().unwrap() = "builtin".into();
    st.conv.lock().unwrap().clear();
    save_cfg(&st);
    start_engine(app, st.inner().clone())
}

/// Ищет LM Studio и Ollama на стандартных портах этого компьютера.
#[tauri::command]
async fn detect_engines() -> Vec<Value> {
    tauri::async_runtime::spawn_blocking(detect_engines_sync).await.unwrap_or_default()
}

fn detect_engines_sync() -> Vec<Value> {
    let mut out = vec![];
    for (name, base) in [("LM Studio", "http://127.0.0.1:1234"), ("Ollama", "http://127.0.0.1:11434")] {
        if let Ok(models) = engine::probe(base, "") {
            out.push(json!({"name": name, "url": base, "models": models.len()}));
        }
    }
    out
}

/// Смена модели: останавливает текущий движок и запускает новый с выбранной моделью.
#[tauri::command]
fn set_model(app: tauri::AppHandle, st: State<'_, Arc<App>>, name: String) -> Result<(), String> {
    set_model_for(app, st.inner(), name)
}

fn set_model_for(app: tauri::AppHandle, st: &Arc<App>, name: String) -> Result<(), String> {
    if st.busy.load(Ordering::SeqCst) {
        return Err("Дождитесь окончания ответа или нажмите «Стоп»".into());
    }
    if st.source.lock().unwrap().as_str() == "external" {
        if let Some(e) = st.engine.lock().unwrap().as_mut() {
            e.alias = name.clone();
        }
        *st.model.lock().unwrap() = name.clone();
        st.conv.lock().unwrap().clear();
        save_cfg(&st);
        let _ = app.emit("engine", json!({"state": "ready", "msg": format!("{name} · внешний движок")}));
        return Ok(());
    }
    if !name.starts_with("nx:") {
        ollama::find_gguf(&name)?;
    }
    st.ready.store(false, Ordering::SeqCst);
    if let Some(mut e) = st.engine.lock().unwrap().take() {
        engine::stop(&mut e);
    }
    *st.model.lock().unwrap() = name;
    st.conv.lock().unwrap().clear();
    save_cfg(&st);
    start_engine(app, st.clone())
}

fn start_engine(app: tauri::AppHandle, st: Arc<App>) -> Result<(), String> {
    if st.engine.lock().unwrap().is_some() {
        return Ok(());
    }
    if st.source.lock().unwrap().as_str() == "external" {
        let url = st.ext_url.lock().unwrap().clone();
        let m = st.model.lock().unwrap().clone();
        return connect_external(&app, &st, &url, "", Some(&m)).map(|_| ());
    }
    let model = st.model.lock().unwrap().clone();
    let s = settings(&model)?;
    let e = engine::start(&s)?;
    let port = e.port;
    *st.engine.lock().unwrap() = Some(e);
    let my_gen = st.gen.fetch_add(1, Ordering::SeqCst) + 1;
    let st2: Arc<App> = st.clone();
    std::thread::spawn(move || {
        let emit = |state: &str, msg: &str| {
            let _ = app.emit("engine", json!({"state": state, "msg": msg}));
        };
        emit("loading", "Загрузка модели в память…");
        let t0 = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(700));
            if st2.gen.load(Ordering::SeqCst) != my_gen {
                return; // запущен другой движок
            }
            {
                let mut g = st2.engine.lock().unwrap();
                match g.as_mut() {
                    None => return, // остановлен
                    Some(e) => {
                        if let Some(code) = e.exited() {
                            *g = None;
                            emit("error", &format!("Движок завершился ({code}). См. лог engine.log"));
                            return;
                        }
                    }
                }
            }
            if engine::health(port) {
                break;
            }
            if t0.elapsed().as_secs() > 900 {
                emit("error", "Модель не загрузилась за 15 минут");
                return;
            }
        }
        match engine::verify_loopback_only(port) {
            Ok(()) => {
                st2.ready.store(true, Ordering::SeqCst);
                emit("ready", &format!("{model} готова"));
            }
            Err(m) => {
                if let Some(mut e) = st2.engine.lock().unwrap().take() {
                    engine::stop(&mut e);
                }
                emit("error", &m);
            }
        }
    });
    Ok(())
}

/// Тяжёлая или сетевая работа уходит из потока окна: интерфейс не замирает.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn active_remote(st: &App) -> Option<ssh::Remote> {
    let label = st.remote.lock().unwrap().clone()?;
    st.remotes.lock().unwrap().iter().find(|r| r.label == label).cloned()
}

#[tauri::command]
fn remote_list(st: State<'_, Arc<App>>) -> Value {
    json!({"remotes": st.remotes.lock().unwrap().clone(), "active": st.remote.lock().unwrap().clone()})
}

#[tauri::command]
fn remote_save(st: State<'_, Arc<App>>, remote: ssh::Remote, old_label: Option<String>) -> Result<(), String> {
    remote.validate()?;
    {
        let mut g = st.remotes.lock().unwrap();
        let old = old_label.unwrap_or_else(|| remote.label.clone());
        if let Some(x) = g.iter_mut().find(|r| r.label == old) {
            *x = remote.clone();
        } else if g.iter().any(|r| r.label == remote.label) {
            return Err("сервер с таким названием уже есть".into());
        } else {
            g.push(remote.clone());
        }
        let mut a = st.remote.lock().unwrap();
        if a.as_deref() == Some(old.as_str()) {
            *a = Some(remote.label.clone());
        }
    }
    save_cfg(&st);
    Ok(())
}

#[tauri::command]
fn remote_delete(st: State<'_, Arc<App>>, label: String) {
    st.remotes.lock().unwrap().retain(|r| r.label != label);
    let mut a = st.remote.lock().unwrap();
    if a.as_deref() == Some(label.as_str()) {
        *a = None;
        st.conv.lock().unwrap().clear();
    }
    drop(a);
    save_cfg(&st);
}

/// Переключиться на проект на сервере (label) или обратно на локальную папку (None).
#[tauri::command]
fn remote_use(st: State<'_, Arc<App>>, label: Option<String>) -> Result<(), String> {
    if let Some(l) = &label {
        if !st.remotes.lock().unwrap().iter().any(|r| &r.label == l) {
            return Err("такого сервера нет".into());
        }
    }
    *st.remote.lock().unwrap() = label;
    st.conv.lock().unwrap().clear();
    save_cfg(&st);
    Ok(())
}

#[tauri::command]
async fn remote_scan(host: String, port: u16) -> Result<Vec<Value>, String> {
    let r = tauri::async_runtime::spawn_blocking(move || ssh::keyscan(&host, port)).await.map_err(|e| e.to_string())??;
    Ok(r.into_iter().map(|(line, fp)| json!({"line": line, "fp": fp})).collect())
}

#[tauri::command]
fn remote_trust(host: String, port: u16, lines: Vec<String>) -> Result<(), String> {
    for l in lines {
        ssh::trust_host(&host, port, &l)?;
    }
    Ok(())
}

#[tauri::command]
async fn remote_test(st: State<'_, Arc<App>>, label: String) -> Result<String, String> {
    let rm = st.remotes.lock().unwrap().iter().find(|r| r.label == label).cloned().ok_or("такого сервера нет")?;
    tauri::async_runtime::spawn_blocking(move || ssh::test(&rm)).await.map_err(|e| e.to_string())?
}

/// Выполнить задачу по расписанию в фоне: отдельный диалог, права «доверять папке», без вопросов (всё, что требует
/// подтверждения, отклоняется). Итог пишется в файл отчёта и в список задач.
fn run_task(app: &tauri::AppHandle, st: &Arc<App>, task: tasks::Task) {
    if !st.ready.load(Ordering::SeqCst) || st.busy.swap(true, Ordering::SeqCst) {
        return; // движок не готов или идёт чат: попробуем в следующий раз
    }
    let now = totp::now_secs();
    let local = tasks::local_now();
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let result = (|| -> Result<String, String> {
        let (base, token, alias, _) = current_conn(st).ok_or("движок не запущен")?;
        let ws = task.workspace.as_ref().map(PathBuf::from).filter(|p| p.is_dir());
        let remote = task.remote.as_ref().and_then(|l| st.remotes.lock().unwrap().iter().find(|r| &r.label == l).cloned());
        if task.remote.is_some() && remote.is_none() {
            return Err("сервер задачи удалён из списка".into());
        }
        if remote.is_none() && ws.is_none() {
            return Err("папка проекта задачи недоступна".into());
        }
        let key = memory::key_for(task.remote.as_deref(), task.workspace.as_deref());
        let l2 = log.clone();
        let ctx = Ctx {
            root: ws.clone().unwrap_or_default(),
            mode: Mode::Trust,
            approvals: st.approvals.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            apps: st.apps.clone(),
            log_dir: engine::data_dir().join("logs"),
            ctx_tokens: effective_ctx(),
            remote: remote.clone(),
            mem_key: key.clone(),
            grants: Default::default(),
            chat_trust: Default::default(),
            scripts_ok: Default::default(),
            unattended: true,
            emit: Box::new(move |name, p| {
                if name == "tool" {
                    let s = p["status"].as_str().unwrap_or("");
                    if s != "running" {
                        l2.lock().unwrap().push(format!("- {}: {}", p["title"].as_str().unwrap_or(""), s));
                    }
                }
            }),
            auto: false,
        };
        let mut sys = compose_system(&ws, &remote, None);
        sys.push_str(&memory::default_store().prompt_block(&key));
        let mut conv = vec![
            json!({"role": "system", "content": sys}),
            json!({"role": "user", "content": format!("Задача по расписанию «{}». Выполняй самостоятельно и не задавай вопросов: пользователя сейчас нет. Действия, требующие подтверждения (удаление, боевой сервер), будут отклонены: не повторяй их, а опиши, что нужно сделать вручную.\n\n{}", task.name, task.prompt)}),
        ];
        agent::run(&ctx, &base, &token, &alias, &mut conv, true)?;
        Ok(conv.iter().rev().find(|m| m["role"] == "assistant" && m["content"].as_str().map_or(false, |c| !c.trim().is_empty())).and_then(|m| m["content"].as_str()).unwrap_or("(модель не дала ответа)").to_string())
    })();
    let (status, file) = {
        let dir = tasks::reports_dir();
        let _ = std::fs::create_dir_all(&dir);
        let name = format!("{}_{}_{:02}{:02}.md", task.id, local.date, local.minutes / 60, local.minutes % 60);
        let steps = log.lock().unwrap().join("\n");
        let (head, body) = match &result {
            Ok(a) => ("выполнено".to_string(), a.clone()),
            Err(e) => (format!("ошибка: {e}"), String::new()),
        };
        let text = format!("# {}\n\nПроект: {}\nКогда: {} {:02}:{:02}\nСтатус: {}\n\n## Что сделал\n{}\n\n## Итог\n{}\n", task.name, task.remote.clone().or(task.workspace.clone()).unwrap_or_default(), local.date, local.minutes / 60, local.minutes % 60, head, if steps.is_empty() { "(действий с инструментами не было)".to_string() } else { steps }, body);
        let ok = std::fs::write(dir.join(&name), text).is_ok();
        (head, if ok { name } else { String::new() })
    };
    let _ = tasks::default_store().record(task.id, now, &local.date, &status, &file);
    st.busy.store(false, Ordering::SeqCst);
    let _ = app.emit("tasks-changed", json!({}));
}

#[tauri::command]
fn tasks_list(st: State<'_, Arc<App>>) -> Value {
    json!({"tasks": tasks::default_store().list(), "ready": st.ready.load(Ordering::SeqCst), "project": {"remote": st.remote.lock().unwrap().clone(), "workspace": st.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string())}})
}

#[tauri::command]
fn task_save(st: State<'_, Arc<App>>, name: String, prompt: String, kind: String, at: String, every_min: u32) -> Result<u64, String> {
    let remote = st.remote.lock().unwrap().clone();
    let workspace = if remote.is_some() { None } else { st.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string()) };
    let t = tasks::Task { id: 0, name: name.trim().to_string(), prompt: prompt.trim().to_string(), workspace, remote, kind, at: at.trim().to_string(), every_min, enabled: true, last_run: 0, last_day: String::new(), last_status: String::new(), last_file: String::new() };
    tasks::default_store().add(t, totp::now_secs())
}

#[tauri::command]
fn task_delete(id: u64) -> Result<(), String> {
    tasks::default_store().remove(id)
}

#[tauri::command]
fn task_toggle(id: u64, on: bool) -> Result<(), String> {
    tasks::default_store().set_enabled(id, on)
}

#[tauri::command]
fn task_run_now(app: tauri::AppHandle, st: State<'_, Arc<App>>, id: u64) -> Result<(), String> {
    if !st.ready.load(Ordering::SeqCst) {
        return Err("Модель ещё не готова: запустите её на странице чата".into());
    }
    if st.busy.load(Ordering::SeqCst) {
        return Err("Сейчас идёт другой запрос, попробуйте позже".into());
    }
    let t = tasks::default_store().get(id).ok_or("такой задачи нет")?;
    let st2: Arc<App> = st.inner().clone();
    std::thread::spawn(move || run_task(&app, &st2, t));
    Ok(())
}

#[tauri::command]
fn task_result(id: u64) -> Result<String, String> {
    let t = tasks::default_store().get(id).ok_or("такой задачи нет")?;
    if t.last_file.is_empty() || t.last_file.contains(['/', '\\']) {
        return Err("отчёта пока нет".into());
    }
    let s = std::fs::read_to_string(tasks::reports_dir().join(&t.last_file)).map_err(|e| e.to_string())?;
    Ok(s.chars().take(20_000).collect())
}

#[tauri::command]
fn mcp_list() -> Value {
    json!(mcp::status())
}

#[tauri::command]
fn mcp_preview(text: String) -> Result<Vec<Value>, String> {
    Ok(mcp::parse_import(&text)?.into_iter().map(|(c, _)| json!({"name": c.name, "command": c.command, "args": c.args, "env_keys": c.env_keys})).collect())
}

#[tauri::command]
fn mcp_import(text: String) -> Result<usize, String> {
    let list = mcp::parse_import(&text)?;
    let n = list.len();
    for (c, env) in list {
        mcp::add(c, env)?;
    }
    Ok(n)
}

#[tauri::command]
fn mcp_toggle(name: String, on: bool) -> Result<(), String> {
    mcp::set_enabled(&name, on)
}

#[tauri::command]
fn mcp_remove(name: String) -> Result<(), String> {
    mcp::remove(&name)
}

#[tauri::command]
fn mcp_restart(name: String) {
    mcp::restart(&name)
}

/// Сохранить текущий разговор на диск (после каждого ответа).
fn save_chat(st: &App) {
    let conv = st.conv.lock().unwrap().clone();
    if !conv.iter().any(|m| m["role"] == "user") {
        return;
    }
    let ws = st.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string());
    let remote = st.remote.lock().unwrap().clone();
    let id = *st.chat_id.lock().unwrap();
    if let Ok(nid) = chats::default_store().save(id, if remote.is_some() { None } else { ws }, remote, &conv) {
        *st.chat_id.lock().unwrap() = Some(nid);
    }
}

#[tauri::command]
fn chats_list(st: State<'_, Arc<App>>) -> Value {
    let cur = *st.chat_id.lock().unwrap();
    json!({"current": cur, "chats": chats::default_store().list().into_iter().map(|c| json!({"id": c.id, "title": c.title, "updated": c.updated, "project": c.remote.clone().map(|r| format!("сервер {r}")).or(c.workspace.clone()), "turns": c.conv.iter().filter(|m| m["role"] == "user").count()})).collect::<Vec<_>>()})
}

/// Открыть сохранённый разговор: история загружается в модель, проект возвращается тот же, если он ещё доступен.
#[tauri::command]
fn chat_open(st: State<'_, Arc<App>>, id: u64) -> Result<Value, String> {
    if st.busy.load(Ordering::SeqCst) {
        return Err("Дождитесь окончания ответа или нажмите «Стоп»".into());
    }
    let c = chats::default_store().load(id).ok_or("такого разговора нет")?;
    *st.conv.lock().unwrap() = c.conv.clone();
    *st.chat_id.lock().unwrap() = Some(id);
    st.chat_trust.store(false, Ordering::SeqCst);
    match &c.remote {
        Some(l) => {
            if st.remotes.lock().unwrap().iter().any(|r| &r.label == l) {
                *st.remote.lock().unwrap() = Some(l.clone());
            }
        }
        None => {
            if let Some(w) = c.workspace.as_ref().filter(|w| std::path::Path::new(w).is_dir()) {
                *st.workspace.lock().unwrap() = Some(PathBuf::from(w));
                *st.remote.lock().unwrap() = None;
                st.ws_auto.store(false, Ordering::SeqCst);
            }
        }
    }
    save_cfg(&st);
    Ok(json!({
        "title": c.title,
        "messages": chats::display(&c.conv).into_iter().map(|(r, t)| json!({"role": r, "text": t})).collect::<Vec<_>>(),
        "workspace": st.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string()),
        "remote": st.remote.lock().unwrap().clone(),
    }))
}

#[tauri::command]
fn chat_delete(st: State<'_, Arc<App>>, id: u64) {
    chats::default_store().delete(id);
    let mut cur = st.chat_id.lock().unwrap();
    if *cur == Some(id) {
        *cur = None;
    }
}

#[tauri::command]
fn chat_rename(id: u64, title: String) -> Result<(), String> {
    chats::default_store().rename(id, &title)
}

#[tauri::command]
fn get_prefs() -> Value {
    json!({"think": prefs::think(), "web": prefs::web()})
}

#[tauri::command]
fn set_think(on: bool) -> Value {
    prefs::set_think(on);
    json!({"think": prefs::think(), "web": prefs::web()})
}

#[tauri::command]
fn set_web(on: bool) -> Value {
    prefs::set_web(on);
    json!({"think": prefs::think(), "web": prefs::web()})
}

#[tauri::command]
fn set_chat_trust(st: State<'_, Arc<App>>, on: bool) {
    st.chat_trust.store(on, Ordering::SeqCst);
}

#[tauri::command]
fn memory_list(st: State<'_, Arc<App>>) -> Vec<Value> {
    let key = current_mem_key(&st);
    memory::default_store()
        .visible(&key)
        .into_iter()
        .map(|n| json!({"id": n.id, "text": n.text, "global": n.key == memory::GLOBAL, "ts": n.ts}))
        .collect()
}

#[tauri::command]
fn memory_delete(st: State<'_, Arc<App>>, id: u64) -> Result<bool, String> {
    let key = current_mem_key(&st);
    let store = memory::default_store();
    match store.get(id) {
        Some(n) if n.key == memory::GLOBAL || n.key == key => store.remove(id),
        _ => Ok(false),
    }
}

fn current_mem_key(st: &App) -> String {
    let ws = st.workspace.lock().unwrap().as_ref().map(|p| p.display().to_string());
    let remote = st.remote.lock().unwrap().clone();
    memory::key_for(remote.as_deref(), ws.as_deref())
}

#[tauri::command]
fn chat_send(app: tauri::AppHandle, st: State<'_, Arc<App>>, text: String, images: Option<Vec<String>>) -> Result<(), String> {
    if !st.ready.load(Ordering::SeqCst) {
        return Err("Модель ещё не готова".into());
    }
    if st.busy.swap(true, Ordering::SeqCst) {
        return Err("Предыдущий запрос ещё выполняется".into());
    }
    let (base, token, alias, vision) = {
        let g = st.engine.lock().unwrap();
        match g.as_ref() {
            Some(e) => (e.base.clone(), e.token.clone(), e.alias.clone(), e.vision),
            None => {
                st.busy.store(false, Ordering::SeqCst);
                return Err("Движок не запущен".into());
            }
        }
    };
    let mut ws = st.workspace.lock().unwrap().clone();
    let remote = active_remote(&st);
    let mem_key = current_mem_key(&st);
    let empty = st.conv.lock().unwrap().is_empty();
    if empty {
        st.chat_trust.store(false, Ordering::SeqCst);
        *st.chat_id.lock().unwrap() = None; // пустая история: это новый разговор
    }
    if empty && remote.is_none() && st.auto_folder.load(Ordering::SeqCst) && (ws.is_none() || st.ws_auto.load(Ordering::SeqCst)) {
        match create_project(&st) {
            Ok(p) => {
                *st.workspace.lock().unwrap() = Some(p.clone());
                st.ws_auto.store(true, Ordering::SeqCst);
                save_cfg(&st);
                let _ = app.emit("workspace", json!({"path": p.display().to_string()}));
                ws = Some(p);
            }
            Err(e) => {
                st.busy.store(false, Ordering::SeqCst);
                return Err(e);
            }
        }
    }
    let mode = *st.mode.lock().unwrap();
    let mut conv = st.conv.lock().unwrap().clone();
    if conv.is_empty() {
        let mut sys = compose_system(&ws, &remote, st.role.lock().unwrap().clone());
        sys.push_str(&memory::default_store().prompt_block(&current_mem_key(&st)));
        conv.push(json!({"role": "system", "content": sys}));
    }
    let imgs = images.unwrap_or_default();
    // режим «Авто»: основная модель не видит картинки, их читает отдельная модель, а описание уходит основной
    let handoff = !imgs.is_empty() && !vision && st.auto_mode.load(Ordering::SeqCst);
    let user: Option<Value> = if handoff {
        None
    } else {
        match build_user_message(&text, imgs.clone(), vision) {
            Ok(u) => Some(u),
            Err(e) => {
                st.busy.store(false, Ordering::SeqCst);
                return Err(e);
            }
        }
    };
    strip_old_images(&mut conv);
    if let Some(u) = &user {
        conv.push(u.clone());
    }
    st.cancel.store(false, Ordering::SeqCst);
    let st2: Arc<App> = st.inner().clone();
    std::thread::spawn(move || {
        let app2 = app.clone();
        let ctx = Ctx {
            root: ws.clone().unwrap_or_default(),
            mode,
            approvals: st2.approvals.clone(),
            cancel: st2.cancel.clone(),
            apps: st2.apps.clone(),
            log_dir: engine::data_dir().join("logs"),
            ctx_tokens: effective_ctx(),
            remote: remote.clone(),
            mem_key: mem_key.clone(),
            grants: st2.grants.clone(),
            chat_trust: st2.chat_trust.clone(),
            scripts_ok: st2.scripts_ok.clone(),
            unattended: false,
            emit: Box::new(move |name, payload| {
                let _ = app2.emit(name, payload);
            }),
            auto: false,
        };
        let (mut base, mut token, mut alias) = (base, token, alias);
        let mut pre_err: Option<String> = None;
        if handoff {
            match describe_via_vision(&app, &st2, &text, imgs) {
                Ok(desc) => {
                    conv.push(json!({"role": "user", "content": format!("{text}\n\n[Описание картинок от модели-зрения]\n{desc}")}));
                    if let Some((b, t, a, _)) = current_conn(&st2) {
                        base = b;
                        token = t;
                        alias = a;
                    }
                }
                Err(e) => pre_err = Some(e),
            }
        }
        let r = match pre_err {
            Some(e) => Err(e),
            None => agent::run(&ctx, &base, &token, &alias, &mut conv, ws.is_some() || remote.is_some() || prefs::web()),
        };
        *st2.conv.lock().unwrap() = conv;
        save_chat(&st2);
        st2.busy.store(false, Ordering::SeqCst);
        let _ = app.emit("chat-done", json!({"error": r.err()}));
    });
    Ok(())
}

/// Системная инструкция: базовая + правила проекта (LOCALCODER.md / AGENTS.md) + роль агента + список навыков.
fn compose_system(ws: &Option<PathBuf>, remote: &Option<ssh::Remote>, role: Option<String>) -> String {
    let mut s = match (remote, ws) {
        (Some(rm), _) => agent::system_prompt_remote(rm),
        (None, Some(p)) => agent::system_prompt(p),
        (None, None) => "Ты помощник-программист. Отвечай по-русски, кратко и по делу. Папка проекта не выбрана, инструментов нет.".to_string(),
    };
    if let (None, Some(p)) = (remote, ws) {
        s.push(' ');
        s.push_str(selfdev::WHERE_AM_I);
        if let Some(b) = selfdev::prompt_block(p) {
            s.push_str(&b);
        }
        for name in ["LOCALCODER.md", "AGENTS.md"] {
            if let Ok(t) = std::fs::read_to_string(p.join(name)) {
                let t: String = t.chars().take(6000).collect();
                s.push_str(&format!("\n\nПравила проекта (из {name}):\n{t}"));
                break;
            }
        }
    }
    if let Some(r) = role {
        if let Some(body) = plugins::agent_body(&r) {
            s.push_str(&format!("\n\nТвоя роль ({r}):\n{body}"));
        }
    }
    // список навыков в подсказку не кладём (это тысячи токенов): подходящий навык подключается по запросу (router.rs), поиск: list_skills
    if !plugins::skill_index().is_empty() {
        s.push_str("\n\nУстановлены навыки: нужный подбирается и подключается автоматически; поиск по теме: list_skills.");
    }
    s
}

fn current_conn(st: &App) -> Option<(String, String, String, bool)> {
    st.engine.lock().unwrap().as_ref().map(|e| (e.base.clone(), e.token.clone(), e.alias.clone(), e.vision))
}

fn wait_ready(st: &Arc<App>) -> Result<(), String> {
    let t = std::time::Instant::now();
    loop {
        if st.ready.load(Ordering::SeqCst) {
            return Ok(());
        }
        if st.cancel.load(Ordering::SeqCst) {
            return Err("Отменено".into());
        }
        if st.engine.lock().unwrap().is_none() {
            return Err("Движок не запустился. См. engine.log".into());
        }
        if t.elapsed().as_secs() > 900 {
            return Err("Модель не загрузилась за 15 минут".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// Режим «Авто»: временно подменяет движок на модель с «зрением», получает описание картинок и возвращает основную модель.
/// В памяти всегда одна модель, поэтому смена занимает десятки секунд (модель читается с диска).
fn describe_via_vision(app: &tauri::AppHandle, st: &Arc<App>, text: &str, images: Vec<String>) -> Result<String, String> {
    if st.source.lock().unwrap().as_str() == "external" {
        return Err("Режим «Авто» с картинками работает только со встроенным движком".into());
    }
    let vm = catalog::load()
        .into_iter()
        .find(|m| !m.mmproj.is_empty() && download::is_ready(m))
        .ok_or("Нет скачанной модели с «зрением». Скачайте Gemma 3 в каталоге моделей.")?;
    let orig = st.model.lock().unwrap().clone();
    let status = |t: &str| {
        let _ = app.emit("chat-status", json!({"text": t}));
    };
    let swap_to = |name: &str| -> Result<(), String> {
        st.ready.store(false, Ordering::SeqCst);
        if let Some(mut e) = st.engine.lock().unwrap().take() {
            engine::stop(&mut e);
        }
        *st.model.lock().unwrap() = name.to_string();
        start_engine(app.clone(), st.clone())?;
        wait_ready(st)
    };
    status("Подгружаю модель, читающую картинки…");
    swap_to(&format!("nx:{}", vm.id))?;
    status("Читаю картинку…");
    let q = format!("{text}\n\nОпиши подробно, что на картинке. Весь видимый текст перепиши дословно. Отметь ошибки, номера, цифры и названия файлов.");
    let msg = build_user_message(&q, images, true)?;
    let (base, token, alias, _) = current_conn(st).ok_or("нет движка")?;
    let mut out = String::new();
    let r = chat::stream_turn(&base, &token, &alias, &[msg], None, &st.cancel, |ev| {
        if let chat::Ev::Text(t) = ev {
            out.push_str(&t)
        }
    });
    status("Возвращаю основную модель…");
    let back = swap_to(&orig);
    r?;
    back?;
    if out.trim().is_empty() {
        return Err("Модель не вернула описания картинки".into());
    }
    Ok(out.trim().to_string())
}

#[tauri::command]
fn set_auto_mode(st: State<'_, Arc<App>>, on: bool) {
    st.auto_mode.store(on, Ordering::SeqCst);
    save_cfg(&st);
}

/// Сообщение пользователя: текст и (если есть) картинки в формате OpenAI (data-URL).
fn build_user_message(text: &str, images: Vec<String>, vision: bool) -> Result<Value, String> {
    if images.is_empty() {
        return Ok(json!({"role": "user", "content": text}));
    }
    if !vision {
        return Err("Эта модель не умеет читать картинки. Выберите модель с «зрением» (например, Gemma 3) в каталоге.".into());
    }
    if images.len() > 6 {
        return Err("Не больше 6 картинок за раз".into());
    }
    let mut parts = vec![json!({"type": "text", "text": if text.trim().is_empty() { "Посмотри на картинку." } else { text }})];
    for im in images {
        let ok = ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/webp;base64,", "data:image/gif;base64,"].iter().any(|p| im.starts_with(p));
        if !ok {
            return Err("Допустимы картинки PNG, JPEG, WEBP и GIF".into());
        }
        if im.len() > 14_000_000 {
            return Err("Картинка слишком большая (больше ~10 МБ)".into());
        }
        parts.push(json!({"type": "image_url", "image_url": {"url": im}}));
    }
    Ok(json!({"role": "user", "content": parts}))
}

/// Старые картинки в истории заменяются пометкой: иначе каждый ход заново тратил бы контекст.
fn strip_old_images(conv: &mut [Value]) {
    for m in conv.iter_mut() {
        if m["role"] != "user" {
            continue;
        }
        let Some(arr) = m["content"].as_array() else { continue };
        let n = arr.iter().filter(|p| p["type"] == "image_url").count();
        if n == 0 {
            continue;
        }
        let txt: Vec<&str> = arr.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect();
        let s = format!("{} [картинок было: {n}]", txt.join(" "));
        m["content"] = Value::String(s);
    }
}

#[tauri::command]
fn chat_stop(st: State<'_, Arc<App>>) {
    st.cancel.store(true, Ordering::SeqCst);
}

fn stop_engine(app: &Arc<App>) {
    for p in app.apps.lock().unwrap().drain(..) {
        tools::kill_tree(p.pid);
    }
    app.ready.store(false, Ordering::SeqCst);
    if let Some(mut e) = app.engine.lock().unwrap().take() {
        engine::stop(&mut e);
    }
}

/// Запуск движка для самопроверок без окна.
fn boot_engine(say: &mut dyn FnMut(String)) -> Option<engine::Engine> {
    let s = match settings(MODEL) {
        Ok(s) => s,
        Err(e) => {
            say(format!("FAIL settings: {e}"));
            return None;
        }
    };
    let mut e = match engine::start(&s) {
        Ok(e) => e,
        Err(m) => {
            say(format!("FAIL start: {m}"));
            return None;
        }
    };
    say(format!("engine on 127.0.0.1:{}", e.port));
    let t0 = std::time::Instant::now();
    while !engine::health(e.port) {
        if let Some(c) = e.exited() {
            say(format!("FAIL engine exited {c}"));
            return None;
        }
        if t0.elapsed().as_secs() > 600 {
            engine::stop(&mut e);
            say("FAIL timeout".into());
            return None;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    say(format!("ready in {}s", t0.elapsed().as_secs()));
    Some(e)
}

/// Сквозная проверка агента: исправить баг в calc.py. `localcoder.exe --selftest-agent <файл-отчёт>`
fn selftest_agent(out: &str) {
    let log = Arc::new(Mutex::new(String::new()));
    let out = out.to_string();
    let say_log = log.clone();
    let say = move |s: String| {
        let mut g = say_log.lock().unwrap();
        g.push_str(&s);
        g.push('\n');
        let _ = std::fs::write(&out, &*g);
    };
    let say = Arc::new(Mutex::new(say));
    let mut s1 = {
        let say = say.clone();
        move |s: String| (say.lock().unwrap())(s)
    };
    let Some(mut e) = boot_engine(&mut s1) else { return };
    let dir = std::env::temp_dir().join(format!("lc_agent_demo_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("calc.py"), "def average(nums):\n    return sum(nums) / len(nums) + 1\n\n\nif __name__ == \"__main__\":\n    print(\"avg:\", average([2, 4, 6]))\n").unwrap();
    s1(format!("project: {}", dir.display()));
    let before = tools::run_command(&dir, "python calc.py", 20);
    s1(format!("before: {}", before.lines().next().unwrap_or("")));

    let say2 = say.clone();
    let ctx = Ctx {
        root: dir.clone(),
        mode: Mode::AskChanges,
        approvals: Arc::new(Approvals::default()),
        cancel: Arc::new(AtomicBool::new(false)),
        apps: Arc::new(Mutex::new(vec![])),
        log_dir: dir.join("_logs"),
        ctx_tokens: 8192,
        emit: Box::new(move |name, p| {
            let line = match name {
                "tool" if p["status"] != "running" => Some(format!("tool {} -> {} | {}", p["name"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or(""), p["title"].as_str().unwrap_or(""))),
                "approval" => Some(format!("approval[{}] {}", p["kind"].as_str().unwrap_or(""), p["title"].as_str().unwrap_or(""))),
                _ => None,
            };
            if let Some(l) = line {
                (say2.lock().unwrap())(l);
            }
        }),
        remote: None,
        mem_key: String::new(),
        grants: Default::default(),
        chat_trust: Default::default(),
        scripts_ok: Default::default(),
        unattended: false,
        auto: true,
    };
    let mut conv = vec![
        json!({"role": "system", "content": agent::system_prompt(&dir)}),
        json!({"role": "user", "content": "Скрипт calc.py должен печатать avg: 4.0, а печатает другое. Найди и исправь."}),
    ];
    let t = std::time::Instant::now();
    let r = agent::run(&ctx, &e.base, &e.token, &e.alias, &mut conv, true);
    s1(format!("agent: {:?} in {}s", r, t.elapsed().as_secs()));
    if let Some(last) = conv.last() {
        s1(format!("final answer: {}", last["content"].as_str().unwrap_or("").trim()));
    }
    let after = tools::run_command(&dir, "python calc.py", 20);
    let line = after.lines().next().unwrap_or("").to_string();
    s1(format!("after: {line}"));
    engine::stop(&mut e);
    s1(format!("PASS={}", r.is_ok() && line.trim() == "avg: 4.0"));
}

/// Проверка внешнего движка: `localcoder.exe --selftest-ext <файл-отчёт> <адрес>`
fn selftest_ext(out: &str, base: &str) {
    let mut log = String::new();
    let mut say = |s: String| {
        log.push_str(&s);
        log.push('\n');
        let _ = std::fs::write(out, &log);
    };
    say(format!("private host: {}", engine::is_private_host(base)));
    let models = match engine::probe(base, "") {
        Ok(m) => m,
        Err(e) => return say(format!("FAIL probe: {e}")),
    };
    say(format!("models: {models:?}"));
    say(format!("kind: {}", engine::kind(base)));
    let Some(model) = models.iter().find(|m| m.starts_with("qwen3-coder")).or(models.first()).cloned() else { return say("FAIL no models".into()) };
    let cancel = AtomicBool::new(false);
    let mut text = String::new();
    let t = std::time::Instant::now();
    let r = chat::stream_turn(base, "", &model, &[json!({"role": "user", "content": "Ответь одним словом по-русски: столица Франции?"})], None, &cancel, |ev| {
        if let chat::Ev::Text(x) = ev {
            text.push_str(&x)
        }
    });
    say(format!("chat via {model}: {:?} in {}s -> {}", r.map(|t| t.content.len()), t.elapsed().as_secs(), text.trim()));
    say(format!("PASS={}", !text.is_empty()));
}

/// Печатает определённое железо и оценки: `nexora.exe --selftest-hw <файл-отчёт>`
fn selftest_hw(out: &str) {
    let t = std::time::Instant::now();
    let h = hw::get();
    let ctx = catalog::default_ctx(h).min(32768);
    let mut s = format!("detected in {:.1}s\n{}\nctx={ctx}\n", t.elapsed().as_secs_f32(), serde_json::to_string_pretty(h).unwrap_or_default());
    for (m, e) in catalog::estimate_all(h, ctx) {
        s.push_str(&format!("{:<28} fits={:<5} mode={:<16} {:>5.1} tok/s  mem {:>5.1} GB\n", m.name, e.fits, e.mode, e.tok_s, e.mem_gb));
    }
    let best = catalog::estimate_all(h, ctx);
    s.push_str(&format!("recommended: {:?}\n", catalog::pick(&best, 6.0).map(|(m, _)| m.id.clone())));
    let _ = std::fs::write(out, s);
}

/// Скачивает модель из каталога, запускает её и задаёт вопрос: `nexora.exe --selftest-dl <отчёт> <id>`
fn selftest_dl(out: &str, id: &str) {
    let mut log = String::new();
    let mut say = |s: String| {
        log.push_str(&s);
        log.push('\n');
        let _ = std::fs::write(out, &log);
    };
    let Some(m) = catalog::load().into_iter().find(|m| m.id == id) else { return say("FAIL: нет в каталоге".into()) };
    let dest = download::model_path(&m);
    say(format!("url: {}
dest: {}", download::model_url(&m), dest.display()));
    let cancel = AtomicBool::new(false);
    let t = std::time::Instant::now();
    let mut last = 0u64;
    let r = download::fetch(&download::model_url(&m), &dest, (m.size_gb * 0.9 * 1e9) as u64, &cancel, |d, tot, sp| {
        if d / 50_000_000 != last / 50_000_000 {
            last = d;
            let _ = std::fs::write(out, format!("downloading {} / {} MB at {:.1} MB/s
", d >> 20, tot >> 20, sp / 1048576.0));
        }
    });
    say(format!("download: {:?} in {}s", r, t.elapsed().as_secs()));
    if r.is_err() {
        return;
    }
    let mut magic = [0u8; 4];
    let _ = std::io::Read::read_exact(&mut std::fs::File::open(&dest).unwrap(), &mut magic);
    say(format!("magic: {:?} ok={}", String::from_utf8_lossy(&magic), &magic == b"GGUF"));
    let st = match settings(&format!("nx:{id}")) {
        Ok(s) => s,
        Err(e) => return say(format!("FAIL settings: {e}")),
    };
    say(format!("ctx: {}", st.ctx));
    let mut e = match engine::start(&st) {
        Ok(e) => e,
        Err(m) => return say(format!("FAIL start: {m}")),
    };
    let t0 = std::time::Instant::now();
    while !engine::health(e.port) {
        if e.exited().is_some() || t0.elapsed().as_secs() > 300 {
            engine::stop(&mut e);
            return say("FAIL engine did not start".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let cancel = AtomicBool::new(false);
    let mut text = String::new();
    let r = chat::stream_turn(&e.base, &e.token, &e.alias, &[json!({"role": "user", "content": "Напиши на Python функцию сложения двух чисел."})], None, &cancel, |ev| {
        if let chat::Ev::Text(x) = ev {
            text.push_str(&x)
        }
    });
    engine::stop(&mut e);
    say(format!("chat: {:?}
{}
PASS={}", r.map(|t| t.content.len()), text.trim(), !text.is_empty()));
}

/// Скачивает модель с «зрением», отправляет ей картинку и спрашивает, что на ней: `nexora.exe --selftest-vision <отчёт> <картинка.png> <id>`
fn selftest_vision(out: &str, image: &str, id: &str) {
    let mut log = String::new();
    let mut say = |s: String| {
        log.push_str(&s);
        log.push('\n');
        let _ = std::fs::write(out, &log);
    };
    let Some(m) = catalog::load().into_iter().find(|m| m.id == id) else { return say("FAIL: нет в каталоге".into()) };
    let cancel = AtomicBool::new(false);
    let t = std::time::Instant::now();
    let mut last = 0u64;
    let r = download::fetch_model(&m, &cancel, |ph, d, tot, sp| {
        if d / 100_000_000 != last / 100_000_000 || d == tot {
            last = d;
            let _ = std::fs::write(out, format!("downloading {ph}: {} / {} MB at {:.1} MB/s\n", d >> 20, tot >> 20, sp / 1048576.0));
        }
    });
    say(format!("download: {:?} in {}s", r, t.elapsed().as_secs()));
    if r.is_err() {
        return;
    }
    let st = match settings(&format!("nx:{id}")) {
        Ok(s) => s,
        Err(e) => return say(format!("FAIL settings: {e}")),
    };
    say(format!("mmproj: {:?}", st.mmproj));
    let mut e = match engine::start(&st) {
        Ok(e) => e,
        Err(m) => return say(format!("FAIL start: {m}")),
    };
    let t0 = std::time::Instant::now();
    while !engine::health(e.port) {
        if e.exited().is_some() || t0.elapsed().as_secs() > 300 {
            engine::stop(&mut e);
            return say("FAIL engine did not start (see engine.log)".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let bytes = std::fs::read(image).unwrap_or_default();
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        b64.push(T[(n >> 18) as usize & 63] as char);
        b64.push(T[(n >> 12) as usize & 63] as char);
        b64.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        b64.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    let msg = match build_user_message("Что написано на картинке? Ответь коротко.", vec![format!("data:image/png;base64,{b64}")], e.vision) {
        Ok(m) => m,
        Err(er) => {
            engine::stop(&mut e);
            return say(format!("FAIL message: {er}"));
        }
    };
    let cancel = AtomicBool::new(false);
    let mut text = String::new();
    let t1 = std::time::Instant::now();
    let r = chat::stream_turn(&e.base, &e.token, &e.alias, &[msg], None, &cancel, |ev| {
        if let chat::Ev::Text(x) = ev {
            text.push_str(&x)
        }
    });
    engine::stop(&mut e);
    say(format!("vision chat: {:?} in {}s\nANSWER: {}\nPASS={}", r.map(|t| t.content.len()), t1.elapsed().as_secs(), text.trim(), !text.is_empty()));
}

/// Проверка сетевых функций: `nexora.exe --selftest-net <отчёт> <репозиторий-с-плагинами>`
fn selftest_net(out: &str, plugin_repo: &str) {
    let mut log = String::new();
    let mut say = |s: String| {
        log.push_str(&s);
        log.push('\n');
        let _ = std::fs::write(out, &log);
    };
    match hf::search("qwen coder") {
        Ok(v) => say(format!("hf_search: {} результатов, первый: {}", v.len(), v.first().map(|x| x["id"].to_string()).unwrap_or_default())),
        Err(e) => say(format!("hf_search FAIL: {e}")),
    }
    match hf::files("unsloth/gemma-3-4b-it-GGUF") {
        Ok(v) => say(format!("hf_files: моделей {}, проекторов {}", v["files"].as_array().map(|a| a.len()).unwrap_or(0), v["mmproj"].as_array().map(|a| a.len()).unwrap_or(0))),
        Err(e) => say(format!("hf_files FAIL: {e}")),
    }
    match plugins::search("skills", "agent-skills") {
        Ok(v) => say(format!("github search: {} репозиториев, первый: {}", v.len(), v.first().map(|x| x["full_name"].to_string()).unwrap_or_default())),
        Err(e) => say(format!("github search FAIL: {e}")),
    }
    match plugins::preview(plugin_repo) {
        Ok(v) => {
            say(format!("preview: sha {} файлов {} скиллов {} агентов {} скриптов пропущено {}", v["sha"], v["files"], v["skills"].as_array().map(|a| a.len()).unwrap_or(0), v["agents"].as_array().map(|a| a.len()).unwrap_or(0), v["skipped_scripts"]));
            if let Some(sk) = v["skills"].as_array().and_then(|a| a.first()) {
                say(format!("первый скилл: {} :: {}", sk["name"], sk["description"]));
            }
            let sha = v["sha"].as_str().unwrap_or("").to_string();
            match plugins::install(plugin_repo, &sha, true, |_, _| {}) {
                Ok(p) => {
                    say(format!("install OK: {} файлов, скиллов {}", p.files, p.skills.len()));
                    say(format!("скриптов установлено: {}", p.scripts.len()));
                    if let Some(sc) = p.scripts.iter().find(|x| p.skills.iter().any(|k| k.path.rsplit_once('/').map_or(false, |d| x.starts_with(&format!("{}/", d.0))))) {
                        if let Some(sk) = p.skills.iter().find(|k| k.path.rsplit_once('/').map_or(false, |d| sc.starts_with(&format!("{}/", d.0)))) {
                            let rel = &sc[sk.path.rsplit_once('/').unwrap().0.len() + 1..];
                            match plugins::script_for(&sk.name, rel) {
                                Ok((full, ext)) => say(format!("script_for({}, {}): {} [{}] команда: {}", sk.name, rel, full.display(), ext, plugins::script_command(&full, &ext, &[]).unwrap_or_default().chars().take(90).collect::<String>())),
                                Err(e) => say(format!("script_for FAIL: {e}")),
                            }
                            say(format!("вне манифеста: {:?}", plugins::script_for(&sk.name, "../../../x.py").is_err()));
                        }
                    }
                    if let Some(first) = p.skills.first() {
                        match plugins::read_skill(&first.name) {
                            Ok(t) => say(format!("read_skill({}): {} символов; начало: {}", first.name, t.chars().count(), t.chars().take(80).collect::<String>().replace('\n', " "))),
                            Err(e) => say(format!("read_skill FAIL: {e}")),
                        }
                    }
                    say(format!("индекс навыков: {}", plugins::skill_index().len()));
                    let _ = plugins::remove(&p.id);
                    say(format!("после удаления плагинов: {}", plugins::list().len()));
                }
                Err(e) => say(format!("install FAIL: {e}")),
            }
        }
        Err(e) => say(format!("preview FAIL: {e}")),
    }
    say("DONE".into());
}

/// Выполнение задачи мобильного клиента. Агент работает как в обычном чате, но вопросы о подтверждении уходят на телефон.
/// Команды и файлы доступны только если включён переключатель «Команды и файлы для телефона» (по умолчанию выключен):
/// тогда агент работает в новой отдельной папке проекта, режим «спрашивать об изменениях».
fn mobile_exec(st: Arc<App>) -> mobile::Exec {
    Arc::new(move |hub: &Arc<mobile::Hub>, id: u64, prompt: &str, cancel: Arc<AtomicBool>| -> Result<String, String> {
        let t0 = std::time::Instant::now();
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("отменено".into());
            }
            if st.ready.load(Ordering::SeqCst) && !st.busy.swap(true, Ordering::SeqCst) {
                break;
            }
            if t0.elapsed() > std::time::Duration::from_secs(600) {
                return Err("Модель не готова или занята другим запросом".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let r = (|| -> Result<String, String> {
            let (base, token, alias, _) = current_conn(&st).ok_or("движок не запущен")?;
            let fast = hub.fast_of(id);
            let tools = hostmode::Settings::load().tools && !fast;
            let ws = if tools { Some(create_project(&st)?) } else { None };
            let h2 = hub.clone();
            let ctx = Ctx {
                root: ws.clone().unwrap_or_default(),
                mode: Mode::AskChanges,
                approvals: hub.approvals.clone(),
                cancel: cancel.clone(),
                apps: st.apps.clone(),
                log_dir: engine::data_dir().join("logs"),
                ctx_tokens: effective_ctx(),
                remote: None,
                // без проекта у телефона своя память, блок памяти десктопа ему не показывается
                mem_key: if ws.is_some() { memory::key_for(None, ws.as_ref().map(|p| p.display().to_string()).as_deref()) } else { "mobile".to_string() },
                grants: Default::default(),
                chat_trust: Default::default(),
                scripts_ok: Default::default(),
                unattended: false,
                emit: Box::new(move |name, p| h2.emit(id, name, p)),
                auto: false,
            };
            // быстрый чат: короткая подсказка вместо длинной (правила проекта, навыки, память), иначе на ЦП разбор подсказки занимает минуты
            let sys = if fast {
                "Ты Nexora, ИИ-ассистент. Отвечай кратко и по делу, на языке пользователя.".to_string()
            } else {
                let mut s = compose_system(&ws, &None, None);
                s.push_str(&memory::default_store().prompt_block(&ctx.mem_key));
                s
            };
            let mut conv = vec![json!({"role": "system", "content": sys})];
            for (role, text) in hub.history_of(id) {
                conv.push(json!({"role": role, "content": text}));
            }
            conv.push(json!({"role": "user", "content": prompt}));
            agent::run(&ctx, &base, &token, &alias, &mut conv, tools)?;
            Ok(conv.iter().rev().find(|m| m["role"] == "assistant" && m["content"].as_str().map_or(false, |c| !c.trim().is_empty())).and_then(|m| m["content"].as_str()).unwrap_or("(модель не дала ответа)").to_string())
        })();
        st.busy.store(false, Ordering::SeqCst);
        r
    })
}

/// Выбор модели с телефона: те же модели и то же переключение, что в окне программы.
struct HostControl(Arc<App>);

impl server::Control for HostControl {
    fn models(&self) -> Value {
        json!({"current": self.0.model.lock().unwrap().clone(), "ready": self.0.ready.load(Ordering::SeqCst), "models": list_models_for(&self.0)})
    }
    fn select(&self, id: &str) -> Result<(), String> {
        let h = self.0.handle.lock().unwrap().clone().ok_or("окно программы ещё не готово")?;
        set_model_for(h, &self.0, id.to_string())
    }
}

/// Для стенда проверки: фиксированный список моделей.
struct StubControl;

impl server::Control for StubControl {
    fn models(&self) -> Value {
        json!({"current": "test-model", "ready": true, "models": [{"id": "test-model", "label": "Тестовая"}, {"id": "other", "label": "Другая"}]})
    }
    fn select(&self, id: &str) -> Result<(), String> {
        if id == "other" || id == "test-model" { Ok(()) } else { Err("нет такой модели".into()) }
    }
}

fn host_status(st: &Arc<App>) -> Arc<dyn Fn() -> Value + Send + Sync> {
    let st = st.clone();
    Arc::new(move || {
        let alias = st.engine.lock().unwrap().as_ref().map(|e| e.alias.clone());
        json!({"name": "Nexora Server", "version": env!("CARGO_PKG_VERSION"), "ready": st.ready.load(Ordering::SeqCst), "busy": st.busy.load(Ordering::SeqCst), "model": alias, "tools": hostmode::Settings::load().tools})
    })
}

#[tauri::command]
fn host_state(st: State<'_, Arc<App>>) -> Value {
    st.host.state(&hostmode::Settings::load())
}

#[tauri::command]
fn host_enable(st: State<'_, Arc<App>>, bind: String, port: u16, names: Vec<String>) -> Result<Value, String> {
    if port < 1024 {
        return Err("порт должен быть от 1024 до 65535".into());
    }
    let mut set = hostmode::Settings::load();
    set.bind = bind.trim().to_string();
    set.port = port;
    set.names = names.into_iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    st.host.enable(&st.hub, mobile_exec(st.inner().clone()), host_status(st.inner()), Arc::new(HostControl(st.inner().clone())), &set)?;
    set.save();
    Ok(st.host.state(&set))
}

#[tauri::command]
fn host_disable(st: State<'_, Arc<App>>) -> Value {
    st.host.disable();
    st.host.state(&hostmode::Settings::load())
}

#[tauri::command]
fn host_window(st: State<'_, Arc<App>>) -> Result<(), String> {
    st.host.open_window()
}

#[tauri::command]
fn host_revoke(st: State<'_, Arc<App>>, serial: String) -> bool {
    st.host.revoke(&serial)
}

#[tauri::command]
fn host_trust(st: State<'_, Arc<App>>, serial: String, ok: bool) -> bool {
    st.host.trust(&serial, ok)
}

#[tauri::command]
fn host_setup(st: State<'_, Arc<App>>) -> Result<Value, String> {
    st.host.setup_info(&hostmode::Settings::load())
}

#[tauri::command]
fn host_overlay(st: State<'_, Arc<App>>, on: bool) -> Value {
    let mut set = hostmode::Settings::load();
    set.overlay = on;
    set.save();
    let r = st.host.apply_overlay(&set);
    let mut v = st.host.state(&set);
    if let Err(e) = r {
        v["overlay_error"] = json!(e);
    }
    v
}

/// Данные для доступа из любой точки: адрес VPN-слоя, строка подключения и QR.
#[tauri::command]
async fn host_remote(st: State<'_, Arc<App>>) -> Result<Value, String> {
    let a: Arc<App> = st.inner().clone();
    blocking(move || a.host.remote_info(&hostmode::Settings::load())).await
}

/// Состояние подключения этого ПК к серверу Nexora (режим «Агент»).
fn agent_state(st: &App) -> Value {
    let c = agentclient::load();
    let active = st.agent_proxy.lock().unwrap().is_some();
    json!({
        "connected": c.is_some(),
        "active": active,
        "server": c.as_ref().map(|c| json!({"host": c.server.host, "port": c.server.port, "short": pki::short(&c.server.fp)})),
        "name": c.as_ref().map(|c| c.name.clone()),
        "default_name": std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Ноутбук".into()),
        "pairing": st.agent_pair.lock().unwrap().is_some(),
    })
}

#[tauri::command]
fn agent_get(st: State<'_, Arc<App>>) -> Value {
    agent_state(&st)
}

/// Проверка строки подключения: возвращает отпечаток сервера для сверки глазами (с экраном сервера или данными привязки).
#[tauri::command]
async fn agent_probe(link: String) -> Result<Value, String> {
    blocking(move || {
        let (srv, short) = agentclient::parse_link(&link)?;
        let fp = agentclient::probe(&srv.host, srv.port)?;
        let matches = if !srv.fp.is_empty() { Some(srv.fp == fp) } else { short.map(|s| fp.starts_with(&s)) };
        Ok(json!({"fp": fp, "short": pki::short(&fp), "host": srv.host, "port": srv.port, "matches": matches}))
    })
    .await
}

fn link_to_server(link: &str, fp: Option<String>) -> Result<agentclient::ServerRef, String> {
    let (mut srv, short) = agentclient::parse_link(link)?;
    if srv.fp.is_empty() {
        let f = fp.filter(|f| f.len() == 64).ok_or("сначала проверьте отпечаток сервера: нажмите «Проверить сервер»")?;
        if !short.map_or(false, |s| f.starts_with(&s)) {
            return Err("отпечаток не совпал со строкой подключения".into());
        }
        srv.fp = f;
    }
    Ok(srv)
}

fn store_connection(srv: agentclient::ServerRef, name: String, creds: agentclient::Credentials) -> Result<(), String> {
    agentclient::save(&agentclient::Connection { server: srv, name, creds })
}

/// Привязка этого ПК кодом из Authenticator (на сервере должно быть открыто окно привязки).
#[tauri::command]
async fn agent_enroll(st: State<'_, Arc<App>>, link: String, fp: Option<String>, name: String, code: String) -> Result<Value, String> {
    let a: Arc<App> = st.inner().clone();
    blocking(move || {
        let srv = link_to_server(&link, fp)?;
        let creds = agentclient::enroll_with_code(&srv, &name, &code)?;
        store_connection(srv, name.trim().to_string(), creds)?;
        Ok(agent_state(&a))
    })
    .await
}

/// Привязка по QR: показывает QR (его сканирует уже привязанный телефон) и код сверки.
#[tauri::command]
async fn agent_pair_start(st: State<'_, Arc<App>>, link: String, fp: Option<String>, name: String) -> Result<Value, String> {
    let a: Arc<App> = st.inner().clone();
    blocking(move || {
        let srv = link_to_server(&link, fp)?;
        probe_matches(&srv)?;
        let ps = agentclient::pair_start(&srv, &name)?;
        let out = json!({"qr": hostmode::qr_svg(&ps.qr_text), "check": ps.check, "text": ps.qr_text});
        *a.agent_pair.lock().unwrap() = Some((srv, ps));
        Ok(out)
    })
    .await
}

/// Сервер должен отвечать с тем же отпечатком, который закреплён (иначе QR показывать бессмысленно).
fn probe_matches(srv: &agentclient::ServerRef) -> Result<(), String> {
    let fp = agentclient::probe(&srv.host, srv.port)?;
    if fp != srv.fp {
        return Err("сервер предъявил другой отпечаток: это не тот сервер или его сертификат пересоздан".into());
    }
    Ok(())
}

/// Опрос: подтвердил ли телефон. {"done": true} после получения сертификата.
#[tauri::command]
async fn agent_pair_poll(st: State<'_, Arc<App>>) -> Result<Value, String> {
    let a: Arc<App> = st.inner().clone();
    blocking(move || {
        let (srv, name, polled) = {
            let g = a.agent_pair.lock().unwrap();
            let Some((srv, ps)) = g.as_ref() else { return Err("привязка по QR не запущена".to_string()) };
            (srv.clone(), ps.name.clone(), agentclient::pair_poll(srv, ps))
        };
        match polled {
            Ok(Some(creds)) => {
                store_connection(srv, name, creds)?;
                *a.agent_pair.lock().unwrap() = None;
                Ok(json!({"done": true, "state": agent_state(&a)}))
            }
            Ok(None) => Ok(json!({"done": false})),
            Err(e) => {
                *a.agent_pair.lock().unwrap() = None;
                Err(e)
            }
        }
    })
    .await
}

#[tauri::command]
fn agent_pair_cancel(st: State<'_, Arc<App>>) {
    *st.agent_pair.lock().unwrap() = None;
}

/// Подключиться: поднять локальный мост и работать с моделью сервера как с внешним движком (инструменты остаются на этом ПК).
#[tauri::command]
async fn agent_connect(app: tauri::AppHandle, st: State<'_, Arc<App>>) -> Result<Value, String> {
    let a: Arc<App> = st.inner().clone();
    blocking(move || {
        let c = agentclient::load().ok_or("сервер не привязан")?;
        probe_matches(&c.server)?;
        let proxy = agentclient::start_proxy(c.server.clone(), c.creds.clone())?;
        let (url, key) = (format!("http://127.0.0.1:{}", proxy.port), proxy.token.clone());
        *a.agent_proxy.lock().unwrap() = Some(proxy);
        match connect_external(&app, &a, &url, &key, None) {
            Ok(v) => Ok(v),
            Err(e) => {
                *a.agent_proxy.lock().unwrap() = None;
                Err(format!("сервер доступен, но модель недоступна: {e}"))
            }
        }
    })
    .await
}

#[tauri::command]
fn agent_forget(st: State<'_, Arc<App>>) -> Result<Value, String> {
    *st.agent_proxy.lock().unwrap() = None;
    *st.agent_pair.lock().unwrap() = None;
    agentclient::forget()?;
    Ok(agent_state(&st))
}

#[tauri::command]
fn update_state() -> Value {
    json!({"version": env!("CARGO_PKG_VERSION"), "url": prefs::update_url(), "key_set": update::UPDATE_PUBKEY_B64.len() == 44})
}

#[tauri::command]
fn update_set_url(url: String) -> Result<(), String> {
    let u = url.trim().to_string();
    if !u.is_empty() && !u.starts_with("https://") {
        return Err("адрес должен начинаться с https://".into());
    }
    prefs::set_update_url(&u);
    Ok(())
}

/// Проверка обновления: манифест с подписью по HTTPS; ничего не скачивается и не ставится.
#[tauri::command]
async fn update_check() -> Result<Value, String> {
    blocking(move || {
        let url = prefs::update_url();
        if url.is_empty() {
            return Err("адрес обновлений не задан".to_string());
        }
        match update::check(&url, env!("CARGO_PKG_VERSION"), update::UPDATE_PUBKEY_B64, false)? {
            Some(m) => Ok(json!({"available": true, "version": m.version, "notes": m.notes})),
            None => Ok(json!({"available": false})),
        }
    })
    .await
}

/// Установка по явной просьбе: манифест проверяется заново, установщик скачивается, сверяются сумма и подпись, затем запуск.
#[tauri::command]
async fn update_install() -> Result<String, String> {
    blocking(move || {
        let url = prefs::update_url();
        let cur = env!("CARGO_PKG_VERSION");
        let m = update::check(&url, cur, update::UPDATE_PUBKEY_B64, false)?.ok_or("новой версии нет")?;
        let path = update::download(&m, &engine::data_dir().join("updates"), cur, update::UPDATE_PUBKEY_B64, false)?;
        update::launch(&path)?;
        Ok(format!("Установщик версии {} запущен: он попросит закрыть Nexora и завершит обновление.", m.version))
    })
    .await
}

/// Для издателя: новая пара ключей подписи обновлений (закрытый и открытый ключи пишутся в файлы).
fn cli_gen_key(priv_out: &str, pub_out: &str) {
    let r = (|| -> Result<String, String> {
        if std::path::Path::new(priv_out).exists() {
            return Err("файл закрытого ключа уже есть: не перезаписываю".into());
        }
        let (privk, pubk) = update::gen_keypair()?;
        std::fs::write(priv_out, &privk).map_err(|e| e.to_string())?;
        std::fs::write(pub_out, &pubk).map_err(|e| e.to_string())?;
        Ok(format!("ok {pubk}"))
    })();
    let _ = std::fs::write(format!("{pub_out}.log"), r.unwrap_or_else(|e| format!("ошибка: {e}")));
}

/// Для издателя: подписать установщик, манифест записывается в файл.
fn cli_sign_update(a: &[String]) {
    // a: установщик, версия, адрес установщика, файл закрытого ключа, файл манифеста, [описание]
    let r = (|| -> Result<(), String> {
        let key = std::fs::read_to_string(&a[3]).map_err(|e| format!("не прочитан закрытый ключ: {e}"))?;
        let m = update::sign_release(std::path::Path::new(&a[0]), &a[1], &a[2], a.get(5).map(|s| s.as_str()).unwrap_or(""), &key)?;
        std::fs::write(&a[4], m).map_err(|e| e.to_string())
    })();
    let _ = std::fs::write(format!("{}.log", a[4]), r.map(|_| "ok".to_string()).unwrap_or_else(|e| format!("ошибка: {e}")));
}

/// Включение сервера из настроек (общий путь для кнопки и автозапуска).
fn host_enable_from_settings(st: &Arc<App>) -> Result<(), String> {
    let set = hostmode::Settings::load();
    st.host.enable(&st.hub, mobile_exec(st.clone()), host_status(st), Arc::new(HostControl(st.clone())), &set)
}

/// Запуск вместе с Windows: модель стартует сама, сервер включается, если так настроено. Сеть после загрузки Windows
/// поднимается не сразу (адрес ещё не выдан), поэтому включение пробуется повторно до 5 минут.
fn background_boot(app: tauri::AppHandle, st: Arc<App>) {
    std::thread::spawn(move || {
        let _ = start_engine(app.clone(), st.clone());
        if !hostmode::Settings::load().autorun {
            return;
        }
        for attempt in 0..30 {
            if st.host.running() {
                return;
            }
            match host_enable_from_settings(&st) {
                Ok(()) => return,
                Err(e) => {
                    if attempt == 29 {
                        let _ = app.emit("host-autostart-failed", json!({"error": e}));
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
            }
        }
    });
}

#[tauri::command]
fn desktop_get() -> Value {
    let (on, here) = desktop::autostart_get();
    json!({"tray": prefs::tray(), "autostart": on, "autostart_here": here, "server_autorun": hostmode::Settings::load().autorun, "exe": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(), "tray_icon": desktop::tray_icon_candidates().into_iter().find(|p| p.is_file()).map(|p| p.display().to_string())})
}

#[tauri::command]
fn desktop_set_tray(on: bool) {
    prefs::set_tray(on);
}

#[tauri::command]
fn desktop_set_autostart(on: bool) -> Result<Value, String> {
    desktop::autostart_set(on)?;
    Ok(desktop_get())
}

#[tauri::command]
fn desktop_set_server_autorun(on: bool) -> Value {
    let mut set = hostmode::Settings::load();
    set.autorun = on;
    set.save();
    desktop_get()
}

#[tauri::command]
fn host_visible(st: State<'_, Arc<App>>, on: bool) -> Value {
    let mut set = hostmode::Settings::load();
    set.visible = on;
    set.save();
    let r = st.host.apply_visibility(&set);
    let mut v = st.host.state(&set);
    if let Err(e) = r {
        v["visible_error"] = json!(e);
    }
    v
}

/// Найти серверы Nexora в локальной сети (слушает объявления несколько секунд).
#[tauri::command]
async fn discover_browse() -> Result<Vec<Value>, String> {
    blocking(move || discover::browse(std::time::Duration::from_secs(4)).map(|v| v.into_iter().map(|f| json!({"name": f.name, "host": f.host, "port": f.port, "fp": f.fp})).collect())).await
}

#[tauri::command]
fn host_tools(st: State<'_, Arc<App>>, on: bool) -> Value {
    let mut set = hostmode::Settings::load();
    set.tools = on;
    set.save();
    st.host.state(&set)
}

/// Стенд для проверки телефонного клиента: сервер с «ненастоящим» исполнителем (без модели). Данные только во временной папке
/// (задайте LOCALAPPDATA). Окно привязки держится открытым. Записывает в файл строку подключения и секрет Authenticator.
fn selftest_host(port: u16, out: &str) {
    let hub = mobile::Hub::new(Arc::new(Approvals::default()));
    let exec: mobile::Exec = Arc::new(|hub: &Arc<mobile::Hub>, id: u64, prompt: &str, cancel: Arc<AtomicBool>| -> Result<String, String> {
        let hist = hub.history_of(id).len();
        if prompt.contains("долго") {
            for _ in 0..600 {
                if cancel.load(Ordering::SeqCst) {
                    return Err("отменено".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        if prompt.contains("удали") || prompt.contains("запиши") {
            let (aid, rx) = hub.approvals.create();
            let kind = if prompt.contains("удали") { "delete" } else { "write" };
            hub.emit(id, "tool", json!({"id": "t1", "name": "x", "status": "running", "title": "действие…"}));
            hub.emit(id, "approval", json!({"id": aid, "kind": kind, "title": "Файл a.txt", "detail": "подробности"}));
            if !rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap_or(false) {
                return Err("отклонено".into());
            }
            hub.emit(id, "tool", json!({"id": "t1", "name": "x", "status": "ok", "title": "Файл a.txt", "output": "готово"}));
        }
        for part in ["Ответ", " на ", prompt, &format!(" (история: {hist})")] {
            hub.emit(id, "chat-token", json!({"kind": "content", "text": part}));
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
        Ok(format!("Ответ на {prompt} (история: {hist})"))
    });
    let host = hostmode::Host::default();
    let mut set = hostmode::Settings::load();
    set.bind = "127.0.0.1".into();
    set.port = port;
    let status: Arc<dyn Fn() -> Value + Send + Sync> = Arc::new(|| json!({"name": "Nexora Server", "version": "test", "ready": true, "busy": false, "model": "test-model", "tools": false}));
    if let Err(e) = host.enable(&hub, exec, status, Arc::new(StubControl), &set) {
        eprintln!("host: {e}");
        std::process::exit(1);
    }
    let info = host.setup_info(&set).expect("setup");
    let _ = std::fs::write(out, json!({"link": info["link"], "secret": info["secret"]}).to_string());
    loop {
        let _ = host.open_window();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--gen-update-key" {
        return cli_gen_key(&args[2], &args[3]);
    }
    if args.len() >= 7 && args[1] == "--sign-update" {
        return cli_sign_update(&args[2..]);
    }
    // стенд проверки включается только явно: он держит окно привязки открытым и пишет секрет Authenticator в файл
    if args.len() == 4 && args[1] == "--selftest-host" && std::env::var("NEXORA_SELFTEST").is_ok() {
        return selftest_host(args[2].parse().unwrap_or(0), &args[3]);
    }
    if args.len() == 4 && args[1] == "--selftest-net" {
        return selftest_net(&args[2], &args[3]);
    }
    if args.len() == 5 && args[1] == "--selftest-vision" {
        return selftest_vision(&args[2], &args[3], &args[4]);
    }
    if args.len() == 4 && args[1] == "--selftest-dl" {
        return selftest_dl(&args[2], &args[3]);
    }
    if args.len() == 3 && args[1] == "--selftest-hw" {
        return selftest_hw(&args[2]);
    }
    if args.len() == 4 && args[1] == "--selftest-ext" {
        return selftest_ext(&args[2], &args[3]);
    }
    if args.len() == 3 && args[1] == "--selftest-agent" {
        return selftest_agent(&args[2]);
    }
    let state = Arc::new(App::new());
    let st2 = state.clone();
    let background = std::env::args().any(|a| a == "--background");
    tauri::Builder::default()
        // повторный запуск (ярлык, автозапуск) не поднимает вторую копию с моделью, а показывает уже работающую
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| desktop::show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && prefs::tray() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .manage(state.clone())
        .setup(move |app| {
            *state.handle.lock().unwrap() = Some(app.handle().clone());
            if let Err(e) = desktop::setup_tray(app.handle()) {
                eprintln!("значок в трее не создан: {e}");
            }
            if background {
                // запуск вместе с Windows: окна нет, всё поднимается само
                background_boot(app.handle().clone(), state.clone());
            } else if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
            let t = title_for(&state.profile.lock().unwrap());
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(t);
            }
            // внешние MCP-серверы из настроек запускаются в фоне
            std::thread::spawn(mcp::start_enabled);
            // планировщик задач: раз в 20 секунд проверяет, не пора ли выполнить задачу
            {
                let (h, s) = (app.handle().clone(), state.clone());
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(20));
                    let (now, local) = (totp::now_secs(), tasks::local_now());
                    if let Some(t) = tasks::default_store().list().into_iter().find(|t| tasks::is_due(t, now, &local)) {
                        run_task(&h, &s, t);
                    }
                });
            }
            // отладочный ход для автоматической проверки интерфейса: выполняет JS из файла, указанного в NEXORA_DEV_EVAL (только в отладочной сборке)
            #[cfg(debug_assertions)]
            if let Ok(path) = std::env::var("NEXORA_DEV_EVAL") {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    if let (Ok(js), Some(w)) = (std::fs::read_to_string(&path), h.get_webview_window("main")) {
                        let _ = w.eval(&js);
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_start, get_prefs, set_think, set_web, chats_list, chat_open, chat_delete, chat_rename, mcp_list, mcp_preview, mcp_import, mcp_toggle, mcp_remove, mcp_restart, tasks_list, task_save, task_delete, task_toggle, task_run_now, task_result, host_state, host_enable, host_disable, host_window, host_revoke, host_setup, host_trust, host_tools, desktop_get, desktop_set_tray, desktop_set_autostart, desktop_set_server_autorun, update_state, update_set_url, update_check, update_install, agent_get, agent_probe, agent_enroll, agent_pair_start, agent_pair_poll, agent_pair_cancel, agent_connect, agent_forget, host_visible, host_overlay, host_remote, discover_browse, set_chat_trust, memory_list, memory_delete, remote_list, remote_save, remote_delete, remote_use, remote_scan, remote_trust, remote_test, chat_send, chat_stop, chat_reset, get_state, set_workspace, set_mode, fs_list, approve,
            new_project, set_auto_folder, set_projects_root, list_models, set_auto_mode, app_info, hf_search, hf_files, hf_add, model_delete, plugin_preview, plugin_install, plugin_list, plugin_toggle, plugin_remove, plugin_search, agents_list, set_role, get_catalog, model_download, model_cancel, set_ctx, ctx_info, set_model, use_external, use_builtin, detect_engines, set_profile
        ])
        .build(tauri::generate_context!())
        .expect("failed to build app")
        .run(move |_h, ev| {
            if let RunEvent::Exit = ev {
                mcp::stop_all();
                browser::close();
                stop_engine(&st2);
            }
        });
}
