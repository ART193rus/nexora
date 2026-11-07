/* Nexora: переключатели в поле ввода: «Рассуждение» и «Веб-поиск». Настройки общие и сохраняются между запусками. */
(function () {
  "use strict";
  var T = window.__TAURI__;
  var think = document.getElementById("thinkBtn"), web = document.getElementById("webBtn");
  if (!T || !think || !web) return;
  var invoke = T.core.invoke, st = { think: false, web: false };

  function paint() {
    think.classList.toggle("on", st.think); think.setAttribute("aria-pressed", st.think ? "true" : "false");
    think.title = st.think ? "Рассуждение включено: модель думает перед ответом (точнее, но дольше). Нажмите, чтобы отключить" : "Рассуждение выключено: модель отвечает сразу (быстрее). Нажмите, чтобы включить";
    web.classList.toggle("on", st.web); web.setAttribute("aria-pressed", st.web ? "true" : "false");
    web.title = st.web ? "Веб-поиск включён: модель может искать в интернете и читать страницы. Нажмите, чтобы отключить" : "Веб-поиск выключен. Нажмите, чтобы разрешить модели искать в интернете и читать страницы";
  }
  function apply(p) { st = p; paint(); }

  think.addEventListener("click", function () { invoke("set_think", { on: !st.think }).then(apply); });
  web.addEventListener("click", function () { invoke("set_web", { on: !st.web }).then(apply); });
  paint();
  invoke("get_prefs").then(apply).catch(function () {});
})();
