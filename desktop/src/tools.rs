//! Инструменты агента и их безопасность: пути только внутри проекта, секретные файлы закрыты,
//! опасные команды запрещены, команды делятся на чтение / обычные / удаление.
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn schemas() -> Value {
    json!([
      {"type":"function","function":{"name":"list_dir","description":"Список файлов и папок. path относительно папки проекта (по умолчанию корень) или абсолютный путь к любой папке на компьютере; вне проекта каждый раз спрашивается подтверждение пользователя.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":[]}}},
      {"type":"function","function":{"name":"read_file","description":"Прочитать текстовый файл целиком (до 200 КБ). path относительно проекта или абсолютный путь на любом диске; вне проекта каждый раз спрашивается подтверждение.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
      {"type":"function","function":{"name":"write_file","description":"Создать файл или полностью перезаписать. Для небольших правок существующих файлов используй edit_file.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}},
      {"type":"function","function":{"name":"edit_file","description":"Заменить ровно одно вхождение old_string на new_string. old_string должен быть уникальным: включай соседние строки для контекста. Текст берётся из read_file без изменений.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"}},"required":["path","old_string","new_string"]}}},
      {"type":"function","function":{"name":"delete_file","description":"Удалить файл. Всегда требует подтверждения пользователя.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
      {"type":"function","function":{"name":"use_skill","description":"Прочитать инструкции установленного навыка (скилла) по имени из списка доступных навыков. Вызывай, когда задача подходит под описание навыка.",
        "parameters":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}},
      {"type":"function","function":{"name":"read_skill_file","description":"Прочитать файл, лежащий рядом со скиллом (шаблон, справка).",
        "parameters":{"type":"object","properties":{"name":{"type":"string"},"path":{"type":"string"}},"required":["name","path"]}}},
      {"type":"function","function":{"name":"open_url","description":"Открыть адрес http(s) в браузере пользователя по умолчанию.",
        "parameters":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}}},
      {"type":"function","function":{"name":"start_app","description":"Запустить программу. background=false: обычное окно (IDE, браузер, эмулятор); background=true: консольный процесс без окна (сервер разработки), вывод пишется в лог. Возвращает номер процесса (pid).",
        "parameters":{"type":"object","properties":{"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"background":{"type":"boolean"}},"required":["program"]}}},
      {"type":"function","function":{"name":"list_apps","description":"Список программ, запущенных этим агентом, с состоянием и хвостом лога.",
        "parameters":{"type":"object","properties":{},"required":[]}}},
      {"type":"function","function":{"name":"stop_app","description":"Остановить программу, запущенную этим агентом (по pid), вместе с дочерними процессами.",
        "parameters":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}}},
      {"type":"function","function":{"name":"run_skill_script","description":"Запустить скрипт, установленный вместе с навыком (путь указан в тексте навыка после use_skill). path задаётся от папки навыка, args это список аргументов. Работает в папке проекта. Первый запуск каждого скрипта за сеанс подтверждает пользователь.",
        "parameters":{"type":"object","properties":{"skill":{"type":"string"},"path":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"timeout_sec":{"type":"integer"}},"required":["skill","path"]}}},
      {"type":"function","function":{"name":"mcp_list","description":"Показать подключённые MCP-серверы: имя, состояние, список инструментов.",
        "parameters":{"type":"object","properties":{},"required":[]}}},
      {"type":"function","function":{"name":"mcp_add","description":"Добавить и запустить внешний MCP-сервер, когда пользователь просит его установить. name: латиницей в нижнем регистре; command и args как в инструкции сервера (например command npx, args [\"-y\", \"пакет\"], или uvx). env: переменные окружения, только если пользователь сам дал значения. Требует подтверждения пользователя. После добавления инструменты сервера доступны сразу, в этом же ответе.",
        "parameters":{"type":"object","properties":{"name":{"type":"string"},"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"env":{"type":"object"}},"required":["name","command"]}}},
      {"type":"function","function":{"name":"mcp_remove","description":"Удалить MCP-сервер по имени (с подтверждением пользователя).",
        "parameters":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}},
      {"type":"function","function":{"name":"browser_open","description":"Открыть адрес http(s) в управляемом браузере (отдельный профиль без паролей пользователя, окно видимое) и получить текст страницы и пронумерованные элементы. Для проверки сайтов, веб-интерфейсов, работы приложений в браузере.",
        "parameters":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}}},
      {"type":"function","function":{"name":"browser_read","description":"Снова прочитать текущую страницу управляемого браузера: текст и пронумерованные элементы (ссылки, кнопки, поля). Номера меняются после каждого чтения.",
        "parameters":{"type":"object","properties":{},"required":[]}}},
      {"type":"function","function":{"name":"browser_click","description":"Нажать элемент страницы по номеру из последнего ответа браузера. Возвращает обновлённую страницу.",
        "parameters":{"type":"object","properties":{"index":{"type":"integer"}},"required":["index"]}}},
      {"type":"function","function":{"name":"browser_type","description":"Ввести текст в поле по номеру (для выпадающего списка: выбрать вариант по тексту). submit=true нажимает Enter после ввода. Пароли и секреты не вводи без прямой просьбы пользователя.",
        "parameters":{"type":"object","properties":{"index":{"type":"integer"},"text":{"type":"string"},"submit":{"type":"boolean"}},"required":["index","text"]}}},
      {"type":"function","function":{"name":"browser_screenshot","description":"Сохранить снимок страницы в PNG и вернуть путь к файлу (содержимое картинки модель не видит; чтобы понять страницу, используй browser_read).",
        "parameters":{"type":"object","properties":{},"required":[]}}},
      {"type":"function","function":{"name":"browser_close","description":"Закрыть управляемый браузер.",
        "parameters":{"type":"object","properties":{},"required":[]}}},
      {"type":"function","function":{"name":"registry_read","description":"Прочитать реестр Windows (только чтение, без вопросов): значения раздела и подразделы, либо одно значение (name). path вида HKLM\\SOFTWARE\\... или HKCU\\Software\\... (поддерживаются HKLM, HKCU, HKCR, HKU). Значения с именами вроде password/token скрыты.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"name":{"type":"string"}},"required":["path"]}}},
      {"type":"function","function":{"name":"registry_write","description":"Записать значение реестра. ВСЕГДА требует подтверждения пользователя (показывается прежнее и новое значение). type: String, ExpandString, DWord, QWord или MultiString (строки через перевод строки). Для HKLM нужны права администратора: elevated=true (окно UAC). Защитные разделы (Defender, SAM, SafeBoot, Lsa, Winlogon) закрыты.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"name":{"type":"string"},"type":{"type":"string"},"value":{"type":"string"},"elevated":{"type":"boolean"}},"required":["path","name","value"]}}},
      {"type":"function","function":{"name":"registry_delete","description":"Удалить значение реестра (name) или целый раздел (без name, не выше третьего уровня). Всегда с подтверждением пользователя. elevated=true для HKLM.",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"name":{"type":"string"},"elevated":{"type":"boolean"}},"required":["path"]}}},
      {"type":"function","function":{"name":"need_tools","description":"Вызови, если для ответа нужны файлы, команды или другие инструменты. Без неё доступны только ответы текстом.",
        "parameters":{"type":"object","properties":{},"required":[]}}},
      {"type":"function","function":{"name":"list_skills","description":"Найти установленные навыки по теме и показать их имена с описаниями.",
        "parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":[]}}},
      {"type":"function","function":{"name":"web_search","description":"Найти информацию в интернете по запросу: названия, ссылки и краткие описания страниц.",
        "parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}},
      {"type":"function","function":{"name":"web_fetch","description":"Прочитать текст веб-страницы по адресу http(s) (обычно из результатов web_search).",
        "parameters":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}}},
      {"type":"function","function":{"name":"remember","description":"Запомнить короткий устойчивый факт (до 300 символов) о проекте или о предпочтениях пользователя: как запускать тесты, где что лежит, правила. Сохраняется сразу, без вопросов (пользователь видит карточку и может удалить заметку в Настройках). Не записывай секреты, пароли, токены и сиюминутные детали. scope: project (по умолчанию, только этот проект) или global (для всех проектов).",
        "parameters":{"type":"object","properties":{"text":{"type":"string"},"scope":{"type":"string"}},"required":["text"]}}},
      {"type":"function","function":{"name":"forget","description":"Удалить заметку памяти по номеру (id из списка заметок), когда она устарела или неверна.",
        "parameters":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}},
      {"type":"function","function":{"name":"find_apps","description":"Найти установленные программы на компьютере по названию (реестр, меню «Пуск», Program Files). Только чтение. Возвращает пути к exe и папкам установки.",
        "parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}},
      {"type":"function","function":{"name":"search_files","description":"Поиск файлов по всем дискам или в указанной папке (абсолютный путь): по части имени, расширению, минимальному размеру. Только имена и пути, содержимое не читается; секретные файлы скрыты.",
        "parameters":{"type":"object","properties":{"query":{"type":"string"},"ext":{"type":"string"},"min_size_mb":{"type":"number"},"path":{"type":"string"},"max_results":{"type":"integer"}},"required":[]}}},
      {"type":"function","function":{"name":"run_elevated","description":"Выполнить команду PowerShell С ПРАВАМИ АДМИНИСТРАТОРА (служба, системная папка). Каждый раз требует подтверждения в Nexora и в окне Windows UAC. Используй только когда обычной команды не хватает.",
        "parameters":{"type":"object","properties":{"command":{"type":"string"},"timeout_sec":{"type":"integer"}},"required":["command"]}}},
      {"type":"function","function":{"name":"run_command","description":"Выполнить команду PowerShell в папке проекта (запуск скриптов, тестов, git). cwd: другая рабочая папка (абсолютный путь), вне проекта каждый раз спрашивается подтверждение. Вывод возвращается текстом.",
        "parameters":{"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"},"timeout_sec":{"type":"integer"}},"required":["command"]}}}
    ])
}

/// Путь внутри папки проекта. Выход за пределы (.., абсолютный путь, ссылка наружу) запрещён.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    let joined = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    return Err("путь выходит за пределы папки проекта".into());
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    let croot = root.canonicalize().map_err(|e| e.to_string())?;
    let mut anc = out.as_path();
    while !anc.exists() {
        anc = anc.parent().ok_or("недопустимый путь")?;
    }
    let canc = anc.canonicalize().map_err(|e| e.to_string())?;
    if !canc.starts_with(&croot) {
        return Err("путь вне папки проекта".into());
    }
    Ok(out)
}

pub fn is_secret(path: &Path) -> bool {
    let n = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    n.starts_with(".env")
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.ends_with(".pfx")
        || n.starts_with("id_rsa")
        || n.starts_with("id_ed25519")
        || n.contains("secret")
        || n.contains("credential")
}

/// Для изменения файла (запись, правка, удаление): то же, что `checked`, плюс запрет на защищённые модули Nexora.
fn checked_mut(root: &Path, args: &Value, key: &str) -> Result<PathBuf, String> {
    let p = checked(root, args, key)?;
    if let Some(why) = crate::selfdev::protected(&p) {
        return Err(why);
    }
    Ok(p)
}

fn checked(root: &Path, args: &Value, key: &str) -> Result<PathBuf, String> {
    let rel = args[key].as_str().ok_or_else(|| format!("не указан параметр {key}"))?;
    let p = resolve(root, rel)?;
    if is_secret(&p) {
        return Err("доступ к секретным файлам закрыт (.env, ключи, учётные данные)".into());
    }
    Ok(p)
}

/// Путь без привязки к проекту: второй элемент = лежит ли он вне папки проекта (тогда нужно подтверждение).
pub fn resolve_free(root: &Path, rel: &str) -> Result<(PathBuf, bool), String> {
    let p = Path::new(rel);
    let joined = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    return Err("недопустимый путь".into());
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    let croot = root.canonicalize().map_err(|e| e.to_string())?;
    let mut anc = out.as_path();
    while !anc.exists() {
        anc = anc.parent().ok_or("недопустимый путь")?;
    }
    let outside = !anc.canonicalize().map_err(|e| e.to_string())?.starts_with(&croot);
    Ok((out, outside))
}

pub fn list_dir(root: &Path, args: &Value) -> Result<String, String> {
    let p = resolve(root, args["path"].as_str().unwrap_or("."))?;
    list_path(&p)
}

pub fn list_path(p: &Path) -> Result<String, String> {
    let mut items: Vec<(bool, String)> = std::fs::read_dir(&p)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| (e.path().is_dir(), e.file_name().to_string_lossy().to_string()))
        .filter(|(_, n)| !matches!(n.as_str(), ".git" | "node_modules" | "target" | "__pycache__" | ".venv"))
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let s: Vec<String> = items.iter().take(300).map(|(d, n)| if *d { format!("{n}/") } else { n.clone() }).collect();
    Ok(if s.is_empty() { "(пусто)".into() } else { s.join("\n") })
}

pub fn read_file(root: &Path, args: &Value) -> Result<String, String> {
    let p = checked(root, args, "path")?;
    read_path(&p)
}

pub fn read_path(p: &Path) -> Result<String, String> {
    if is_secret(p) {
        return Err("доступ к секретным файлам закрыт (.env, ключи, учётные данные)".into());
    }
    let p = p.to_path_buf();
    let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    if meta.len() > 200_000 {
        return Err("файл больше 200 КБ".into());
    }
    let b = std::fs::read(&p).map_err(|e| e.to_string())?;
    if b.contains(&0) {
        return Err("бинарный файл".into());
    }
    Ok(String::from_utf8_lossy(&b).to_string())
}

/// (путь, старое содержимое или None, новое содержимое)
pub fn plan_write(root: &Path, args: &Value) -> Result<(PathBuf, Option<String>, String), String> {
    let p = checked_mut(root, args, "path")?;
    let content = args["content"].as_str().ok_or("не указан content")?.to_string();
    if content.len() > 2_000_000 {
        return Err("содержимое больше 2 МБ".into());
    }
    let old = std::fs::read_to_string(&p).ok();
    Ok((p, old, content))
}

/// Заменить ровно одно вхождение (учитывает CRLF). Общая логика для локальных и удалённых файлов.
pub fn apply_edit(old: &str, from: &str, to: &str) -> Result<String, String> {
    let (mut from, mut to) = (from.to_string(), to.to_string());
    if from.is_empty() {
        return Err("old_string пуст".into());
    }
    if old.contains("\r\n") && !from.contains("\r\n") {
        from = from.replace('\n', "\r\n");
        to = to.replace('\n', "\r\n");
    }
    let n = old.matches(&from).count();
    if n == 0 {
        return Err("old_string не найден в файле: перечитай файл и скопируй фрагмент точно".into());
    }
    if n > 1 {
        return Err(format!("old_string найден {n} раз: добавь соседние строки, чтобы он был уникальным"));
    }
    Ok(old.replacen(&from, &to, 1))
}

pub fn plan_edit(root: &Path, args: &Value) -> Result<(PathBuf, String, String), String> {
    let p = checked_mut(root, args, "path")?;
    let old = std::fs::read_to_string(&p).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
    let from = args["old_string"].as_str().ok_or("не указан old_string")?;
    let to = args["new_string"].as_str().ok_or("не указан new_string")?;
    let new = apply_edit(&old, from, to)?;
    Ok((p, old, new))
}

pub fn apply_write(path: &Path, content: &str) -> Result<(), String> {
    crate::selfdev::backup(path); // копия перед правкой, если это исходники Nexora
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())
}

pub fn delete_file(root: &Path, args: &Value) -> Result<PathBuf, String> {
    let p = checked_mut(root, args, "path")?;
    if !p.is_file() {
        return Err("это не файл (удаление папок агенту недоступно)".into());
    }
    Ok(p)
}

/// Простой diff: общие начало и конец отрезаются, показываются изменённые строки с контекстом.
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut p = 0;
    while p < a.len() && p < b.len() && a[p] == b[p] {
        p += 1;
    }
    let mut s = 0;
    while s < a.len() - p && s < b.len() - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        s += 1;
    }
    let mut out = vec![];
    for l in &a[p.saturating_sub(3)..p] {
        out.push(format!("  {l}"));
    }
    for l in &a[p..a.len() - s] {
        out.push(format!("- {l}"));
    }
    for l in &b[p..b.len() - s] {
        out.push(format!("+ {l}"));
    }
    for l in &a[a.len() - s..(a.len() - s + 3.min(s))] {
        out.push(format!("  {l}"));
    }
    if out.len() > 200 {
        out.truncate(200);
        out.push("… (показаны первые 200 строк)".into());
    }
    out.join("\n")
}

#[derive(Debug, PartialEq)]
pub enum CmdClass {
    Blocked(&'static str),
    ReadOnly,
    Delete,
    Normal,
}

pub fn classify(cmd: &str) -> CmdClass {
    let c = cmd.to_lowercase();
    const BLOCK: &[(&str, &str)] = &[
        ("format-volume", "форматирование дисков"),
        ("format c:", "форматирование дисков"),
        ("diskpart", "работа с разделами дисков"),
        ("shutdown", "выключение компьютера"),
        ("restart-computer", "перезагрузка компьютера"),
        ("stop-computer", "выключение компьютера"),
        ("bcdedit", "изменение загрузчика"),
        ("reg delete", "изменение реестра"),
        ("reg add", "изменение реестра"),
        ("net user", "изменение учётных записей"),
        ("cipher /w", "стирание диска"),
        ("vssadmin", "удаление теневых копий"),
        ("invoke-expression", "выполнение произвольного кода"),
        ("downloadstring", "загрузка и запуск кода из сети"),
        ("rm -rf /", "удаление корня"),
        ("rm -rf ~", "удаление домашней папки"),
        ("mkfs", "форматирование дисков"),
    ];
    for (k, why) in BLOCK {
        if c.contains(k) {
            return CmdClass::Blocked(why);
        }
    }
    let tokens: Vec<&str> = c.split(|ch: char| ch.is_whitespace() || ";|&()".contains(ch)).filter(|t| !t.is_empty()).collect();
    const DEL: &[&str] = &["rm", "del", "erase", "rd", "rmdir", "remove-item", "ri", "unlink"];
    if tokens.iter().any(|t| DEL.contains(t)) || c.contains("git clean") || c.contains("reset --hard") {
        return CmdClass::Delete;
    }
    if c.contains("iex ") || c.contains("| iex") || c.contains("|iex") {
        return CmdClass::Blocked("выполнение произвольного кода");
    }
    let bad_ops = c.contains(';') || c.contains('&') || c.contains('>') || c.contains('<') || c.contains('`') || c.contains("$(");
    if !bad_ops {
        const SAFE: &[&str] = &["dir", "ls", "cat", "type", "get-content", "gc", "get-childitem", "gci", "pwd", "echo", "where", "findstr",
            "select-string", "sls", "get-location", "tree", "head", "tail", "grep", "rg"];
        let all_safe = c.split('|').all(|seg| {
            let mut w = seg.split_whitespace();
            match w.next() {
                Some("git") => matches!(w.next(), Some("status" | "diff" | "log" | "show")),
                Some(f) => SAFE.contains(&f),
                None => false,
            }
        });
        if all_safe {
            return CmdClass::ReadOnly;
        }
    }
    CmdClass::Normal
}

fn clip(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 12_000 {
        return s.to_string();
    }
    let head: String = chars[..6000].iter().collect();
    let tail: String = chars[chars.len() - 4000..].iter().collect();
    format!("{head}\n… (вывод сокращён) …\n{tail}")
}

pub fn run_command(root: &Path, cmd: &str, timeout: u64) -> String {
    let mut c;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("$OutputEncoding=[Console]::OutputEncoding=[Text.Encoding]::UTF8; {cmd}"));
        c.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        c = Command::new("sh");
        c.args(["-c", cmd]);
    }
    c.current_dir(root)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return format!("не удалось запустить: {e}"),
    };
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || { let mut b = vec![]; let _ = so.read_to_end(&mut b); b });
    let t2 = std::thread::spawn(move || { let mut b = vec![]; let _ = se.read_to_end(&mut b); b });
    let deadline = Instant::now() + Duration::from_secs(timeout.clamp(1, 600));
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };
    let out = String::from_utf8_lossy(&t1.join().unwrap_or_default()).to_string();
    let err = String::from_utf8_lossy(&t2.join().unwrap_or_default()).to_string();
    let mut r = String::new();
    if !out.is_empty() { r.push_str(&out); }
    if !err.is_empty() { r.push_str(&format!("\n[stderr]\n{err}")); }
    if timed_out { r.push_str(&format!("\n[прервано по таймауту {timeout} с]")); }
    else { r.push_str(&format!("\n[код выхода: {}]", code.map(|c| c.to_string()).unwrap_or("?".into()))); }
    clip(r.trim())
}


