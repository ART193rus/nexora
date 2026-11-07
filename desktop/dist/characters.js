/* Nexora: персонажи-индикаторы. Внешний вид взят из исходных картинок без изменений:
   base (корпус, лицо стёрто) + glyph (оригинальное лицо, моргает) + векторное лицо для состояний.
   NxChar.html(семейство, состояние, размер) -> разметка; NxChar.set(элемент, состояние) меняет состояние.
   Состояния: idle loading thinking generating coding executing success error offline sleep
   (reading = thinking, processing = executing). Размеры: 32, 48, 64, 96 (по большей стороне). */
(function (root) {
  "use strict";
  var META = {"nexora": {"w": 236, "h": 207, "vx": 72, "vy": 47, "vw": 116, "vh": 80, "dark": "#100e24", "glow": "#e2d2ec", "fx": 72, "fy": 53, "fw": 116, "fh": 70}, "qwen": {"w": 202, "h": 207, "vx": 56, "vy": 28, "vw": 117, "vh": 100, "dark": "#0e0d21", "glow": "#d9b5e9", "fx": 56, "fy": 53, "fw": 117, "fh": 69}, "deepseek": {"w": 211, "h": 206, "vx": 71, "vy": 45, "vw": 120, "vh": 79, "dark": "#081125", "glow": "#5cd2eb", "fx": 71, "fy": 50, "fw": 120, "fh": 71}, "llama": {"w": 189, "h": 215, "vx": 55, "vy": 67, "vw": 109, "vh": 69, "dark": "#071223", "glow": "#83dfe2", "fx": 55, "fy": 71, "fw": 109, "fh": 63}, "gemma": {"w": 195, "h": 202, "vx": 38, "vy": 42, "vw": 141, "vh": 81, "dark": "#110d26", "glow": "#e0c1e4", "fx": 38, "fy": 53, "fw": 141, "fh": 68}, "moe": {"w": 223, "h": 205, "vx": 77, "vy": 44, "vw": 102, "vh": 79, "dark": "#0e0d21", "glow": "#ddc2f1", "fx": 77, "fy": 49, "fw": 102, "fh": 71}};
  var STATES = ["idle", "loading", "thinking", "generating", "coding", "executing", "success", "error", "offline", "sleep"];
  var ALIAS = { reading: "thinking", processing: "executing" };
  var TITLE = { idle: "ожидает", loading: "загружается", thinking: "думает", generating: "пишет ответ", coding: "правит код",
    executing: "выполняет", success: "готово", error: "ошибка", offline: "нет связи", sleep: "спит" };
  var NAME = { nexora: "NEXORA", qwen: "Qwen", deepseek: "DeepSeek", llama: "Llama", gemma: "Gemma", moe: "MoE Expert" };

  function family(name) {
    var n = String(name || "").toLowerCase();
    if (n.indexOf("qwen") >= 0) return "qwen";
    if (n.indexOf("deepseek") >= 0) return "deepseek";
    if (n.indexOf("llama") >= 0) return "llama";
    if (n.indexOf("gemma") >= 0) return "gemma";
    if (n.indexOf("gpt-oss") >= 0 || n.indexOf("moe") >= 0) return "moe";
    return "nexora";
  }
  function norm(s) { s = ALIAS[s] || s; return STATES.indexOf(s) >= 0 ? s : "idle"; }

  /* лицо в системе координат 68x40 (центр 34,20) */
  var FACE =
    '<g class="f f-loading"><circle cx="34" cy="20" r="10" class="st ring spin"/><circle cx="34" cy="20" r="2" class="fl"/></g>' +
    '<g class="f f-thinking"><g class="eyes"><circle cx="24" cy="14" r="4.4" class="fl"/><circle cx="44" cy="14" r="4.4" class="fl"/></g>' +
      '<circle cx="24" cy="29" r="2.4" class="fl dd d1"/><circle cx="34" cy="29" r="2.4" class="fl dd d2"/><circle cx="44" cy="29" r="2.4" class="fl dd d3"/></g>' +
    '<g class="f f-generating"><rect x="13" y="17" width="4.4" height="6" rx="2.2" class="fl bar b1"/><rect x="20.5" y="17" width="4.4" height="6" rx="2.2" class="fl bar b2"/>' +
      '<rect x="28" y="17" width="4.4" height="6" rx="2.2" class="fl bar b3"/><rect x="35.5" y="17" width="4.4" height="6" rx="2.2" class="fl bar b4"/>' +
      '<rect x="43" y="17" width="4.4" height="6" rx="2.2" class="fl bar b5"/><rect x="50.5" y="17" width="4.4" height="6" rx="2.2" class="fl bar b6"/></g>' +
    '<g class="f f-coding"><path d="M26 11l-10 9 10 9" class="st cc c1"/><path d="M37 9l-6 22" class="st cc c2"/><path d="M42 11l10 9-10 9" class="st cc c3"/></g>' +
    '<g class="f f-executing"><g class="spin slow"><circle cx="34" cy="20" r="6.5" class="st thin"/>' +
      '<rect x="32.2" y="8" width="3.6" height="5.4" rx="1.1" class="fl"/><rect x="32.2" y="8" width="3.6" height="5.4" rx="1.1" class="fl" transform="rotate(60 34 20)"/>' +
      '<rect x="32.2" y="8" width="3.6" height="5.4" rx="1.1" class="fl" transform="rotate(120 34 20)"/><rect x="32.2" y="8" width="3.6" height="5.4" rx="1.1" class="fl" transform="rotate(180 34 20)"/>' +
      '<rect x="32.2" y="8" width="3.6" height="5.4" rx="1.1" class="fl" transform="rotate(240 34 20)"/><rect x="32.2" y="8" width="3.6" height="5.4" rx="1.1" class="fl" transform="rotate(300 34 20)"/></g></g>' +
    '<g class="f f-success"><path d="M14 24q7-15 14 0M40 24q7-15 14 0" class="st"/><path d="M27 29q7 6 14 0" class="st thin"/></g>' +
    '<g class="f f-error"><path d="M24 11l20 18M44 11l-20 18" class="st errc"/></g>' +
    '<g class="f f-offline"><path d="M17 17q17-13 34 0M24 23q10-8 20 0" class="st dim"/><circle cx="34" cy="29" r="2.2" class="fl dimf"/><path d="M19 32L49 8" class="st errc thin"/></g>' +
    '<g class="f f-sleep"><path d="M14 19q7 8 14 0M40 19q7 8 14 0" class="st dim"/></g>';

  var FX =
    '<g class="sparks"><path d="M17 4L20 13 29 16 20 19 17 28 14 19 5 16 14 13Z" class="fl sp1"/><path d="M84 3L86.6 9.4 93 12 86.6 14.6 84 21 81.4 14.6 75 12 81.4 9.4Z" class="fl sp2"/></g>';

  function html(fam, state, size) {
    fam = META[fam] ? fam : "nexora";
    state = norm(state);
    var m = META[fam], S = size || 64;
    var w = m.w >= m.h ? S : Math.round(S * m.w / m.h), h = m.w >= m.h ? Math.round(S * m.h / m.w) : S;
    var pc = function (v, t) { return (v / t * 100).toFixed(2) + "%"; };
    var delay = "-" + (Math.random() * 4).toFixed(1) + "s";
    return '<div class="nxc" data-state="' + state + '" data-family="' + fam + '" style="width:' + w + "px;height:" + h + "px;--b:" + m.glow + ";--dark:" + m.dark +
      '" role="img" aria-label="' + NAME[fam] + ": " + TITLE[state] + '"><div class="nxc-in" style="animation-delay:' + delay + '">' +
      '<img class="base" src="chars/' + fam + '-base.png" alt="" draggable="false">' +
      '<img class="glyph" src="chars/' + fam + '-glyph.png" alt="" draggable="false" style="left:' + pc(m.vx, m.w) + ";top:" + pc(m.vy, m.h) + ";width:" + pc(m.vw, m.w) + ";height:" + pc(m.vh, m.h) + ';animation-delay:' + delay + '">' +
      '<svg class="face" viewBox="0 0 68 40" preserveAspectRatio="xMidYMid meet" style="left:' + pc(m.fx, m.w) + ";top:" + pc(m.fy, m.h) + ";width:" + pc(m.fw, m.w) + ";height:" + pc(m.fh, m.h) + '">' + FACE + "</svg>" +
      '<div class="glow" style="left:' + pc(m.vx, m.w) + ";top:" + pc(m.vy, m.h) + ";width:" + pc(m.vw, m.w) + ";height:" + pc(m.vh, m.h) + '"></div>' +
      '<svg class="fx" viewBox="0 0 100 100" preserveAspectRatio="none">' + FX + "</svg></div></div>";
  }
  function set(el, state) {
    var c = el && (el.classList && el.classList.contains("nxc") ? el : el.querySelector(".nxc"));
    if (!c) return;
    state = norm(state);
    if (c.dataset.state === state) return;
    c.dataset.state = state;
    c.setAttribute("aria-label", NAME[c.dataset.family] + ": " + TITLE[state]);
  }
  root.NxChar = { html: html, set: set, family: family, STATES: STATES, TITLE: TITLE, NAME: NAME, META: META };
})(window);
