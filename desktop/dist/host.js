/* Nexora Server: страница «Сервер» (режим для телефона). Секрет Authenticator показывается только здесь, на этом ПК. */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T) return;
  var invoke = T.core.invoke;
  var $ = function (id) { return document.getElementById(id); };
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  var timer = null, st = null, setupOpen = false;

  function fmt(ts) { var d = new Date(ts * 1000); return d.toLocaleDateString() + " " + d.toLocaleTimeString().slice(0, 5); }
  function say(t, bad) { var m = $("hostMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--err, #ff5c7a)" : ""; }

  function render() {
    if (!st) return;
    $("hostState").textContent = st.running ? "Сервер включён: слушает " + st.bind + ":" + st.port : "Сервер выключен";
    $("hostToggle").textContent = st.running ? "Выключить" : "Включить";
    $("hostToggle").classList.toggle("p", !st.running);
    if (document.activeElement !== $("hostBind")) $("hostBind").value = st.bind;
    if (document.activeElement !== $("hostPort")) $("hostPort").value = st.port;
    if (document.activeElement !== $("hostNames")) $("hostNames").value = (st.names || []).join(", ");
    $("hostBind").disabled = $("hostPort").disabled = $("hostNames").disabled = st.running;
    $("hostLan").textContent = st.lan_ip ? "Адрес этого ПК в локальной сети: " + st.lan_ip : "Адрес в локальной сети определить не удалось";
    $("hostTools").checked = !!st.tools;
    $("hostRun").hidden = !st.running;
    if (!st.running) { setupOpen = false; $("hostSetup").hidden = true; return; }
    $("hostWin").textContent = st.window_open ? "Окно привязки открыто (до 10 минут, закроется после первой привязки)" : "Окно привязки закрыто";
    var d = $("hostDevs"); d.textContent = "";
    var list = st.devices || [];
    if (!list.length) d.appendChild(el("p", "muted small", "Устройств пока нет."));
    list.forEach(function (x) {
      var r = el("div", "ra");
      r.appendChild(el("span", null, x.name + (x.revoked ? " (отозвано)" : "")));
      r.appendChild(el("span", "muted small", "добавлено " + fmt(x.added)));
      if (!x.revoked) {
        // права режима агента на создание и редактирование файлов выдаются только вручную здесь, на ПК
        var rights = el("span", x.trust_pending ? "small" : "muted small", x.trust_pending ? "● Запрашивает права на создание и редактирование файлов" : x.trusted ? "Права на файлы выданы" : "Права на файлы не выданы");
        if (x.trust_pending) rights.style.fontWeight = "600";
        r.appendChild(rights);
        function decide(ok) { invoke("host_trust", { serial: x.serial, ok: ok }).then(refresh).catch(function (e) { say(String(e), true); }); }
        if (!x.trusted) {
          var gb = el("button", "btn p", x.trust_pending ? "Предоставить" : "Предоставить права…");
          gb.addEventListener("click", function () {
            if (confirm("Разрешить устройству «" + x.name + "» в режиме агента создавать и редактировать файлы и выполнять обычные действия без вопросов на телефоне?\n\nУдаление и опасные действия всё равно потребуют подтверждения. Права можно снять в любой момент.")) decide(true);
          });
          r.appendChild(gb);
          if (x.trust_pending) { var nb = el("button", "btn", "Отклонить"); nb.addEventListener("click", function () { decide(false); }); r.appendChild(nb); }
        } else {
          var rb = el("button", "btn", "Снять права");
          rb.addEventListener("click", function () { decide(false); });
          r.appendChild(rb);
        }
        var b = el("button", "btn", "Отозвать");
        b.addEventListener("click", function () { if (confirm("Отозвать доступ устройства «" + x.name + "»? Оно сразу перестанет подключаться.")) invoke("host_revoke", { serial: x.serial }).then(refresh); });
        r.appendChild(b);
      }
      d.appendChild(r);
    });
    var lg = $("hostLog"); lg.textContent = "";
    (st.log || []).slice(0, 20).forEach(function (l) { lg.appendChild(el("div", "muted small", fmt(l.ts) + "  " + l.ip + "  " + l.event)); });
  }

  function refresh() { return invoke("host_state").then(function (s) { st = s; render(); }); }

  function showSetup() {
    invoke("host_setup").then(function (i) {
      var b = $("hostSetup"); b.hidden = false; b.textContent = "";
      b.appendChild(el("h3", null, "Привязка телефона"));
      b.appendChild(el("p", "muted small", "1. В Google Authenticator: «+» → «Сканировать QR-код» (или «Ввести ключ настройки»). Секрет показывается только здесь и в приложении на телефоне не хранится."));
      var qr = el("div"); qr.style.cssText = "background:#fff;padding:6px;border-radius:8px;width:max-content"; qr.innerHTML = i.otpauth_qr; b.appendChild(qr);
      b.appendChild(el("p", "small", "Ключ: " + i.secret.replace(/(.{4})/g, "$1 ").trim()));
      b.appendChild(el("p", "muted small", "2. Нажмите «Открыть окно привязки», затем в приложении Nexora Agent вставьте строку подключения и введите 6-значный код."));
      b.appendChild(el("p", "muted small", "Или отсканируйте этот QR в приложении Nexora Agent (кнопка «Сканировать QR» на экране подключения):"));
      var qr2 = el("div"); qr2.style.cssText = "background:#fff;padding:6px;border-radius:8px;width:max-content"; qr2.innerHTML = i.link_qr; b.appendChild(qr2);
      var l = el("p", "small", i.link); l.style.wordBreak = "break-all"; b.appendChild(l);
      var cp = el("button", "btn", "Скопировать строку"); cp.addEventListener("click", function () { navigator.clipboard.writeText(i.link).then(function () { say("Строка скопирована"); }); }); b.appendChild(cp);
      b.appendChild(el("p", "muted small", "Сверьте отпечаток на телефоне, если он показан: " + i.fingerprint_short));
    }).catch(function (e) { say(String(e), true); });
  }

  $("hostToggle").addEventListener("click", function () {
    say("");
    if (st && st.running) { invoke("host_disable").then(function (s) { st = s; render(); }); return; }
    var names = $("hostNames").value.split(",").map(function (x) { return x.trim(); }).filter(Boolean);
    invoke("host_enable", { bind: $("hostBind").value, port: parseInt($("hostPort").value, 10) || 0, names: names })
      .then(function (s) { st = s; render(); })
      .catch(function (e) { say(String(e), true); });
  });
  $("hostWinBtn").addEventListener("click", function () { invoke("host_window").then(refresh).catch(function (e) { say(String(e), true); }); });
  $("hostSetupBtn").addEventListener("click", function () { setupOpen = !setupOpen; if (setupOpen) showSetup(); else $("hostSetup").hidden = true; });
  $("hostTools").addEventListener("change", function () {
    var on = $("hostTools").checked;
    if (on && !confirm("Разрешить агенту по запросу с телефона создавать файлы и запускать команды на этом ПК? Каждое изменение будет требовать подтверждения, удаление и права администратора ещё и код Authenticator.")) { $("hostTools").checked = false; return; }
    invoke("host_tools", { on: on }).then(function (s) { st = s; render(); });
  });

  function enter() { refresh(); if (!timer) timer = setInterval(function () { if ($("v-server").hidden) { clearInterval(timer); timer = null; } else refresh(); }, 3000); }
  document.querySelectorAll('[data-view="server"]').forEach(function (b) { b.addEventListener("click", enter); });
})();
