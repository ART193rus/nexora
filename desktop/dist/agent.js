/* Nexora Agent: подключение этого ПК к серверу Nexora (страница «Соединения»). */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T || !document.getElementById("agBox")) return;
  var invoke = T.core.invoke;
  var $ = function (id) { return document.getElementById(id); };
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function say(t, bad) { var m = $("agMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--err, #ff5c7a)" : ""; }
  var fpOk = null, poll = null;

  function render(s) {
    $("agConnected").hidden = !s.connected; $("agSetup").hidden = !!s.connected;
    if (s.connected) $("agConnText").textContent = "Привязано к серверу " + s.server.host + ":" + s.server.port + " (отпечаток " + s.server.short + "), имя устройства «" + s.name + "»." + (s.active ? " Сейчас используется модель сервера." : "");
    if (!$("agName").value) $("agName").value = s.default_name || "";
  }
  function load() { return invoke("agent_get").then(render).catch(function () {}); }
  function stopPoll() { if (poll) { clearInterval(poll); poll = null; } }

  $("agFind").addEventListener("click", function () {
    var box = $("agFound"); box.innerHTML = ""; say("Ищу серверы в сети (несколько секунд)…");
    invoke("discover_browse").then(function (list) {
      if (!list.length) { say("Серверы не найдены. На сервере должна быть включена галочка «Виден в локальной сети»; можно вставить строку подключения вручную.", true); return; }
      say("Найдено: " + list.length + ". Нажмите на нужный сервер, затем «Проверить сервер».");
      list.forEach(function (f) {
        var b = el("button", "btn", f.name + "   " + f.host + ":" + f.port + "   " + f.fp);
        b.addEventListener("click", function () { $("agLink").value = "nexora://" + f.host + ":" + f.port + "#" + f.fp; fpOk = null; $("agFp").textContent = ""; say("Теперь нажмите «Проверить сервер» и сверьте отпечаток."); });
        box.appendChild(b);
      });
    }).catch(function (e) { say(String(e), true); });
  });

  $("agCheck").addEventListener("click", function () {
    var fpBox = $("agFp"); fpBox.innerHTML = ""; fpOk = null; say("Проверяю сервер…");
    invoke("agent_probe", { link: $("agLink").value }).then(function (r) {
      say("");
      fpBox.appendChild(el("div", null, "Отпечаток сервера: " + r.short));
      if (r.matches === true) { fpOk = r.fp; fpBox.appendChild(el("div", "muted", "Совпадает со строкой подключения.")); }
      else if (r.matches === false) { fpBox.appendChild(el("div", null, "ВНИМАНИЕ: отпечаток НЕ совпадает со строкой. Это может быть другой сервер или подмена: не продолжайте.")); fpBox.lastChild.style.color = "var(--err, #ff5c7a)"; }
      else {
        fpBox.appendChild(el("div", "muted", "Сверьте этот код с экраном сервера (страница «Сервер», данные для привязки)."));
        var b = el("button", "btn p", "Код совпадает, это мой сервер");
        b.addEventListener("click", function () { fpOk = r.fp; fpBox.innerHTML = ""; fpBox.appendChild(el("div", "muted", "Отпечаток подтверждён: " + r.short)); });
        fpBox.appendChild(b);
      }
    }).catch(function (e) { say(String(e), true); });
  });

  $("agEnroll").addEventListener("click", function () {
    say("Привязываю…");
    invoke("agent_enroll", { link: $("agLink").value, fp: fpOk, name: $("agName").value, code: $("agCode").value }).then(function (s) { $("agCode").value = ""; say("Готово: устройство привязано."); render(s); }).catch(function (e) { say(String(e), true); });
  });

  $("agQr").addEventListener("click", function () {
    var box = $("agQrBox"); stopPoll();
    invoke("agent_pair_start", { link: $("agLink").value, fp: fpOk, name: $("agName").value }).then(function (r) {
      box.hidden = false; box.innerHTML = "";
      box.appendChild(el("p", "small", "1. На телефоне, уже привязанном к серверу, откройте в приложении «Добавить устройство» и отсканируйте этот QR-код."));
      var qr = el("div"); qr.style.cssText = "background:#fff;padding:8px;border-radius:8px;width:max-content"; qr.innerHTML = r.qr; box.appendChild(qr);
      var chk = el("p", "small", "2. Сверьте код на телефоне с этим: "); var b = el("b", null, r.check); chk.appendChild(b); box.appendChild(chk);
      box.appendChild(el("p", "small", "3. Введите на телефоне код из Authenticator. Этот экран сам продолжит, когда сервер выдаст сертификат (до 10 минут)."));
      var cancel = el("button", "btn", "Отмена");
      cancel.addEventListener("click", function () { stopPoll(); invoke("agent_pair_cancel"); box.hidden = true; say("Отменено."); });
      box.appendChild(cancel);
      say("Ждём подтверждения с телефона…");
      poll = setInterval(function () {
        invoke("agent_pair_poll").then(function (p) {
          if (p.done) { stopPoll(); box.hidden = true; say("Готово: устройство привязано телефоном."); render(p.state); }
        }).catch(function (e) { stopPoll(); box.hidden = true; say(String(e), true); });
      }, 2500);
    }).catch(function (e) { say(String(e), true); });
  });

  $("agConnect").addEventListener("click", function () {
    say("Подключаюсь к серверу…");
    invoke("agent_connect").then(function () { say("Подключено: модель сервера выбрана. Откройте «Чат / Агент»."); load(); }).catch(function (e) { say(String(e), true); });
  });

  $("agForget").addEventListener("click", function () {
    if (!confirm("Забыть сервер? Сертификат этого ПК будет удалён, для нового подключения потребуется привязка заново.")) return;
    invoke("agent_forget").then(function (s) { fpOk = null; $("agFp").textContent = ""; say("Сервер забыт."); render(s); }).catch(function (e) { say(String(e), true); });
  });

  document.querySelectorAll('[data-view="links"]').forEach(function (b) { b.addEventListener("click", load); });
  load();
})();
