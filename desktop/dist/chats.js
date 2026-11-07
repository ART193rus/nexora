/* Nexora: список сохранённых разговоров (раздел «Чаты»). Отдельный файл; окно чата берёт из app.js нажатием «Новый чат». */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T || !document.getElementById("v-chats")) return;
  var invoke = T.core.invoke, listen = T.event.listen;
  var $ = function (id) { return document.getElementById(id); };
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function say(t, bad) { var m = $("chatsMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--bad)" : "var(--muted)"; }
  function when(ts) { var d = new Date(ts * 1000); return d.toLocaleDateString("ru-RU") + " " + d.toLocaleTimeString("ru-RU").slice(0, 5); }
  function go(view) { var b = document.querySelector('#nav [data-view="' + view + '"]'); if (b) b.click(); }

  function load() {
    invoke("chats_list").then(function (d) {
      var box = $("chatsList"); box.innerHTML = "";
      if (!d.chats.length) { box.appendChild(el("p", "muted small", "Сохранённых разговоров пока нет. Они появятся после первого ответа.")); return; }
      d.chats.forEach(function (c) {
        var cur = d.current === c.id, row = el("div", "ra"), info = el("div");
        info.style.flex = "1"; info.style.minWidth = "220px";
        info.appendChild(el("b", null, (cur ? "● " : "") + c.title));
        info.appendChild(el("div", "muted small", when(c.updated) + " · сообщений: " + c.turns + (c.project ? " · " + c.project : "")));
        row.appendChild(info);
        var open = el("button", "btn p", cur ? "Текущий" : "Открыть");
        open.disabled = cur;
        open.addEventListener("click", function () { openChat(c.id); });
        var ren = el("button", "btn", "Переименовать");
        ren.addEventListener("click", function () {
          var t = window.prompt("Название разговора", c.title);
          if (t && t.trim()) invoke("chat_rename", { id: c.id, title: t }).then(load).catch(function (e) { say(String(e), true); });
        });
        var del = el("button", "btn d", "Удалить");
        del.addEventListener("click", function () { if (confirm("Удалить разговор «" + c.title + "»? Это нельзя отменить.")) invoke("chat_delete", { id: c.id }).then(load); });
        [open, ren, del].forEach(function (b) { row.appendChild(b); });
        box.appendChild(row);
      });
    }).catch(function (e) { say(String(e), true); });
  }

  function addMsg(role, text) {
    var log = $("log"), m = el("div", "msg " + (role === "user" ? "user" : "ai"));
    m.appendChild(el("b", null, role === "user" ? "Вы" : "Модель"));
    var body = el("div"); body.textContent = text; m.appendChild(body);
    log.appendChild(m);
  }

  function openChat(id) {
    say("Открываю…");
    // «Новый чат» очищает окно и состояние в app.js, затем подгружаем выбранный разговор
    $("newChat").click();
    invoke("chat_open", { id: id }).then(function (r) {
      r.messages.forEach(function (m) { addMsg(m.role, m.text); });
      var w = $("welcome"); if (w) w.hidden = r.messages.length > 0;
      var log = $("log"); log.scrollTop = log.scrollHeight;
      if (r.workspace) { $("wsPath").value = r.workspace; $("wsMini").textContent = r.remote ? "Сервер: " + r.remote : "Проект: " + r.workspace; }
      else if (r.remote) { $("wsMini").textContent = "Сервер: " + r.remote; }
      say("");
      go("chat");
    }).catch(function (e) { say(String(e), true); });
  }

  $("chatsNew").addEventListener("click", function () { $("newChat").click(); go("chat"); });
  document.querySelectorAll('[data-view="chats"]').forEach(function (b) { b.addEventListener("click", load); });
  listen("chat-done", function () { if (!$("v-chats").hidden) load(); });
})();
