/* Nexora: трей и запуск вместе с Windows (раздел «Настройки»). */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T || !document.getElementById("dskTray")) return;
  var invoke = T.core.invoke, listen = T.event.listen;
  var $ = function (id) { return document.getElementById(id); };
  function say(t, bad) { var m = $("dskMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--err, #ff5c7a)" : ""; }

  function paint(s) {
    $("dskTray").checked = !!s.tray; $("dskAuto").checked = !!s.autostart; $("dskServer").checked = !!s.server_autorun;
    var n = "Выйти из программы совсем (остановить модель и сервер) можно через значок в трее: правая кнопка, «Выход». ";
    if (s.autostart && !s.autostart_here) n += "Автозапуск сейчас указывает на другой файл программы, снимите и включите галочку заново, чтобы он указывал на этот. ";
    n += s.tray_icon ? "Свой значок трея: " + s.tray_icon + "." : "Свой значок трея: положите файл tray.png или tray.ico рядом с программой или в папку %LOCALAPPDATA%\\Nexora и перезапустите программу.";
    $("dskNote").textContent = n;
  }
  function load() { invoke("desktop_get").then(paint).catch(function () {}); }

  $("dskTray").addEventListener("change", function () { invoke("desktop_set_tray", { on: $("dskTray").checked }).then(function () { say($("dskTray").checked ? "Теперь крестик сворачивает в трей." : "Теперь крестик закрывает программу."); }); });
  $("dskAuto").addEventListener("change", function () {
    invoke("desktop_set_autostart", { on: $("dskAuto").checked }).then(function (s) { paint(s); say(s.autostart ? "Программа будет запускаться вместе с Windows." : "Автозапуск выключен."); }).catch(function (e) { say(String(e), true); load(); });
  });
  $("dskServer").addEventListener("change", function () {
    invoke("desktop_set_server_autorun", { on: $("dskServer").checked }).then(function (s) { paint(s); say(s.server_autorun ? "Сервер будет включаться при запуске программы (адрес и порт берутся из страницы «Сервер»)." : "Сервер при запуске включаться не будет."); }).catch(function (e) { say(String(e), true); });
  });

  listen("host-autostart-failed", function (ev) { say("Сервер не удалось включить при запуске: " + (ev.payload && ev.payload.error), true); });
  document.querySelectorAll('[data-view="settings"]').forEach(function (b) { b.addEventListener("click", load); });
  load();
})();
