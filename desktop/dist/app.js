(function () {
  "use strict";
  var T = window.__TAURI__;
  var invoke = T.core.invoke, listen = T.event.listen;
  var $ = function (id) { return document.getElementById(id); };
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function lsGet(k, d) { try { var v = localStorage.getItem(k); return v == null ? d : v; } catch (e) { return d; } }
  function lsSet(k, v) { try { localStorage.setItem(k, v); } catch (e) {} }

  var log = $("log"), ask = $("ask"), send = $("send"), stopBtn = $("stop"), startBtn = $("startBtn");
  var ready = false, busy = false, bot = null, curText = null, curThink = null, cards = {};
  var typing = null, tTimer = null, t0 = 0, statBase = "", ind = lsGet("lc-ind", "robot");
  if (!lsGet("lc-ind2", "")) { ind = "robot"; lsSet("lc-ind", "robot"); lsSet("lc-ind2", "1"); } /* однократный переход на индикатор «Робот» */
  var curModel = "", curSource = "builtin", curProfile = "local", attach = [], dl = {}, catData = null, catCat = "all";
  var WORDS = ["Думает…", "Размышляет…", "Разбирается…", "Сопоставляет…", "Прикидывает…"];

  /* ---------- навигация ---------- */
  var VIEWS = { chat: "v-chat", chats: "v-chats", models: "catalog", plugins: "v-plugins", auto: "v-auto", projects: "v-projects", files: "v-files", tasks: "v-soon", links: "v-links", server: "v-server", settings: "v-settings", look: "v-look" };
  var SOON = {
    auto: ["Автоматизация", "Запуск задач по расписанию: например, каждое утро собирать отчёт или проверять проект на ошибки."],
    tasks: ["Задачи", "Список дел агента: что поручено, что выполнено и что ждёт вашего решения."]
  };
  function show(name) {
    if (!VIEWS[name]) name = "chat";
    Object.keys(VIEWS).forEach(function (k) { var s = $(VIEWS[k]); if (s) s.hidden = true; });
    $(VIEWS[name]).hidden = false;
    if (SOON[name]) { $("soonTitle").textContent = SOON[name][0]; $("soonText").textContent = SOON[name][1]; }
    document.querySelectorAll(".side nav button").forEach(function (b) { if (b.dataset.view === name) b.setAttribute("aria-current", "page"); else b.removeAttribute("aria-current"); });
    if (name === "models") openCatalog();
    if (name === "files") showTree();
    if (name === "plugins") loadPlugins();
    if (name === "chat") ask.focus();
  }
  document.querySelectorAll("[data-view]").forEach(function (b) { b.addEventListener("click", function () { $("tuneBox").hidden = true; show(b.dataset.view); }); });

  /* ---------- окно ---------- */
  var W = T.window && T.window.getCurrentWindow ? T.window.getCurrentWindow() : null;
  $("wMin").addEventListener("click", function () { if (W) W.minimize(); });
  $("wMax").addEventListener("click", function () { if (W) W.toggleMaximize(); });
  $("wClose").addEventListener("click", function () { if (W) W.close(); });

  /* ---------- темы и своя тема ---------- */
  var CUS_DEF = { bg: "#0e1514", panel: "#131d1b", line: "#24423c", fg: "#e6f2ef", accent: "#2dd4bf", font: "system", fs: 14, alpha: 100, r: 10 };
  var CUS = Object.assign({}, CUS_DEF);
  try { var sc = JSON.parse(lsGet("lc-custom", "null")); if (sc) Object.assign(CUS, sc); } catch (e) {}
  var FONTS = { system: '"Segoe UI",system-ui,sans-serif', plex: '"IBM Plex Sans","Segoe UI",sans-serif', mono: '"JetBrains Mono",Consolas,monospace', serif: 'Georgia,"Times New Roman",serif', round: '"Trebuchet MS","Segoe UI",sans-serif' };
  var FNAMES = { system: "Системный (Segoe UI)", plex: "IBM Plex Sans", mono: "Моноширинный", serif: "Georgia (с засечками)", round: "Trebuchet (мягкий)" };
  var PRESETS = [
    ["Teal", { bg: "#0e1514", panel: "#131d1b", line: "#24423c", fg: "#e6f2ef", accent: "#2dd4bf" }],
    ["Rose", { bg: "#1a1015", panel: "#231720", line: "#4a2a3b", fg: "#f7e9ef", accent: "#fb7185" }],
    ["Amber", { bg: "#17130c", panel: "#201a10", line: "#43371d", fg: "#f6efe0", accent: "#f59e0b" }],
    ["Sky", { bg: "#0c1520", panel: "#112031", line: "#223c58", fg: "#e6f0fa", accent: "#38bdf8" }],
    ["Lime", { bg: "#10150c", panel: "#172010", line: "#2f4320", fg: "#edf5e3", accent: "#a3e635" }],
    ["Paper", { bg: "#f4efe6", panel: "#fffaf0", line: "#d9cfbd", fg: "#2b2418", accent: "#b45309" }],
    ["Mono", { bg: "#111111", panel: "#1a1a1a", line: "#333333", fg: "#eeeeee", accent: "#ffffff" }]];
  var CKEYS = ["--bg", "--panel", "--panel2", "--line", "--fg", "--muted", "--accent", "--accent2", "--accent-fg", "--r", "--fs", "--sans"];
  function lum(h) { var n = parseInt(h.slice(1), 16); return (0.299 * (n >> 16 & 255) + 0.587 * (n >> 8 & 255) + 0.114 * (n & 255)) / 255; }
  function hsl2hex(h, s, l) { s /= 100; l /= 100; var a = s * Math.min(l, 1 - l); function f(n) { var k = (n + h / 30) % 12, c = l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1)); return ("0" + Math.round(255 * c).toString(16)).slice(-2); } return "#" + f(0) + f(8) + f(4); }
  function applyCustom() {
    var s = document.documentElement.style, c = CUS, a = c.alpha + "%";
    s.setProperty("--bg", c.bg);
    s.setProperty("--panel", "color-mix(in srgb, " + c.panel + " " + a + ", transparent)");
    s.setProperty("--panel2", "color-mix(in srgb, color-mix(in srgb, " + c.panel + " 90%, " + c.fg + ") " + a + ", transparent)");
    s.setProperty("--line", c.line); s.setProperty("--fg", c.fg);
    s.setProperty("--muted", "color-mix(in srgb, " + c.fg + " 62%, " + c.bg + ")");
    s.setProperty("--accent", c.accent); s.setProperty("--accent2", c.accent); s.setProperty("--accent-fg", lum(c.accent) > 0.6 ? "#111111" : "#ffffff");
    s.setProperty("--r", c.r + "px"); s.setProperty("--fs", c.fs + "px"); s.setProperty("--sans", FONTS[c.font] || FONTS.system);
    s.colorScheme = lum(c.bg) < 0.5 ? "dark" : "light";
    lsSet("lc-custom", JSON.stringify(c));
  }
  function clearCustom() { var s = document.documentElement.style; CKEYS.forEach(function (k) { s.removeProperty(k); }); s.colorScheme = ""; }
  function setTheme(t) {
    var r = document.documentElement;
    if (t === "custom") applyCustom(); else clearCustom();
    if (t === "system") r.removeAttribute("data-theme"); else r.setAttribute("data-theme", t);
    document.querySelectorAll("#themes button").forEach(function (b) { b.setAttribute("aria-pressed", b.dataset.th === t); });
    lsSet("lc-theme", t);
  }
  function editCustom(patch) { Object.assign(CUS, patch); setTheme("custom"); renderCust(); }
  function renderCust() {
    var box = $("custBox"); box.innerHTML = "";
    var pal = el("div", "pal");
    PRESETS.forEach(function (p) {
      var b = el("button"); b.title = p[0]; b.setAttribute("aria-label", "Палитра " + p[0]);
      b.style.background = "linear-gradient(135deg," + p[1].bg + " 50%," + p[1].accent + " 50%)";
      b.addEventListener("click", function () { editCustom(p[1]); }); pal.appendChild(b);
    });
    var rnd = el("button", null, "🎲"); rnd.title = "Случайная палитра"; rnd.style.background = "var(--panel2)";
    rnd.addEventListener("click", function () {
      var h = Math.floor(Math.random() * 360), light = Math.random() < 0.25;
      editCustom(light ? { bg: hsl2hex(h, 25, 94), panel: hsl2hex(h, 30, 98), line: hsl2hex(h, 20, 80), fg: hsl2hex(h, 30, 12), accent: hsl2hex((h + 20) % 360, 70, 38) }
        : { bg: hsl2hex(h, 30, 7), panel: hsl2hex(h, 28, 11), line: hsl2hex(h, 25, 22), fg: hsl2hex(h, 25, 92), accent: hsl2hex((h + 30) % 360, 85, 62) });
    });
    pal.appendChild(rnd); box.appendChild(pal);
    var g = el("div", "cgrid"); box.appendChild(g);
    function row(label, input) { var r = el("div", "crow"); r.appendChild(el("span", null, label)); r.appendChild(input); g.appendChild(r); }
    [["Фон окна", "bg"], ["Панели", "panel"], ["Линии", "line"], ["Текст", "fg"], ["Кнопки и акцент", "accent"]].forEach(function (x) {
      var i = el("input"); i.type = "color"; i.value = CUS[x[1]]; i.setAttribute("aria-label", x[0]);
      i.addEventListener("input", function () { CUS[x[1]] = i.value; setTheme("custom"); }); row(x[0], i);
    });
    var sel = el("select"); sel.setAttribute("aria-label", "Шрифт");
    Object.keys(FNAMES).forEach(function (k) { var o = el("option", null, FNAMES[k]); o.value = k; if (k === CUS.font) o.selected = true; sel.appendChild(o); });
    sel.addEventListener("change", function () { editCustom({ font: sel.value }); }); row("Шрифт", sel);
    function range(label, key, min, max, unit) {
      var i = el("input"); i.type = "range"; i.min = min; i.max = max; i.value = CUS[key]; i.setAttribute("aria-label", label);
      var w = el("span"); w.style.cssText = "display:inline-flex;gap:8px;align-items:center"; var v = el("span", "muted", CUS[key] + unit); v.style.minWidth = "46px";
      i.addEventListener("input", function () { CUS[key] = +i.value; v.textContent = i.value + unit; setTheme("custom"); }); w.appendChild(i); w.appendChild(v); row(label, w);
    }
    range("Размер шрифта", "fs", 12, 18, " px"); range("Непрозрачность панелей", "alpha", 40, 100, " %"); range("Скругление кнопок", "r", 0, 20, " px");
    var rs = el("button", "btn", "Сбросить Custom"); rs.addEventListener("click", function () { CUS = Object.assign({}, CUS_DEF); setTheme("custom"); renderCust(); }); row("Вернуть начальные цвета", rs);
  }
  document.querySelectorAll("#themes button").forEach(function (b) { b.addEventListener("click", function () { setTheme(b.dataset.th); }); });
  setTheme(lsGet("lc-theme", "aura")); renderCust();

  /* ---------- персонаж и его состояния ---------- */
  var BOT = { show: true, size: 48, anim: "full" };
  try { var bs = JSON.parse(lsGet("lc-bot", "null")); if (bs) BOT = Object.assign(BOT, bs); } catch (e) {}
  var botState = "idle", engState = "idle", botFam = "nexora", botTimer = null, sleepTimer = null, botEl = null, wEl = null, prevState = "idle";
  function saveBot() { lsSet("lc-bot", JSON.stringify(BOT)); }
  function applyAnim() {
    var r = document.documentElement.classList;
    r.toggle("nx-anim-lite", BOT.anim === "lite"); r.toggle("nx-anim-off", BOT.anim === "off");
    document.querySelectorAll("#botAnimSeg button").forEach(function (b) { b.setAttribute("aria-pressed", b.dataset.ba === BOT.anim); });
    document.querySelectorAll("#botSizeSeg button").forEach(function (b) { b.setAttribute("aria-pressed", +b.dataset.bs === BOT.size); });
    $("botShow").checked = BOT.show;
  }
  function renderBot() {
    var box = $("bot"); box.hidden = !BOT.show; box.innerHTML = "";
    if (BOT.show) { box.innerHTML = NxChar.html(botFam, botState, BOT.size); botEl = box.firstChild; } else { botEl = null; }
    $("wBot").innerHTML = NxChar.html(botFam, botState, 150); wEl = $("wBot").firstChild;
    $("botPrev").innerHTML = NxChar.html(botFam, prevState, Math.max(BOT.size, 64));
    var rb = document.querySelector("#indPreview .rb"); if (rb) rb.innerHTML = NxChar.html(botFam, "thinking", 32);
  }
  function paint(s) {
    botState = s; if (botEl) NxChar.set(botEl, s); if (wEl) NxChar.set(wEl, s);
    if (typing && typing.typRobot) NxChar.set(typing.typRobot, s);
  }
  function restState() { return busy ? "thinking" : engState; }
  function armSleep() { clearTimeout(sleepTimer); sleepTimer = setTimeout(function () { if (!busy && engState === "idle") paint("sleep"); }, 10 * 60 * 1000); }
  function robotSet(s, ms) { clearTimeout(botTimer); paint(s); armSleep(); if (ms) botTimer = setTimeout(function () { paint(restState()); }, ms); }
  function robotEngine(state) { engState = state === "ready" ? "idle" : state === "loading" ? "loading" : "error"; if (!busy) robotSet(engState === "error" ? "error" : engState); }
  function robotTool(name, status) {
    if (status !== "running") { robotSet("thinking"); return; }
    if (name === "read_file" || name === "list_dir" || name === "list_apps") robotSet("thinking");
    else if (name === "write_file" || name === "edit_file") robotSet("coding");
    else robotSet("executing");
  }
  function robotDone(err) { if (err) robotSet("error", 3500); else robotSet("success", 2400); }
  function robotFamily() { botFam = NxChar.family(curModel); renderBot(); }
  document.querySelectorAll("#botSizeSeg button").forEach(function (b) { b.addEventListener("click", function () { BOT.size = +b.dataset.bs; saveBot(); applyAnim(); renderBot(); }); });
  document.querySelectorAll("#botAnimSeg button").forEach(function (b) { b.addEventListener("click", function () { BOT.anim = b.dataset.ba; saveBot(); applyAnim(); }); });
  $("botShow").addEventListener("change", function () { BOT.show = $("botShow").checked; saveBot(); renderBot(); });
  NxChar.STATES.forEach(function (s) {
    var b = el("button", null, NxChar.TITLE[s]); b.type = "button";
    b.addEventListener("click", function () { prevState = s; NxChar.set($("botPrev"), s); document.querySelectorAll("#botStates button").forEach(function (x) { x.setAttribute("aria-pressed", x === b); }); });
    $("botStates").appendChild(b);
  });
  document.addEventListener("visibilitychange", function () { document.documentElement.classList.toggle("nx-hidden", document.hidden); });

  /* ---------- состояние движка, строка состояния ---------- */
  var NAMES = { local: "Nexora", server: "Nexora Server", agent: "Nexora Agent" };
  var MODE_SB = { local: "Агент (локально)", agent: "Сервер (подключение)", server: "Этот ПК: сервер" };
  var MODE_SIDE = { local: "Локальный режим", agent: "Подключение к серверу", server: "Режим сервера" };
  function refresh() { send.disabled = !ready || busy; }
  function setState(state, msg) {
    $("dot").className = "dot " + state; $("sbDot").className = "dot " + state; $("mstate").textContent = msg;
    $("sbReady").textContent = state === "ready" ? "Готов к работе" : state === "loading" ? "Загрузка модели…" : state === "error" ? "Ошибка" : "Не запущено";
    $("mpick").disabled = state === "loading";
    ready = state === "ready"; startBtn.hidden = state === "loading" || state === "ready";
    robotEngine(state); refresh();
  }
  startBtn.addEventListener("click", function () { setState("loading", "Запуск…"); invoke("engine_start").catch(function (e) { setState("error", String(e)); }); });
  listen("engine", function (ev) { setState(ev.payload.state, ev.payload.msg); });
  function isMoe(id) {
    if (id && id.indexOf("nx:") === 0 && catData) { var it = catData.items.filter(function (x) { return "nx:" + x.model.id === id; })[0]; if (it) return !!it.model.moe; }
    return /(gpt-oss|moe|a3b|coder-v2|qwen3-coder|mixtral)/i.test(id || "");
  }
  function updateInfo() {
    var sel = $("mpick"), label = sel.options.length && sel.selectedIndex >= 0 ? sel.options[sel.selectedIndex].text : curModel || "—";
    $("sbModel").textContent = "Модель: " + label;
    var moe = isMoe(curModel); $("chipMoe").hidden = !moe; $("sbMoe").textContent = "MoE: " + (moe ? "включён" : "нет");
    $("chipSrc").textContent = curProfile === "agent" ? "Сервер" : curSource === "external" ? "Внешний" : "Локально";
    $("sbMode").textContent = "Режим: " + MODE_SB[curProfile];
  }

  /* ---------- модели: выбор ---------- */
  function loadModels(cur) {
    curModel = cur; robotFamily();
    invoke("list_models").then(function (list) {
      var s = $("mpick"); s.innerHTML = "";
      if (cur && !list.some(function (x) { return x.id === cur; })) list.unshift({ id: cur, label: cur });
      list.forEach(function (x) { var o = el("option", null, x.label); o.value = x.id; if (x.id === cur) o.selected = true; s.appendChild(o); });
      updateInfo();
    });
  }
  $("mpick").addEventListener("change", function () {
    var name = $("mpick").value, prev = curModel;
    setState("loading", "Смена модели…");
    invoke("set_model", { name: name }).then(function () { curModel = name; robotFamily(); clearChat(); updateInfo(); })
      .catch(function (e) { alert(String(e)); loadModels(prev); setState("ready", prev + " готова"); });
  });

  /* ---------- каталог моделей с категориями ---------- */
  var MODE = { cpu: "процессор", "cpu+disk": "процессор + подкачка с SSD", gpu: "видеокарта", "gpu+cpu-experts": "видеокарта + процессор", "gpu+cpu-layers": "видеокарта + процессор", metal: "Apple Metal", none: "—" };
  var CATS = [["all", "Все"], ["chat", "Чат"], ["code", "Код"], ["vision", "Изображения"], ["audio", "Аудио"]];
  var CAT_NOTE = {
    vision: "Здесь модели, которые читают картинки и скриншоты. Генерация изображений появится позже.",
    audio: "Распознавание речи (Whisper и подобные) и озвучка появятся позже: для них нужен отдельный движок."
  };
  var TAGNAME = { chat: "чат", code: "код", vision: "картинки", audio: "аудио" };
  function fmtMB(n) { return (n / 1048576).toFixed(0); }
  function renderCatalog(c) {
    var h = c.hw, gpu = h.gpus.length ? h.gpus.map(function (g) { return g.name + " " + g.vram_gb.toFixed(0) + " ГБ"; }).join(", ") : "нет";
    $("catHw").textContent = "ОЗУ " + h.ram_gb.toFixed(0) + " ГБ · ядер " + h.cores + " (потоков " + h.threads + ") · память ≈ " + h.mem_bw_gbs.toFixed(0) + " ГБ/с · видеокарта: " + gpu + " · контекст " + c.ctx.toLocaleString("ru-RU");
    var chips = $("catFilter"); chips.innerHTML = "";
    CATS.forEach(function (x) {
      var b = el("button", null, x[1]); b.type = "button"; b.setAttribute("aria-pressed", catCat === x[0]);
      b.addEventListener("click", function () { catCat = x[0]; renderCatalog(c); }); chips.appendChild(b);
    });
    var items = c.items.filter(function (it) { return catCat === "all" || (it.model.tags || []).indexOf(catCat) >= 0; });
    var key = $("catSort").value;
    items.sort(function (a, b) {
      if (key === "speed") return (b.est.fits ? b.est.tok_s : -1) - (a.est.fits ? a.est.tok_s : -1);
      if (key === "size") return a.model.size_gb - b.model.size_gb;
      return b.model.quality - a.model.quality;
    });
    var body = $("catBody"); body.innerHTML = "";
    if (CAT_NOTE[catCat]) body.appendChild(el("p", "muted small", CAT_NOTE[catCat]));
    if (!items.length) { body.appendChild(el("div", "emptycat", "В этой категории пока нет моделей.")); return; }
    items.forEach(function (it) {
      var m = it.model, e = it.est, row = el("div", "mrow"), left = el("div"), nm = el("div", "nm", m.name);
      if (m.id === c.recommended) nm.appendChild(el("span", "badge rec", "рекомендуется"));
      var st = !e.fits ? ["bad", "не помещается"] : e.tok_s < 6 ? ["warn", "медленно"] : ["ok", "подходит"];
      nm.appendChild(el("span", "badge " + st[0], st[1]));
      (m.tags || []).forEach(function (t) { nm.appendChild(el("span", "badge tag", TAGNAME[t] || t)); });
      if (m.custom) nm.appendChild(el("span", "badge tag", "с Hugging Face"));
      left.appendChild(nm);
      left.appendChild(el("div", "sub", m.size_gb.toFixed(1) + " ГБ · " + (m.moe ? "MoE, активно " + m.active_b + "B из " + m.params_b + "B" : m.params_b + "B") +
        " · нужно ≈ " + e.mem_gb.toFixed(1) + " ГБ · " + (e.fits ? "режим: " + MODE[e.mode] : e.note)));
      if (e.fits) {
        left.appendChild(el("div", "sub", "≈ " + e.tok_s.toFixed(1) + " ток/с"));
        var bar = el("div", "bar2"), i = el("i"); i.style.width = Math.min(100, e.tok_s / 20 * 100) + "%"; bar.appendChild(i); left.appendChild(bar);
      }
      left.appendChild(el("div", "sub", m.notes));
      var act = el("div", "act"), prog = el("div", "prog"); prog.id = "pg-" + m.id;
      if (it.downloaded) {
        var use = el("button", "btn p", "Использовать");
        use.addEventListener("click", function () {
          show("chat"); setState("loading", "Смена модели…");
          invoke("set_model", { name: "nx:" + m.id }).then(function () { clearChat(); loadModels("nx:" + m.id); }).catch(function (er) { alert(String(er)); });
        });
        act.appendChild(use);
        var del = el("button", "btn d", "Удалить");
        del.addEventListener("click", function () {
          if (!confirm("Удалить файлы модели «" + m.name + "» с диска?")) return;
          invoke("model_delete", { id: m.id }).then(function () { openCatalog(); loadModels(curModel); }).catch(function (er) { alert(String(er)); });
        });
        act.appendChild(del);
      } else if (dl[m.id]) {
        var cn = el("button", "btn", "Отмена"); cn.addEventListener("click", function () { invoke("model_cancel", { id: m.id }); }); act.appendChild(cn);
        prog.textContent = dl[m.id]; act.appendChild(prog);
      } else {
        var go = el("button", "btn p", "Скачать " + m.size_gb.toFixed(1) + " ГБ"); go.disabled = !e.fits;
        go.addEventListener("click", function () {
          dl[m.id] = "запуск…"; renderCatalog(c);
          invoke("model_download", { id: m.id }).catch(function (er) { delete dl[m.id]; alert(String(er)); renderCatalog(c); });
        });
        act.appendChild(go);
      }
      row.appendChild(left); row.appendChild(act); body.appendChild(row);
    });
  }
  function openCatalog() { $("catHw").textContent = "Определяю железо…"; invoke("get_catalog").then(function (c) { catData = c; renderCatalog(c); updateInfo(); }); }
  $("catSort").addEventListener("change", function () { if (catData) renderCatalog(catData); });
  $("catBtn").addEventListener("click", function () { show("models"); });
  listen("download", function (ev) {
    var p = ev.payload;
    if (p.state === "running") {
      dl[p.id] = (p.msg ? p.msg + ": " : "") + (p.total ? fmtMB(p.done) + " / " + fmtMB(p.total) + " МБ · " + (p.speed / 1048576).toFixed(1) + " МБ/с" : "загрузка…");
      var e = document.getElementById("pg-" + p.id); if (e) e.textContent = dl[p.id];
    } else {
      delete dl[p.id];
      if (p.state === "error") alert(p.msg);
      if (!$("catalog").hidden) invoke("get_catalog").then(function (c) { catData = c; renderCatalog(c); });
      loadModels(curModel);
    }
  });

  /* ---------- проекты и файлы ---------- */
  function treeNode(parentUl, rel) {
    invoke("fs_list", { rel: rel }).then(function (items) {
      items.forEach(function (it) {
        var li = el("li"), sp = el("span", it.dir ? "d" : "", (it.dir ? "▸ " : "") + it.name); li.appendChild(sp);
        if (it.dir) {
          var sub = null, open = false, path = (rel === "." ? "" : rel + "/") + it.name;
          sp.addEventListener("click", function () {
            open = !open; sp.textContent = (open ? "▾ " : "▸ ") + it.name;
            if (open) { sub = el("ul"); li.appendChild(sub); treeNode(sub, path); } else if (sub) { li.removeChild(sub); sub = null; }
          });
        }
        parentUl.appendChild(li);
      });
    }).catch(function () {});
  }
  function showTree() { var t = $("tree"); t.innerHTML = ""; var ul = el("ul"); t.appendChild(ul); treeNode(ul, "."); }
  function setWs(p) { $("wsPath").value = p || ""; $("wsMini").textContent = p ? "Проект: " + p : "Папка проекта не выбрана"; }
  function openWs(path) { invoke("set_workspace", { path: path }).then(function (p) { setWs(p); clearChat(); }).catch(function (e) { alert(String(e)); }); }
  function pickDir(title) { return T.dialog.open({ directory: true, multiple: false, title: title }); }
  $("wsPick").addEventListener("click", function () { pickDir("Выберите папку проекта").then(function (p) { if (p) openWs(p); }); });
  $("wsNew").addEventListener("click", function () { invoke("new_project").then(function (p) { setWs(p); clearChat(); }).catch(function (e) { alert(String(e)); }); });
  $("autoFolder").addEventListener("change", function () { invoke("set_auto_folder", { on: $("autoFolder").checked }); });
  $("rootPick").addEventListener("click", function () {
    pickDir("Где создавать новые проекты").then(function (p) { if (p) invoke("set_projects_root", { path: p }).then(function (r) { $("rootLbl").textContent = "Новые проекты: " + r; }); });
  });
  listen("workspace", function (ev) { setWs(ev.payload.path); });
  $("mode").addEventListener("change", function () { invoke("set_mode", { mode: $("mode").value }); });
  $("tuneBtn").addEventListener("click", function () { $("tuneBox").hidden = !$("tuneBox").hidden; });
  $("newChat").addEventListener("click", function () { invoke("chat_reset"); clearChat(); $("tuneBox").hidden = true; });

  /* ---------- сообщения ---------- */
  function updateWelcome() { $("welcome").hidden = log.children.length > 0; }
  function clearChat() { log.innerHTML = ""; resetBot(); attach = []; renderAttach(); updateWelcome(); }
  function resetBot() { bot = null; curText = null; curThink = null; cards = {}; }
  function ensureBot() {
    if (!bot) { bot = el("div", "msg ai"); bot.appendChild(el("b", null, "Модель")); log.appendChild(bot); updateWelcome(); }
    return bot;
  }
  function scroll() { if (typing && bot) bot.appendChild(typing); log.scrollTop = log.scrollHeight; }
  listen("chat-token", function (ev) {
    var p = ev.payload, b = ensureBot();
    robotSet(p.kind === "think" ? "thinking" : "generating");
    if (p.kind === "think") {
      if (!curThink) { var dt = el("details", "think"); dt.appendChild(el("summary", null, "Размышления")); curThink = el("div", "tb"); dt.appendChild(curThink); b.appendChild(dt); }
      curThink.textContent += p.text; setStat("Думает…");
    } else {
      setStat("Пишет ответ…");
      if (!curText) { curText = el("div"); curText.buf = ""; b.appendChild(curText); }
      curText.buf += p.text; curText.textContent = curText.buf;
    }
    scroll();
  });
  listen("chat-status", function (ev) { setStat(ev.payload.text); });
  function card(id) {
    if (cards[id]) return cards[id];
    var c = el("div", "card"), t = el("div", "t"), tt = el("span", "tt"), st = el("span", "st");
    t.appendChild(tt); t.appendChild(st); c.appendChild(t); ensureBot().appendChild(c);
    curText = null; curThink = null; cards[id] = { root: c, title: tt, status: st }; return cards[id];
  }
  var LABEL = { running: "выполняется", done: "готово", error: "ошибка", denied: "отклонено", wait: "ждёт решения" };
  function setStatus(c, s) { c.status.textContent = LABEL[s] || s; c.status.className = "st " + s; }
  function fillDiff(pre, text) {
    pre.textContent = "";
    text.split("\n").forEach(function (l) { pre.appendChild(el("span", "dl" + (l.indexOf("+ ") === 0 ? " add" : l.indexOf("- ") === 0 ? " del" : ""), l || " ")); });
  }
  listen("tool", function (ev) {
    var p = ev.payload, c = card(p.id);
    c.title.textContent = p.title || p.name; setStatus(c, p.status);
    setStat(p.status === "running" ? (p.title || p.name) : "Думает…"); robotTool(p.name, p.status);
    if (p.output && p.status !== "running") c.root.appendChild(el("pre", null, p.output));
    scroll();
  });
  listen("approval", function (ev) {
    var p = ev.payload, c = card(p.call_id);
    c.title.textContent = p.title; setStatus(c, "wait"); setStat("Ждёт вашего решения", true); robotSet("thinking");
    var pre = el("pre"); if (p.kind === "edit" || p.kind === "write") fillDiff(pre, p.detail); else pre.textContent = p.detail;
    var a = el("div", "a"), ok = el("button", "btn p", p.kind === "delete" ? "Удалить" : p.kind === "script" ? "Выполнить скрипт" : p.kind === "mcp" ? "Разрешить вызов" : p.kind === "registry" ? "Изменить реестр" : p.kind === "run" ? "Выполнить" : "Применить"), no = el("button", "btn d", "Отклонить");
    function done(v) { invoke("approve", { id: p.id, ok: v }); c.root.removeChild(pre); c.root.removeChild(a); setStatus(c, "running"); setStat("Выполняет…"); }
    ok.addEventListener("click", function () { done(true); }); no.addEventListener("click", function () { done(false); });
    a.appendChild(ok); a.appendChild(no);
    if (p.kind !== "delete" && p.kind !== "script" && p.kind !== "mcp" && p.kind !== "registry" && p.title.indexOf("БОЕВОЙ") < 0 && p.title.indexOf("АДМИНИСТРАТОРА") < 0) {
      var all = el("button", "btn", "Разрешать всё до конца чата");
      all.title = "Дальше правки и команды в этом чате выполняются без вопросов. Удаление, права администратора и боевой сервер всё равно спросят. Новый чат возвращает вопросы.";
      all.addEventListener("click", function () { invoke("set_chat_trust", { on: true }).then(function () { done(true); setStat("Выполняет… (без вопросов до конца чата)"); }); });
      a.appendChild(all);
    } c.root.appendChild(pre); c.root.appendChild(a); scroll();
  });
  listen("chat-done", function (ev) {
    if (ev.payload.error) ensureBot().appendChild(el("div", "err", ev.payload.error));
    typingOff(); busy = false; stopBtn.hidden = true; resetBot(); refresh(); ask.focus(); robotDone(ev.payload.error);
  });

  /* ---------- индикатор «думает»: робот, точки, бегущий текст ---------- */
  function tick() {
    if (!typing) return;
    var s = Math.floor((Date.now() - t0) / 1000);
    typing.querySelector(".el").textContent = s + " с";
    if (ind === "shimmer" && statBase === "Думает…") typing.querySelector(".stat").textContent = WORDS[Math.floor(s / 3) % WORDS.length];
  }
  function typingOn(text) {
    var b = ensureBot();
    if (!typing) {
      typing = el("div", "typing " + ind);
      var rb = el("span", "rb"); rb.innerHTML = NxChar.html(botFam, botState === "idle" ? "thinking" : botState, 36); typing.typRobot = rb.firstChild; typing.appendChild(rb);
      var d = el("span", "dots"); d.appendChild(el("i")); d.appendChild(el("i")); d.appendChild(el("i"));
      typing.appendChild(d); typing.appendChild(el("span", "stat")); typing.appendChild(el("span", "el"));
      t0 = Date.now(); tTimer = setInterval(tick, 1000);
    }
    setStat(text); b.appendChild(typing);
  }
  function setStat(t, paused) { if (!typing) return; statBase = t; typing.querySelector(".stat").textContent = t; typing.classList.toggle("paused", !!paused); }
  function typingOff() { clearInterval(tTimer); if (typing && typing.parentNode) typing.parentNode.removeChild(typing); typing = null; }
  function setInd(v) {
    ind = v;
    document.querySelectorAll("#indSeg button").forEach(function (b) { b.setAttribute("aria-pressed", b.dataset.ind === v); });
    $("indPreview").className = "typing " + v;
    if (typing) typing.className = "typing " + v + (typing.classList.contains("paused") ? " paused" : "");
    lsSet("lc-ind", v);
  }
  document.querySelectorAll("#indSeg button").forEach(function (b) { b.addEventListener("click", function () { setInd(b.dataset.ind); }); });

  /* ---------- картинки ---------- */
  function renderAttach() {
    var row = $("attachRow"); row.innerHTML = ""; row.hidden = !attach.length;
    attach.forEach(function (u, i) {
      var w = el("div", "th"), im = el("img"); im.src = u; im.alt = "Картинка " + (i + 1);
      var x = el("button", null, "×"); x.type = "button"; x.setAttribute("aria-label", "Убрать картинку");
      x.addEventListener("click", function () { attach.splice(i, 1); renderAttach(); }); w.appendChild(im); w.appendChild(x); row.appendChild(w);
    });
  }
  function addImage(file) {
    if (!file || file.type.indexOf("image/") !== 0 || attach.length >= 6) return;
    var rd = new FileReader();
    rd.onload = function () {
      var im = new Image();
      im.onload = function () {
        var max = 1280, k = Math.min(1, max / Math.max(im.width, im.height)), c = document.createElement("canvas");
        c.width = Math.round(im.width * k); c.height = Math.round(im.height * k);
        var g = c.getContext("2d"); g.fillStyle = "#fff"; g.fillRect(0, 0, c.width, c.height); g.drawImage(im, 0, 0, c.width, c.height);
        attach.push(c.toDataURL("image/jpeg", 0.88)); renderAttach();
      };
      im.src = rd.result;
    };
    rd.readAsDataURL(file);
  }
  $("attachBtn").addEventListener("click", function () { $("fileIn").click(); });
  $("fileIn").addEventListener("change", function () { Array.prototype.forEach.call($("fileIn").files, addImage); $("fileIn").value = ""; });
  ask.addEventListener("paste", function (e) {
    var items = (e.clipboardData && e.clipboardData.items) || [];
    Array.prototype.forEach.call(items, function (it) { if (it.type.indexOf("image/") === 0) { addImage(it.getAsFile()); e.preventDefault(); } });
  });
  var cp = document.querySelector(".cpanel");
  cp.addEventListener("dragover", function (e) { e.preventDefault(); });
  cp.addEventListener("drop", function (e) { e.preventDefault(); Array.prototype.forEach.call((e.dataTransfer && e.dataTransfer.files) || [], addImage); });

  /* ---------- отправка ---------- */
  function submit() {
    var text = ask.value.trim();
    if ((!text && !attach.length) || !ready || busy) return;
    var imgs = attach.slice(); ask.value = ""; autosize(); attach = []; renderAttach();
    var u = el("div", "msg user"); u.appendChild(el("b", null, "Вы"));
    if (text) u.appendChild(el("div", null, text));
    imgs.forEach(function (s) { var im = el("img", "pic"); im.src = s; u.appendChild(im); });
    log.appendChild(u); updateWelcome();
    resetBot(); busy = true; stopBtn.hidden = false; refresh(); typingOn("Читает запрос…"); robotSet("thinking"); scroll();
    invoke("chat_send", { text: text, images: imgs.length ? imgs : null }).catch(function (e) {
      typingOff(); ensureBot().appendChild(el("div", "err", String(e))); busy = false; stopBtn.hidden = true; refresh(); robotSet("error", 3000);
    });
  }
  function autosize() { ask.style.height = "auto"; ask.style.height = Math.min(140, ask.scrollHeight) + "px"; }
  ask.addEventListener("input", autosize);
  $("form").addEventListener("submit", function (e) { e.preventDefault(); submit(); });
  ask.addEventListener("keydown", function (e) { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } });
  stopBtn.addEventListener("click", function () { invoke("chat_stop"); });
  document.querySelectorAll("#quick button").forEach(function (b) {
    b.addEventListener("click", function () { show("chat"); ask.value = b.dataset.q; autosize(); ask.focus(); ask.setSelectionRange(ask.value.length, ask.value.length); });
  });

  /* ---------- режим работы и соединения ---------- */
  var I = {
    pc: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
    laptop: '<svg viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="10" rx="2"/><path d="M3 19h18"/></svg>',
    srv: '<svg viewBox="0 0 24 24"><rect x="4" y="3" width="16" height="7" rx="2"/><rect x="4" y="14" width="16" height="7" rx="2"/><path d="M8 6.5h.01M8 17.5h.01"/></svg>',
    ai: '<svg viewBox="0 0 24 24"><path d="M12 3l2.2 6.8L21 12l-6.8 2.2L12 21l-2.2-6.8L3 12l6.8-2.2z"/></svg>'
  };
  var CARDS = {
    local: { ico: I.pc, title: "Локальный агент", desc: "Вычисления и выполнение команд на твоём компьютере.", dia: [["ИИ-агент", I.ai], "→", ["Твой ПК", I.laptop]],
      cap: "Пишет и запускает код, работает с файлами в папке проекта. Каждое изменение и запуск с твоего подтверждения. Снаружи подключиться нельзя." },
    agent: { ico: I.srv, title: "Серверный режим", desc: "Модель работает на удалённом сервере и использует его вычислительные ресурсы.", dia: [["Твой ПК", I.laptop], "⇄", ["Сервер + ИИ", I.srv]],
      cap: "Ты подключаешься к своему серверу Nexora Server. Файлы и команды по умолчанию остаются у тебя. Сетевая часть (ключи, код Authenticator) появится на этапе 4: сейчас меняется только вид и название." },
    server: { ico: I.srv, title: "Этот компьютер: сервер", desc: "Принимает подключения твоих устройств и считает ответы модели.", dia: [["Твои устройства", I.laptop], "⇄", ["Этот ПК + ИИ", I.srv]],
      cap: "Вход только с сертификатом устройства и кодом из Authenticator, порт закрыт для остальных. Сетевая часть появится на этапе 4: сейчас меняется только вид и название." }
  };
  var tab = lsGet("lc-tab", "local");
  function fillCard(v) {
    var c = CARDS[v]; $("mIco").innerHTML = c.ico; $("mTitle").textContent = c.title; $("mDesc").textContent = c.desc; $("mCap").textContent = c.cap;
    var d = $("mDia"); d.innerHTML = "";
    c.dia.forEach(function (x) {
      if (typeof x === "string") { d.appendChild(el("span", "marrow", x)); return; }
      var n = el("div", "mnode"), ic = el("span", "ico"); ic.innerHTML = x[1]; n.appendChild(ic); n.appendChild(el("span", null, x[0])); d.appendChild(n);
    });
  }
  function showProfile(v) {
    var host = v === "server"; if (!host) tab = v; curProfile = v;
    $("hostChk").checked = host; $("profSeg").classList.toggle("off", host);
    document.querySelectorAll("#profSeg button").forEach(function (b) { b.setAttribute("aria-pressed", !host && b.dataset.prof === v); });
    fillCard(v); $("brand").textContent = NAMES[v].toUpperCase(); document.title = NAMES[v];
    $("profNote").textContent = "Название программы: " + NAMES[v];
    var sm = $("sideMode"); sm.dataset.mode = v; $("sideModeTxt").textContent = MODE_SIDE[v]; updateInfo(); lsSet("lc-tab", tab);
  }
  function pickProfile(v) { showProfile(v); invoke("set_profile", { profile: v }); }
  document.querySelectorAll("#profSeg button").forEach(function (b) { b.addEventListener("click", function () { pickProfile(b.dataset.prof); }); });
  $("hostChk").addEventListener("change", function () { pickProfile($("hostChk").checked ? "server" : tab); });

  function setSrc(v) {
    curSource = v; updateInfo();
    document.querySelectorAll("#srcSeg button").forEach(function (b) { b.setAttribute("aria-pressed", b.dataset.src === v); });
    $("extBox").hidden = v !== "external";
  }
  document.querySelectorAll("#srcSeg button").forEach(function (b) {
    b.addEventListener("click", function () {
      var v = b.dataset.src; setSrc(v);
      if (v === "builtin") { setState("loading", "Запуск встроенного движка…"); invoke("use_builtin").then(function () { clearChat(); loadModels(""); }).catch(function (e) { alert(String(e)); }); }
    });
  });
  $("extFind").addEventListener("click", function () {
    var box = $("extList"); box.innerHTML = "";
    invoke("detect_engines").then(function (list) {
      if (!list.length) { box.appendChild(el("p", "muted small", "На стандартных портах ничего не найдено. Введите адрес вручную.")); return; }
      list.forEach(function (d) { var b = el("button", "btn", d.name + " · " + d.url + " · моделей: " + d.models); b.addEventListener("click", function () { $("extUrl").value = d.url; }); box.appendChild(b); });
    });
  });
  $("extGo").addEventListener("click", function () {
    var note = $("extNote"); note.className = "muted small"; note.textContent = "Подключение…"; setState("loading", "Подключение…");
    invoke("use_external", { url: $("extUrl").value, key: $("extKey").value }).then(function (r) {
      clearChat(); loadModels(r.model);
      note.textContent = (r.warn || []).join("\n") || "Подключено."; note.className = (r.warn || []).length ? "warn small" : "muted small";
    }).catch(function (e) { note.className = "warn small"; note.textContent = String(e); setState("error", "Нет подключения"); robotSet("offline"); });
  });

  /* ---------- настройки: контекст, авто-режим ---------- */
  var ctxVal = 0;
  function showCtx() {
    document.querySelectorAll("#ctxSeg button").forEach(function (b) { b.setAttribute("aria-pressed", (b.dataset.ctxm === "manual") === (ctxVal > 0)); });
    $("ctxManual").hidden = ctxVal === 0; if (ctxVal > 0) $("ctxSel").value = String(ctxVal);
    invoke("ctx_info", { ctx: ctxVal }).then(function (i) {
      var t = (i.auto ? "Авто: " : "") + i.ctx.toLocaleString("ru-RU") + " токенов. Контекст ≈ " + i.kv_gb.toFixed(1) + " ГБ, модель вместе с ним ≈ " + i.need_gb.toFixed(1) + " ГБ из " + i.ram_gb.toFixed(0) + " ГБ ОЗУ.";
      if (!i.fits) t += " Не помещается: уменьшите контекст или выберите модель меньше.";
      $("ctxNote").textContent = t + " Действует на встроенный движок; у LM Studio и Ollama задаётся в них самих."; $("ctxNote").className = i.fits ? "muted small" : "warn small";
    });
  }
  function applyCtx(v) { ctxVal = v; showCtx(); setState("loading", "Перезапуск движка…"); invoke("set_ctx", { ctx: v }).catch(function (e) { alert(String(e)); }); }
  document.querySelectorAll("#ctxSeg button").forEach(function (b) { b.addEventListener("click", function () { applyCtx(b.dataset.ctxm === "auto" ? 0 : (parseInt($("ctxSel").value, 10) || 32768)); }); });
  $("ctxSel").addEventListener("change", function () { applyCtx(parseInt($("ctxSel").value, 10)); });
  function syncAuto() {
    var on = $("autoChk").checked, b = $("autoBtn");
    b.textContent = "Авто: " + (on ? "вкл" : "выкл"); b.setAttribute("aria-pressed", on ? "true" : "false");
    b.classList.toggle("p", on); $("autoNote").hidden = !on;
  }
  $("autoChk").addEventListener("change", function () { invoke("set_auto_mode", { on: $("autoChk").checked }); syncAuto(); });
  $("autoBtn").addEventListener("click", function () { $("autoChk").checked = !$("autoChk").checked; $("autoChk").dispatchEvent(new Event("change")); });

  /* ---------- Hugging Face: поиск и загрузка любых GGUF ---------- */
  function fmtN(n) { return n >= 1e6 ? (n / 1e6).toFixed(1) + " млн" : n >= 1e3 ? (n / 1e3).toFixed(0) + " тыс" : String(n); }
  $("hfForm").addEventListener("submit", function (e) {
    e.preventDefault();
    var box = $("hfRes"); box.innerHTML = ""; box.appendChild(el("p", "empty", "Ищу…"));
    invoke("hf_search", { q: $("hfQ").value }).then(function (list) {
      box.innerHTML = "";
      if (!list.length) { box.appendChild(el("p", "empty", "Ничего не найдено.")); return; }
      list.forEach(function (r) {
        var row = el("div", "hfrow"), left = el("div"), files = el("div", "hffiles");
        left.appendChild(el("div", "nm", r.id)); left.appendChild(el("div", "sub muted small", "скачиваний: " + fmtN(r.downloads) + " · ♥ " + r.likes));
        var b = el("button", "btn", "Файлы");
        b.addEventListener("click", function () { if (files.children.length) { files.innerHTML = ""; return; } showHfFiles(r.id, files); });
        row.appendChild(left); row.appendChild(b); row.appendChild(files); box.appendChild(row);
      });
    }).catch(function (er) { box.innerHTML = ""; box.appendChild(el("p", "warnline", String(er))); });
  });
  function showHfFiles(repo, box) {
    box.appendChild(el("p", "empty", "Загружаю список файлов…"));
    var ramP = catData ? Promise.resolve(catData) : invoke("get_catalog");
    Promise.all([invoke("hf_files", { repo: repo }), ramP]).then(function (res) {
      var d = res[0], c = res[1]; catData = c; box.innerHTML = "";
      var budget = Math.min(c.hw.ram_gb - 4, c.hw.ram_gb * 0.85);
      if (!d.files.length) { box.appendChild(el("p", "empty", "В репозитории нет подходящих файлов GGUF.")); return; }
      var mm = d.mmproj.filter(function (x) { return /f16/i.test(x.name); })[0] || d.mmproj[0];
      var wantImg = null;
      if (mm) {
        var lab = el("label", "hostrow"); var cb = el("input"); cb.type = "checkbox"; cb.checked = true; wantImg = cb;
        lab.appendChild(cb); lab.appendChild(document.createTextNode(" Скачать и файл для картинок (" + mm.name + ", " + mm.size_gb.toFixed(2) + " ГБ)")); box.appendChild(lab);
      }
      d.files.sort(function (a, b) { return a.size_gb - b.size_gb; }).forEach(function (f) {
        var fit = f.size_gb + 1.5 + 0.5 <= budget, row = el("div", "hffile");
        row.appendChild(el("span", null, f.name)); row.appendChild(el("span", "muted", f.size_gb.toFixed(1) + " ГБ"));
        row.appendChild(el("span", "badge " + (fit ? "ok" : "bad"), fit ? "влезет" : "не влезет")); row.appendChild(el("span", "sp"));
        var go = el("button", "btn p", "Скачать");
        go.addEventListener("click", function () {
          var useMm = mm && wantImg && wantImg.checked ? mm.name : null;
          invoke("hf_add", { repo: repo, file: f.name, mmproj: useMm, sizeGb: f.size_gb }).then(function (id) {
            dl[id] = "запуск…"; show("models"); return invoke("model_download", { id: id }).catch(function (er) { delete dl[id]; alert(String(er)); });
          }).then(function () { openCatalog(); }).catch(function (er) { alert(String(er)); });
        });
        row.appendChild(go); box.appendChild(row);
      });
      box.appendChild(el("p", "muted small", "Оценка «влезет» осторожная: параметры модели неизвестны, она считается плотной."));
    }).catch(function (er) { box.innerHTML = ""; box.appendChild(el("p", "warnline", String(er))); });
  }

  /* ---------- плагины: скиллы, агенты, правила ---------- */
  function loadRoles() {
    invoke("agents_list").then(function (list) {
      var s = $("roleSel"), cur = s.value; s.innerHTML = ""; var o0 = el("option", null, "Без роли"); o0.value = ""; s.appendChild(o0);
      list.forEach(function (a) { var o = el("option", null, a.name); o.value = a.name; o.title = a.description || ""; if (a.name === cur) o.selected = true; s.appendChild(o); });
    });
  }
  $("roleSel").addEventListener("change", function () { invoke("set_role", { name: $("roleSel").value || null }); clearChat(); });
  function loadPlugins() {
    var box = $("plList"); box.innerHTML = "";
    invoke("plugin_list").then(function (list) {
      if (!list.length) { box.appendChild(el("p", "empty", "Пока ничего не установлено.")); }
      list.forEach(function (p) {
        var row = el("div", "plrow"), left = el("div");
        left.appendChild(el("div", "nm", p.owner + "/" + p.repo));
        left.appendChild(el("div", "sub", "коммит " + p.sha.slice(0, 7) + " · скиллов: " + p.skills.length + " · агентов: " + p.agents.length + " · файлов: " + p.files + (p.scripts && p.scripts.length ? " · скриптов: " + p.scripts.length : "")));
        var names = p.skills.map(function (s) { return s.name; }).concat(p.agents.map(function (a) { return "агент " + a.name; }));
        if (names.length) left.appendChild(el("div", "sub", names.slice(0, 8).join(", ") + (names.length > 8 ? "…" : "")));
        var act = el("div", "act"), lab = el("label", "hostrow"), cb = el("input"); cb.type = "checkbox"; cb.checked = p.enabled;
        cb.addEventListener("change", function () { invoke("plugin_toggle", { id: p.id, on: cb.checked }).then(loadRoles); });
        lab.appendChild(cb); lab.appendChild(document.createTextNode(" включён")); act.appendChild(lab);
        var rm = el("button", "btn d", "Удалить");
        rm.addEventListener("click", function () { if (confirm("Удалить плагин " + p.owner + "/" + p.repo + "?")) invoke("plugin_remove", { id: p.id }).then(function () { loadPlugins(); loadRoles(); clearChat(); }); });
        act.appendChild(rm); row.appendChild(left); row.appendChild(act); box.appendChild(row);
      });
    });
    loadRoles();
  }
  var plProg = null;
  listen("plugin-progress", function (ev) { if (plProg) plProg(ev.payload); });
  function plPreview(src) {
    var box = $("plPrev"); box.innerHTML = ""; box.appendChild(el("p", "empty", "Проверяю репозиторий…"));
    invoke("plugin_preview", { source: src }).then(function (v) {
      box.innerHTML = ""; var c = el("div", "plcard");
      c.appendChild(el("b", null, v.owner + "/" + v.repo + (v.sub ? "/" + v.sub : "") + " · коммит " + v.sha.slice(0, 7)));
      c.appendChild(el("div", "muted small", "будет скачано файлов: " + v.files + " (" + Math.round(v.bytes / 1024) + " КБ), только текст"));
      if (v.skills.length) { c.appendChild(el("div", null, "Скиллы:")); var u = el("ul"); v.skills.forEach(function (s) { u.appendChild(el("li", null, s.name + (s.description ? ": " + s.description : ""))); }); c.appendChild(u); }
      if (v.agents.length) { c.appendChild(el("div", null, "Агенты (роли):")); var u2 = el("ul"); v.agents.forEach(function (s) { u2.appendChild(el("li", null, s.name + (s.description ? ": " + s.description : ""))); }); c.appendChild(u2); }
      var scb = null;
      if (v.scripts && v.scripts.length) {
        c.appendChild(el("div", null, "Скрипты репозитория (" + v.scripts.length + ", " + Math.round(v.script_bytes / 1024) + " КБ):"));
        var u3 = el("ul"); v.scripts.slice(0, 15).forEach(function (s) { u3.appendChild(el("li", null, s)); });
        if (v.scripts.length > 15) u3.appendChild(el("li", null, "…и ещё " + (v.scripts.length - 15))); c.appendChild(u3);
        var sl = el("label", "hostrow"); scb = el("input"); scb.type = "checkbox"; sl.appendChild(scb);
        sl.appendChild(document.createTextNode(" Установить и скрипты. Это сторонний код: он выполняется на этом компьютере с вашими правами; каждый скрипт при первом запуске в сеансе спрашивает подтверждение и показывает начало файла."));
        c.appendChild(sl);
      }
      if (v.skipped_scripts) c.appendChild(el("div", "warnline", "Не устанавливаются (" + v.skipped_scripts + " шт.): исполняемые файлы (exe, dll), TypeScript и слишком большие файлы."));
      if (v.files > 150) c.appendChild(el("div", "warnline", "Большой репозиторий: " + v.files + " файлов. Они скачаются все, порциями, установка может занять несколько минут (ход будет виден на кнопке). Не закрывайте программу до конца."));
      if (v.truncated) c.appendChild(el("div", "warnline", "Файлов больше предела (3000): установится только первая часть."));
      var a = el("div", "ra"), ok = el("button", "btn p", "Установить"), no = el("button", "btn", "Отмена");
      ok.addEventListener("click", function () {
        ok.disabled = true; ok.textContent = "Скачиваю…"; plProg = function (p) { ok.textContent = "Скачиваю… " + p.done + " из " + p.total; };
        invoke("plugin_install", { source: src, sha: v.sha, withScripts: !!(scb && scb.checked) }).then(function () { plProg = null; box.innerHTML = ""; loadPlugins(); }).catch(function (er) { plProg = null; ok.disabled = false; ok.textContent = "Установить"; alert(String(er)); });
      });
      no.addEventListener("click", function () { box.innerHTML = ""; });
      a.appendChild(ok); a.appendChild(no); c.appendChild(a); box.appendChild(c);
    }).catch(function (er) { box.innerHTML = ""; box.appendChild(el("p", "warnline", String(er))); });
  }
  $("plForm").addEventListener("submit", function (e) { e.preventDefault(); plPreview($("plSrc").value); });
  $("plSearchForm").addEventListener("submit", function (e) {
    e.preventDefault();
    var box = $("plRes"); box.innerHTML = ""; box.appendChild(el("p", "empty", "Ищу…"));
    invoke("plugin_search", { q: $("plQ").value, topic: $("plTopic").value }).then(function (list) {
      box.innerHTML = ""; if (!list.length) { box.appendChild(el("p", "empty", "Ничего не найдено.")); return; }
      list.forEach(function (r) {
        var row = el("div", "plrow"), left = el("div");
        left.appendChild(el("div", "nm", r.full_name + "  ★ " + r.stars));
        left.appendChild(el("div", "sub", (r.description || "без описания") + (r.license && r.license !== "NOASSERTION" ? " · " + r.license : "")));
        var b = el("button", "btn", "Выбрать"); b.addEventListener("click", function () { $("plSrc").value = r.full_name; plPreview(r.full_name); });
        row.appendChild(left); row.appendChild(b); box.appendChild(row);
      });
    }).catch(function (er) { box.innerHTML = ""; box.appendChild(el("p", "warnline", String(er))); });
  });

  /* ---------- О системе ---------- */
  $("sbAbout").addEventListener("click", function () {
    invoke("app_info").then(function (i) {
      var h = i.hw, b = $("aboutBody"); b.innerHTML = "";
      [["Версия", i.version], ["Процессор", h.cpu_name || "—"], ["Ядра / потоки", h.cores + " / " + h.threads], ["ОЗУ", h.ram_gb.toFixed(1) + " ГБ"],
        ["Пропускная способность памяти (оценка)", h.mem_bw_gbs.toFixed(0) + " ГБ/с"], ["Видеокарта", h.gpus.length ? h.gpus.map(function (g) { return g.name + " " + g.vram_gb.toFixed(0) + " ГБ"; }).join(", ") : "нет"],
        ["Папка данных", i.data_dir]].forEach(function (r) { var p = el("p"); p.style.margin = "4px 0"; var s = el("b", null, r[0] + ": "); s.style.color = "var(--fg)"; p.appendChild(s); p.appendChild(document.createTextNode(r[1])); b.appendChild(p); });
      $("about").hidden = false;
    });
  });
  $("aboutClose").addEventListener("click", function () { $("about").hidden = true; });
  document.addEventListener("keydown", function (e) { if (e.key === "Escape") { $("about").hidden = true; $("tuneBox").hidden = true; } });

  /* ---------- проекты на серверах (SSH) ---------- */
  var sshData = { remotes: [], active: null }, sshOld = null;
  function sshMsg(t, bad) { var m = $("sshMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--bad)" : "var(--muted)"; }
  function sshMini() {
    var r = sshData.remotes.filter(function (x) { return x.label === sshData.active; })[0];
    if (r) { $("wsMini").textContent = "Сервер: " + r.label + (r.battle ? " (боевой)" : ""); } else { setWs($("wsPath").value); }
  }
  function sshForm(r) {
    sshOld = r ? r.label : null;
    $("sshLabel").value = r ? r.label : ""; $("sshHost").value = r ? r.host : ""; $("sshPort").value = r ? r.port : 22;
    $("sshUser").value = r ? r.user : ""; $("sshKey").value = r ? r.key : ""; $("sshDir").value = r ? r.dir : "";
    $("sshBattle").checked = r ? !!r.battle : true; $("sshForm").hidden = false;
  }
  function sshBtn(row, text, fn, cls) { var b = el("button", "btn" + (cls ? " " + cls : ""), text); b.addEventListener("click", fn); row.appendChild(b); }
  function sshRender() {
    var box = $("sshList"); box.innerHTML = "";
    if (!sshData.remotes.length) { box.appendChild(el("p", "muted small", "Серверов пока нет.")); }
    sshData.remotes.forEach(function (r) {
      var on = sshData.active === r.label, row = el("div", "ra");
      var info = el("span", null, (on ? "● " : "") + r.label + (r.battle ? " [боевой]" : "") + "   " + r.user + "@" + r.host + ":" + r.port + "   " + r.dir);
      info.style.flex = "1"; info.style.minWidth = "220px"; info.style.fontWeight = on ? "600" : "400"; row.appendChild(info);
      sshBtn(row, on ? "Вернуться к папке" : "Работать здесь", function () {
        invoke("remote_use", { label: on ? null : r.label }).then(function () { clearChat(); sshLoad(); }).catch(function (e) { sshMsg(String(e), true); });
      }, on ? "" : "p");
      sshBtn(row, "Отпечаток…", function () {
        sshMsg("Запрашиваю ключ сервера…");
        invoke("remote_scan", { host: r.host, port: r.port }).then(function (keys) {
          var txt = keys.map(function (k) { return k.fp; }).join("\n");
          if (confirm("Сервер " + r.host + ":" + r.port + " предъявил ключи:\n\n" + txt + "\n\nСверьте отпечаток с тем, который вы знаете (на сервере: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub). Запомнить этот сервер?")) {
            return invoke("remote_trust", { host: r.host, port: r.port, lines: keys.map(function (k) { return k.line; }) }).then(function () { sshMsg("Отпечаток запомнен."); });
          }
          sshMsg("Отменено.");
        }).catch(function (e) { sshMsg(String(e), true); });
      });
      sshBtn(row, "Проверить", function () {
        sshMsg("Подключаюсь…");
        invoke("remote_test", { label: r.label }).then(function (t) { sshMsg(t.replace(/\n/g, "  "), false); }).catch(function (e) { sshMsg(String(e), true); });
      });
      sshBtn(row, "Изменить", function () { sshForm(r); });
      sshBtn(row, "Удалить", function () { if (confirm("Удалить сервер «" + r.label + "» из списка? Сам сервер не затрагивается.")) { invoke("remote_delete", { label: r.label }).then(function () { sshLoad(); }); } }, "d");
      box.appendChild(row);
    });
  }
  function sshLoad() { return invoke("remote_list").then(function (d) { sshData = d; sshRender(); sshMini(); }); }
  $("sshAdd").addEventListener("click", function () { sshMsg(""); sshForm(null); });
  $("sshCancel").addEventListener("click", function () { $("sshForm").hidden = true; });
  $("sshSave").addEventListener("click", function () {
    var rem = { label: $("sshLabel").value.trim(), host: $("sshHost").value.trim(), port: parseInt($("sshPort").value, 10) || 22, user: $("sshUser").value.trim(), key: $("sshKey").value.trim(), dir: $("sshDir").value.trim(), battle: $("sshBattle").checked };
    invoke("remote_save", { remote: rem, oldLabel: sshOld }).then(function () { $("sshForm").hidden = true; sshMsg("Сохранено. Нажмите «Отпечаток…», затем «Проверить»."); return sshLoad(); }).catch(function (e) { sshMsg(String(e), true); });
  });

  /* ---------- серверы MCP ---------- */
  function mcpMsg(t, bad) { var m = $("mcpMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--bad)" : "var(--muted)"; }
  function mcpLoad() {
    invoke("mcp_list").then(function (list) {
      var box = $("mcpList"); box.innerHTML = "";
      if (!list.length) { box.appendChild(el("p", "muted small", "Серверов MCP пока нет.")); return; }
      list.forEach(function (s) {
        var row = el("div", "ra"), info = el("div"); info.style.flex = "1"; info.style.minWidth = "220px";
        info.appendChild(el("b", null, s.name));
        info.appendChild(el("div", "muted small", s.command + " " + s.args.join(" ") + (s.env_keys.length ? "   env: " + s.env_keys.join(", ") : "")));
        info.appendChild(el("div", "muted small", s.status + (s.tools.length ? ": " + s.tools.slice(0, 6).join(", ") + (s.tools.length > 6 ? "…" : "") : "")));
        row.appendChild(info);
        var b1 = el("button", "btn", s.enabled ? "Выключить" : "Включить");
        b1.addEventListener("click", function () { invoke("mcp_toggle", { name: s.name, on: !s.enabled }).then(function () { mcpLoad(); setTimeout(mcpLoad, 3000); }); });
        var b2 = el("button", "btn", "Перезапустить");
        b2.addEventListener("click", function () { invoke("mcp_restart", { name: s.name }).then(function () { setTimeout(mcpLoad, 500); setTimeout(mcpLoad, 4000); }); });
        var b3 = el("button", "btn d", "Удалить");
        b3.addEventListener("click", function () { if (confirm("Удалить сервер MCP «" + s.name + "» вместе с сохранёнными ключами?")) { invoke("mcp_remove", { name: s.name }).then(mcpLoad); } });
        [b1, b2, b3].forEach(function (b) { row.appendChild(b); });
        box.appendChild(row);
      });
    });
  }
  $("mcpAdd").addEventListener("click", function () {
    var text = $("mcpText").value;
    invoke("mcp_preview", { text: text }).then(function (list) {
      var lines = list.map(function (s) { return "• " + s.name + ": " + s.command + " " + s.args.join(" ") + (s.env_keys.length ? "  (ключи: " + s.env_keys.join(", ") + ")" : ""); }).join("\n");
      if (!confirm("Будет добавлен и сразу запущен (с вашими правами):\n\n" + lines + "\n\nДобавлять только то, чему доверяете. Продолжить?")) { mcpMsg("Отменено."); return; }
      return invoke("mcp_import", { text: text }).then(function (n) { $("mcpText").value = ""; mcpMsg("Добавлено: " + n + ". Запускаю…"); mcpLoad(); setTimeout(mcpLoad, 3000); setTimeout(mcpLoad, 10000); });
    }).catch(function (e) { mcpMsg(String(e), true); });
  });
  document.querySelectorAll('[data-view="settings"]').forEach(function (b) { b.addEventListener("click", mcpLoad); });

  /* ---------- память Nexora ---------- */
  function memLoad() {
    invoke("memory_list").then(function (list) {
      var box = $("memList"); box.innerHTML = "";
      if (!list.length) { box.appendChild(el("p", "muted small", "Заметок пока нет. Агент предложит запомнить важное, а вы решите.")); return; }
      list.forEach(function (n) {
        var row = el("div", "ra");
        var t = el("span", null, (n.global ? "(общее) " : "") + n.text); t.style.flex = "1"; t.style.minWidth = "200px"; row.appendChild(t);
        var b = el("button", "btn d", "Удалить");
        b.addEventListener("click", function () { invoke("memory_delete", { id: n.id }).then(memLoad); });
        row.appendChild(b); box.appendChild(row);
      });
    });
  }
  document.querySelectorAll('[data-view="settings"]').forEach(function (b) { b.addEventListener("click", memLoad); });

  /* ---------- автоматизация ---------- */
  var taskData = { tasks: [], ready: false, project: {} };
  function taskMsg(t, bad) { var m = $("taskMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--bad)" : "var(--muted)"; }
  function taskWhen(t) { return t.kind === "daily" ? "каждый день в " + t.at : "каждые " + t.every_min + " мин"; }
  function taskRender() {
    var box = $("taskList"); box.innerHTML = "";
    if (!taskData.tasks.length) { box.appendChild(el("p", "muted small", "Задач пока нет.")); }
    taskData.tasks.forEach(function (t) {
      var row = el("div", "ra"), info = el("div");
      info.style.flex = "1"; info.style.minWidth = "220px";
      info.appendChild(el("b", null, (t.enabled ? "" : "(выкл) ") + t.name));
      info.appendChild(el("div", "muted small", taskWhen(t) + "   Проект: " + (t.remote ? "сервер " + t.remote : t.workspace)));
      info.appendChild(el("div", "muted small", t.last_run && t.last_status ? "Последний запуск: " + new Date(t.last_run * 1000).toLocaleString("ru-RU") + ", " + t.last_status : "Ещё не запускалась"));
      row.appendChild(info);
      var b1 = el("button", "btn p", "Запустить сейчас");
      b1.addEventListener("click", function () { taskMsg("Запускаю…"); invoke("task_run_now", { id: t.id }).then(function () { taskMsg("Выполняется в фоне, результат появится в списке."); }).catch(function (e) { taskMsg(String(e), true); }); });
      var b2 = el("button", "btn", "Результат");
      b2.addEventListener("click", function () { invoke("task_result", { id: t.id }).then(function (r) { $("taskResTitle").textContent = t.name; $("taskRes").textContent = r; $("taskResBox").hidden = false; }).catch(function (e) { taskMsg(String(e), true); }); });
      var b3 = el("button", "btn", t.enabled ? "Выключить" : "Включить");
      b3.addEventListener("click", function () { invoke("task_toggle", { id: t.id, on: !t.enabled }).then(taskLoad); });
      var b4 = el("button", "btn d", "Удалить");
      b4.addEventListener("click", function () { if (confirm("Удалить задачу «" + t.name + "»?")) { invoke("task_delete", { id: t.id }).then(taskLoad); } });
      [b1, b2, b3, b4].forEach(function (b) { row.appendChild(b); });
      box.appendChild(row);
    });
  }
  function taskLoad() { return invoke("tasks_list").then(function (d) { taskData = d; taskRender(); }); }
  function taskKindUi() { var every = $("taskKind").value === "every"; $("taskAt").hidden = every; $("taskEvery").hidden = !every; }
  $("taskKind").addEventListener("change", taskKindUi);
  $("taskAdd").addEventListener("click", function () {
    taskMsg(""); var p = taskData.project || {};
    $("taskProj").textContent = "Проект задачи: " + (p.remote ? "сервер " + p.remote : p.workspace || "не выбран (откройте папку или сервер на странице «Проекты»)");
    $("taskForm").hidden = false;
  });
  $("taskCancel").addEventListener("click", function () { $("taskForm").hidden = true; });
  $("taskSave").addEventListener("click", function () {
    invoke("task_save", { name: $("taskName").value, prompt: $("taskPrompt").value, kind: $("taskKind").value, at: $("taskAt").value, everyMin: parseInt($("taskEvery").value, 10) || 0 })
      .then(function () { $("taskForm").hidden = true; $("taskName").value = ""; $("taskPrompt").value = ""; taskMsg("Задача создана."); return taskLoad(); })
      .catch(function (e) { taskMsg(String(e), true); });
  });
  document.querySelectorAll('[data-view="auto"]').forEach(function (b) { b.addEventListener("click", taskLoad); });
  listen("tasks-changed", taskLoad);

  /* ---------- запуск ---------- */
  invoke("get_state").then(function (s) {
    $("mode").value = s.mode; $("autoFolder").checked = s.auto_folder; $("autoChk").checked = !!s.auto; syncAuto();
    $("rootLbl").textContent = "Новые проекты: " + s.projects_root;
    if (s.workspace) { setWs(s.workspace); }
    showProfile(s.profile || "local"); setSrc(s.source); $("extUrl").value = s.ext_url || ""; ctxVal = s.ctx || 0; showCtx(); loadModels(s.model);
  });
  sshLoad(); setInd(ind); applyAnim(); renderBot(); armSleep(); updateWelcome(); autosize(); show("chat"); loadRoles();
  startBtn.click();
})();
