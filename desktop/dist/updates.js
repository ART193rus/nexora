/* Nexora: обновления с подписью (раздел «Настройки»). Ничего не скачивается без нажатия кнопки. */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T || !document.getElementById("updCheck")) return;
  var invoke = T.core.invoke;
  var $ = function (id) { return document.getElementById(id); };
  function say(t, bad) { var m = $("updMsg"); m.textContent = t || ""; m.style.color = bad ? "var(--err, #ff5c7a)" : ""; }

  function load() {
    invoke("update_state").then(function (s) {
      $("updVer").textContent = "Установлена версия " + s.version + (s.key_set ? "" : ". В этой сборке нет ключа проверки подписи: обновления отключены.");
      if (document.activeElement !== $("updUrl")) $("updUrl").value = s.url || "";
      $("updCheck").disabled = !s.key_set;
    }).catch(function () {});
  }

  $("updSave").addEventListener("click", function () {
    invoke("update_set_url", { url: $("updUrl").value }).then(function () { say("Адрес сохранён."); load(); }).catch(function (e) { say(String(e), true); });
  });

  $("updCheck").addEventListener("click", function () {
    $("updInstall").hidden = true; say("Проверяю…");
    invoke("update_set_url", { url: $("updUrl").value }).then(function () { return invoke("update_check"); }).then(function (r) {
      if (!r.available) { say("У вас последняя версия."); return; }
      say("Доступна версия " + r.version + (r.notes ? ": " + r.notes : "") + ". Подпись проверена.");
      $("updInstall").hidden = false;
    }).catch(function (e) { say(String(e), true); });
  });

  $("updInstall").addEventListener("click", function () {
    if (!confirm("Скачать и запустить установщик новой версии? Подпись и контрольная сумма проверяются до запуска.")) return;
    $("updInstall").disabled = true; say("Скачиваю и проверяю…");
    invoke("update_install").then(function (t) { say(t); }).catch(function (e) { say(String(e), true); }).then(function () { $("updInstall").disabled = false; });
  });

  document.querySelectorAll('[data-view="settings"]').forEach(function (b) { b.addEventListener("click", load); });
  load();
})();