// ---------- запуск приложений и браузера ----------
#[derive(Clone, Debug)]
pub struct AppProc {
    pub pid: u32,
    pub cmd: String,
    pub log: Option<PathBuf>,
}

pub fn check_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("разрешены только адреса http:// и https://".into());
    }
    if u.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"') {
        return Err("в адресе недопустимые символы".into());
    }
    Ok(())
}

/// Открывает адрес в браузере по умолчанию (без оболочки, поэтому & и ? в адресе безопасны).
pub fn open_url(url: &str) -> Result<(), String> {
    check_url(url)?;
    #[cfg(windows)]
    let r = Command::new("explorer.exe").arg(url.trim()).spawn();
    #[cfg(not(windows))]
    let r = Command::new("xdg-open").arg(url.trim()).spawn();
    r.map(|_| ()).map_err(|e| format!("не удалось открыть: {e}"))
}

pub fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn arg_line(args: &[String]) -> String {
    args.iter().map(|a| format!("\"{}\"", a.replace('"', "\\\""))).collect::<Vec<_>>().join(" ")
}

pub fn app_text(prog: &str, args: &[String]) -> String {
    if args.is_empty() { prog.to_string() } else { format!("{prog} {}", args.join(" ")) }
}

/// Запускает приложение. background=false: обычное окно (IDE, браузер, эмулятор), возвращается pid.
/// background=true: консольный процесс без окна (сервер разработки, сборка в фоне) с записью вывода в лог.
pub fn start_app(root: &Path, prog: &str, args: &[String], background: bool, log_dir: &Path) -> Result<AppProc, String> {
    let text = app_text(prog, args);
    if background {
        std::fs::create_dir_all(log_dir).map_err(|e| e.to_string())?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let log = log_dir.join(format!("app-{stamp}.log"));
        let f = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let f2 = f.try_clone().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let child = {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd");
            c.arg("/C").raw_arg(&text).creation_flags(0x0800_0000 | 0x0000_0200);
            c.current_dir(root).env("PYTHONIOENCODING", "utf-8").stdin(Stdio::null()).stdout(f).stderr(f2).spawn()
        };
        #[cfg(not(windows))]
        let child = Command::new("sh").args(["-c", &text]).current_dir(root).stdin(Stdio::null()).stdout(f).stderr(f2).spawn();
        let child = child.map_err(|e| format!("не удалось запустить: {e}"))?;
        return Ok(AppProc { pid: child.id(), cmd: text, log: Some(log) });
    }
    let mut script = format!("$p = Start-Process -FilePath '{}'", ps_quote(prog));
    if !args.is_empty() {
        script.push_str(&format!(" -ArgumentList '{}'", ps_quote(&arg_line(args))));
    }
    script.push_str(&format!(" -WorkingDirectory '{}' -PassThru; $p.Id", ps_quote(&root.display().to_string())));
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("не удалось запустить: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().map_err(|_| "не удалось получить номер процесса".to_string())?;
    Ok(AppProc { pid, cmd: text, log: None })
}

pub fn pid_alive(pid: u32) -> bool {
    let mut c = Command::new("tasklist");
    c.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))).unwrap_or(false)
}

