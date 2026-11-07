/* Nexora Server: поиск в локальной сети (mDNS). Отдельный файл, чтобы не смешивать с host.js. */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T || !document.getElementById("discVisible")) return;
  var invoke = T.core.invoke;
  var $ = function (id) { return document.getElementById(id); };
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function say(t, bad) { var m = $("discMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--err, #ff5c7a)" : ""; }

  function syncVisible() {
    invoke("host_state").then(function (s) { $("discVisible").checked = !!s.visible; }).catch(function () {});
  }

  $("discVisible").addEventListener("change", function () {
    var on = $("discVisible").checked;
    invoke("host_visible", { on: on }).then(function (s) {
      if (s.visible_error) { say("Не удалось объявить сервер: " + s.visible_error, true); return; }
      if (on && !s.running) { say("Настройка сохранена. Сервер сейчас выключен: он станет виден после включения."); return; }
      say(on ? "Сервер виден в локальной сети." : "Сервер скрыт.");
    }).catch(function (e) { say(String(e), true); });
  });

  $("discFind").addEventListener("click", function () {
    var box = $("discList"); box.innerHTML = ""; say("Ищу серверы в сети (несколько секунд)…");
    $("discFind").disabled = true;
    invoke("discover_browse").then(function (list) {
      $("discFind").disabled = false;
      if (!list.length) {
        say("Ничего не найдено. Проверьте: сервер включён и «Виден в локальной сети» отмечено; устройства в одной сети Wi-Fi или по кабелю (не гостевая сеть и не мобильный интернет); Windows разрешил программе доступ в частной сети.", true);
        return;
      }
      say("Найдено: " + list.length + ". Отпечаток сверяйте с данными для привязки на экране сервера.");
      list.forEach(function (f) {
        var row = el("div", "ra");
        row.appendChild(el("span", null, f.name + "   nexora://" + f.host + ":" + f.port + "   отпечаток " + f.fp));
        box.appendChild(row);
      });
    }).catch(function (e) { $("discFind").disabled = false; say(String(e), true); });
  });


  /* ---------- доступ из любой точки (Tailscale) ---------- */
  var remBox = $("remVisible");
  if (remBox) {
    var remSay = function (t, bad) { var m = $("remStatus"); m.textContent = t || ""; m.style.color = bad ? "var(--err, #ff5c7a)" : ""; };
    var remRefresh = function () {
      invoke("host_remote").then(function (r) {
        remBox.checked = !!r.enabled;
        if (!r.adapters.length) { remSay("Tailscale (или другой VPN) на этом ПК не найден. Установите его, войдите в аккаунт и нажмите «Показать адрес…» ещё раз.", false); return; }
        var a = r.adapters.map(function (x) { return x.ip + " (" + x.name + ")"; }).join(", ");
        remSay("Найден адрес VPN: " + a + ". " + (r.listening ? "Сервер слушает и на нём: подключаться можно из любой сети." : r.enabled ? "Включено, но слушатель не запущен (включите сервер)." : "Отметьте галочку, чтобы слушать и на этом адресе."), false);
      }).catch(function (e) { remSay(String(e), true); });
    };
    remBox.addEventListener("change", function () {
      invoke("host_overlay", { on: remBox.checked }).then(function (s) {
        if (s.overlay_error) { remSay(s.overlay_error, true); return; }
        remRefresh();
      }).catch(function (e) { remSay(String(e), true); });
    });
    $("remShow").addEventListener("click", function () {
      var box = $("remInfo"); box.innerHTML = "";
      invoke("host_remote").then(function (r) {
        remRefresh();
        if (!r.link) { box.appendChild(el("p", "muted small", "Адрес появится, когда на ПК будет установлен Tailscale, а сервер включён и настроен.")); return; }
        box.appendChild(el("p", "small", "Строка для подключения из любой сети (вместо домашней):"));
        var code = el("div", "small", r.link); code.style.cssText = "word-break:break-all;user-select:all;font-family:monospace"; box.appendChild(code);
        var qr = el("div"); qr.style.cssText = "background:#fff;padding:6px;border-radius:8px;width:max-content;margin-top:8px"; qr.innerHTML = r.link_qr; box.appendChild(qr);
        box.appendChild(el("p", "muted small", "Телефон тоже должен быть в вашем Tailscale. Отпечаток в строке тот же, что и дома: ранее привязанные устройства подключаются без новой привязки."));
      }).catch(function (e) { remSay(String(e), true); });
    });
    remRefresh();
    setInterval(function () { if (!$("v-server").hidden) remRefresh(); }, 15000);
  }

  syncVisible();
  setInterval(function () { if (!$("v-server").hidden) syncVisible(); }, 4000);
})();