/// Завершает процесс вместе с потомками.
pub fn kill_tree(pid: u32) -> bool {
    let mut c = Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn tail_log(p: &Path, max: usize) -> String {
    let s = String::from_utf8_lossy(&std::fs::read(p).unwrap_or_default()).to_string();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max { s } else { chars[chars.len() - max..].iter().collect() }
}

#[cfg(test)]
mod app_tests {
    use super::*;

    #[test]
    fn url_rules() {
        assert!(check_url("https://example.com/a?b=1&c=2").is_ok());
        assert!(check_url("http://127.0.0.1:8080").is_ok());
        assert!(check_url("file:///C:/Windows/win.ini").is_err());
        assert!(check_url("javascript:alert(1)").is_err());
        assert!(check_url("https://a.com/ b").is_err());
        assert!(check_url("ftp://x").is_err());
    }

    #[test]
    fn background_app_lifecycle() {
        let d = std::env::temp_dir();
        let a: Vec<String> = vec!["-n".into(), "30".into(), "127.0.0.1".into()];
        let p = start_app(&d, "ping", &a, true, &d.join("lc_test_logs")).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        assert!(pid_alive(p.pid), "процесс должен работать");
        assert!(tail_log(p.log.as_ref().unwrap(), 500).to_lowercase().contains("127.0.0.1"));
        assert!(kill_tree(p.pid));
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!pid_alive(p.pid), "процесс должен быть остановлен");
    }

    #[test]
    fn gui_app_gets_real_pid() {
        let d = std::env::temp_dir();
        let p = start_app(&d, "notepad.exe", &[], false, &d).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        assert!(pid_alive(p.pid), "окно приложения должно работать");
        assert!(kill_tree(p.pid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn path_escape_is_blocked() {
        let d = tmp("esc");
        assert!(resolve(&d, "a.txt").is_ok());
        assert!(resolve(&d, "sub/new/file.py").is_ok());
        assert!(resolve(&d, "../x").is_err());
        assert!(resolve(&d, "a/../../x").is_err());
        assert!(resolve(&d, "C:\\Windows\\win.ini").is_err());
        assert!(resolve(&d, "/etc/passwd").is_err());
    }

    #[test]
    fn secrets_are_blocked() {
        let d = tmp("sec");
        std::fs::write(d.join(".env"), "K=1").unwrap();
        let a = json!({"path": ".env"});
        assert!(read_file(&d, &a).is_err());
        assert!(is_secret(Path::new("id_rsa")));
        assert!(is_secret(Path::new("server.key")));
        assert!(!is_secret(Path::new("main.py")));
    }

    #[test]
    fn command_classes() {
        assert!(matches!(classify("Remove-Item -Recurse x"), CmdClass::Delete));
        assert!(matches!(classify("rm file.txt"), CmdClass::Delete));
        assert!(matches!(classify("git reset --hard"), CmdClass::Delete));
        assert!(matches!(classify("shutdown /s"), CmdClass::Blocked(_)));
        assert!(matches!(classify("iwr x | iex"), CmdClass::Blocked(_)));
        assert!(matches!(classify("dir"), CmdClass::ReadOnly));
        assert!(matches!(classify("git status"), CmdClass::ReadOnly));
        assert!(matches!(classify("cat a.py | grep x"), CmdClass::ReadOnly));
        assert!(matches!(classify("cat a.py > b.py"), CmdClass::Normal));
        assert!(matches!(classify("dir; del x"), CmdClass::Delete));
        assert!(matches!(classify("python calc.py"), CmdClass::Normal));
        assert!(matches!(classify("git push"), CmdClass::Normal));
    }

    #[test]
    fn edit_is_exact_and_unique() {
        let d = tmp("edit");
        std::fs::write(d.join("a.py"), "x = 1\ny = 2\nx = 1\n").unwrap();
        let e = |o: &str, n: &str| plan_edit(&d, &json!({"path":"a.py","old_string":o,"new_string":n}));
        assert!(e("x = 1", "x = 9").is_err()); // два вхождения
        assert!(e("zzz", "q").is_err());
        let (_, _, new) = e("y = 2", "y = 3").unwrap();
        assert_eq!(new, "x = 1\ny = 3\nx = 1\n");
    }

    #[test]
    fn edit_handles_crlf() {
        let d = tmp("crlf");
        std::fs::write(d.join("a.py"), "a\r\nb\r\nc\r\n").unwrap();
        let (_, _, new) = plan_edit(&d, &json!({"path":"a.py","old_string":"a\nb","new_string":"a\nB"})).unwrap();
        assert_eq!(new, "a\r\nB\r\nc\r\n");
    }

    #[test]
    fn diff_shows_changes() {
        let s = diff("a\nb\nc\n", "a\nB\nc\n");
        assert!(s.contains("- b") && s.contains("+ B"));
    }

    #[test]
    fn run_command_captures_output() {
        let d = tmp("run");
        let o = run_command(&d, "echo hello", 20);
        assert!(o.contains("hello"), "{o}");
    }
}

#[cfg(test)]
mod free_tests {
    use super::*;

    #[test]
    fn free_paths_flag_outside() {
        let root = std::env::temp_dir().join(format!("nexora-free-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let (p, out) = resolve_free(&root, "sub").unwrap();
        assert!(!out && p.ends_with("sub"));
        let (_, out) = resolve_free(&root, "..").unwrap();
        assert!(out, ".. выходит из проекта");
        let other = std::env::temp_dir().to_string_lossy().to_string();
        assert!(resolve_free(&root, &other).unwrap().1);
        assert!(list_path(&root).unwrap().contains("sub/"));
        std::fs::write(root.join(".env"), "A=1").unwrap();
        assert!(read_path(&root.join(".env")).is_err(), "секреты закрыты и вне проекта");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod size_probe {
    #[test]
    fn prompt_sizes() {
        let t = super::schemas().to_string();
        println!("СХЕМЫ ИНСТРУМЕНТОВ: {} символов, {} инструментов", t.chars().count(), super::schemas().as_array().map(|a| a.len()).unwrap_or(0));
        let sys = crate::agent::system_prompt(std::path::Path::new("proj"));
        println!("СИСТЕМНАЯ ПОДСКАЗКА: {} символов", sys.chars().count());
        let skills = crate::plugins::skill_index();
        println!("НАВЫКИ установлено: {} шт. (список в подсказку больше не кладётся)", skills.len());
        use serde_json::json;
        let size = |text: &str| -> usize {
            let conv = vec![json!({"role": "system", "content": sys}), json!({"role": "user", "content": text})];
            crate::router::select(&crate::router::profile_for(&conv, false), vec![]).to_string().chars().count()
        };
        println!("ИНСТРУМЕНТЫ: обычный вопрос {} симв., задача {} симв., задача с браузером {} симв.", size("привет, как дела?"), size("создай файл"), size("открой сайт"));
        for q in ["хочу написать игру", "сделай игру-платформер с боевой механикой", "как дела?", "исправь ошибку в тесте"] {
            let mut conv = vec![json!({"role": "system", "content": sys}), json!({"role": "user", "content": q})];
            let task = crate::router::profile_for(&conv, false).task;
            if task {
                crate::router::attach_skill(&mut conv);
            }
            let t = conv[1]["content"].as_str().unwrap_or("").to_string();
            let added = t.chars().count().saturating_sub(q.chars().count());
            let first = t.lines().find(|l| l.contains("Подключён навык") || l.contains("Похожие навыки")).unwrap_or("(навык не подобран)");
            println!("«{q}»: режим {}, добавлено {added} симв.: {}", if task { "задача" } else { "разговор" }, first.chars().take(110).collect::<String>());
        }
        let total_chat = sys.chars().count() + size("привет");
        println!("ИТОГО вводная часть простого вопроса: ~{} симв. (~{} токенов); раньше ~21000 симв.", total_chat, total_chat * 10 / 28);
    }
}
