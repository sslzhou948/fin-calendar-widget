/* 桌面财经日历挂件 — 主面板渲染逻辑
 *
 * 数据接入（两种模式）：
 *  1) Tauri 内：window.__TAURI__ 存在时调 invoke('get_state') 取全量
 *     { events: [...], config: {...} }，并监听 'calendar-updated' 事件增量刷新。
 *  2) 普通浏览器：自动 fetch 同目录 sample-data.json 渲染（截图/预览用）。
 *
 * 后端契约（供 src-tauri 实现参考）：
 *  - command get_state() -> { events: CalendarEvent[], config: Config }
 *  - event   calendar-updated（无 payload，前端收到后重新 get_state）
 *  CalendarEvent: { id, release_at(ISO, 北京时间), time('HH:mm' 备用),
 *                   country('CN'|'US'|...), kind('data'|'event'|'holiday'),
 *                   title, importance(1-5),
 *                   forecast|null, previous|null, actual|null,
 *                   deviation('beat'|'miss'|'inline'|null),
 *                   status(string|null),
 *                   usd_sentiment('bull'|'bear'|'neutral'|null), source }
 *    - kind 缺失时按 'data' 处理（向后兼容旧数据）。
 *    - usd_sentiment 仅在 config.usd_mode=true（美元口径着色）时由前端使用。
 *    - 所有字段缺失时前端必须容忍，不得崩溃。
 *  Config: { countries:[..], min_stars, side('left'|'right'),
 *            font_size(12-18), frosted(bool), notify(bool), source('auto'|'primary'|'backup'),
 *            usd_mode(bool), notify_events(bool), hide_holidays(bool) }
 */
(function () {
  'use strict';

  var DEFAULT_CONFIG = {
    countries: ['CN', 'US'],
    min_stars: 3,
    side: 'right',
    font_size: 14,
    frosted: true,
    notify: true,
    source: 'auto',
    usd_mode: false,       // 美元口径：按 usd_sentiment(bull=红/bear=绿) 给实际值着色
    notify_events: false,   // 事件类（讲话/纪要）也发系统通知
    hide_holidays: false    // 隐藏假期行
  };

  var MIN_FS = 10;
  var NEAR_MS = 6 * 3600 * 1000;   // 距公布 <6 小时高亮
  var WEEKDAYS = ['日', '一', '二', '三', '四', '五', '六'];

  var config = Object.assign({}, DEFAULT_CONFIG);
  var events = [];

  var T = (typeof window !== 'undefined') ? window.__TAURI__ : undefined;

  function isTauri() {
    return !!(T && T.core && typeof T.core.invoke === 'function');
  }

  /* ---------- 配置 ---------- */

  function applyConfigToDom() {
    var fs = clampInt(config.font_size, 12, 18, 14);
    config.font_size = fs;
    document.documentElement.style.setProperty('--fs', fs + 'px');
    document.body.classList.toggle('no-frosted', !config.frosted);
    document.body.classList.toggle('side-left', config.side === 'left');
  }

  function mergeConfig(c) {
    if (!c) return;
    config = Object.assign({}, DEFAULT_CONFIG, c);
    applyConfigToDom();
  }

  function loadLocalConfig() {
    try {
      var raw = localStorage.getItem('fin-cal-config');
      return raw ? JSON.parse(raw) : null;
    } catch (e) { return null; }
  }

  /* ---------- 数据加载 ---------- */

  function applyState(state) {
    if (!state) return;
    events = Array.isArray(state.events) ? state.events : (Array.isArray(state) ? state : []);
    mergeConfig(state.config);
    render();
  }

  async function loadFromTauri() {
    try {
      var state = await T.core.invoke('get_state');
      applyState(state);
    } catch (e) {
      showError('读取后端状态失败：' + String(e && e.message || e));
    }
    try {
      if (T.event && typeof T.event.listen === 'function') {
        await T.event.listen('calendar-updated', async function () {
          try {
            var s = await T.core.invoke('get_state');
            applyState(s);
          } catch (e) { /* 下次事件再试 */ }
        });
      }
    } catch (e) { /* 监听失败不影响首屏 */ }
  }

  async function loadFromSample() {
    mergeConfig(loadLocalConfig());
    try {
      var res = await fetch('sample-data.json');
      if (!res.ok) throw new Error('HTTP ' + res.status);
      var data = await res.json();
      events = Array.isArray(data.events) ? data.events : [];
      render();
    } catch (e) {
      showError('加载 sample-data.json 失败：' + String(e && e.message || e) +
        '（file:// 下直接打开需给 Chromium 加 --allow-file-access-from-files）');
    }
  }

  function showError(msg) {
    var el = document.getElementById('p-err');
    el.textContent = msg;
    el.hidden = false;
  }

  /* ---------- 渲染 ---------- */

  function escapeHtml(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function clampInt(v, lo, hi, dflt) {
    v = parseInt(v, 10);
    if (isNaN(v)) return dflt;
    return Math.min(hi, Math.max(lo, v));
  }

  /* 国旗目录：截图脚本可经 window.__FLAG_DIR__ 指定 file:// 绝对路径 */
  function flagDir() {
    try {
      if (typeof window !== 'undefined' && window.__FLAG_DIR__) {
        return String(window.__FLAG_DIR__);
      }
    } catch (e) { /* 忽略 */ }
    return 'flags/';
  }

  /* 国旗 + 国家缩写；withCode=false 时只渲染旗（假期行用） */
  function flagHtml(e, withCode) {
    var cc = (e.country || '').toUpperCase();
    var esc = escapeHtml(cc);
    var img = '';
    if (cc) {
      img = '<img class="flag" alt="' + esc + '" src="' + flagDir() + cc.toLowerCase() + '.svg"' +
        ' onerror="this.style.display=\'none\';var fb=this.nextElementSibling;if(fb){fb.style.display=\'inline-flex\';}">';
    }
    var fb = '<span class="flag-fb" style="display:none">' + esc + '</span>';
    return img + fb + (withCode === false ? '' : '<span class="ccode">' + esc + '</span>');
  }

  function evTimeMs(e) {
    if (e.release_at) {
      var d = new Date(e.release_at);
      if (!isNaN(d)) return d.getTime();
    }
    return null;
  }

  function dateKey(iso) {
    var d = new Date(iso);
    return d.getFullYear() + '-' +
      String(d.getMonth() + 1).padStart(2, '0') + '-' +
      String(d.getDate()).padStart(2, '0');
  }

  function dayLabel(key, idx) {
    if (key === 'undated') return '待公布时间';
    var parts = key.split('-').map(Number);
    var d = new Date(parts[0], parts[1] - 1, parts[2]);
    var md = String(parts[1]).padStart(2, '0') + '-' + String(parts[2]).padStart(2, '0');
    var prefix = idx === 0 ? '今天 · ' : (idx === 1 ? '明天 · ' : '');
    return prefix + md + ' 星期' + WEEKDAYS[d.getDay()];
  }

  function timeHM(iso) {
    var d = new Date(iso);
    return String(d.getHours()).padStart(2, '0') + ':' +
           String(d.getMinutes()).padStart(2, '0');
  }

  function timeLabel(e) {
    var t = evTimeMs(e);
    if (t != null) return timeHM(e.release_at);
    return e.time || '--:--';
  }

  function stars(n) {
    n = clampInt(n, 0, 5, 0);
    return '<span class="stars">' + '★'.repeat(n) +
      '<span class="off">' + '☆'.repeat(5 - n) + '</span></span>';
  }

  function valCell(label, value, isActual, extraCls) {
    var v = (value == null || value === '') ? '<span class="v na">—</span>'
      : '<span class="v' + (extraCls ? ' ' + extraCls : '') + '">' + escapeHtml(value) + '</span>';
    return '<div class="val' + (isActual ? ' actual' : '') + '">' +
             '<span class="k">' + label + '</span>' + v + '</div>';
  }

  function titleHtml(e) {
    return '<span class="title" title="' + escapeHtml(e.title) + '">' +
      escapeHtml(e.title) + '</span>';
  }

  /* data 行实际值着色类：中性口径（deviation）或美元口径（usd_sentiment） */
  function actualColorCls(e) {
    if (config.usd_mode && (e.usd_sentiment === 'bull' || e.usd_sentiment === 'bear')) {
      return e.usd_sentiment;              // 沿用旧 .bull / .bear 类
    }
    if (config.usd_mode && e.usd_sentiment != null) return '';   // neutral 不着色
    if (e.deviation === 'beat') return 'dev-beat';
    if (e.deviation === 'miss') return 'dev-miss';
    if (e.deviation === 'inline') return 'dev-inline';
    return '';
  }

  function renderDataRow(e, near) {
    return '<div class="ev' + (near ? ' near' : '') + '">' +
      '<span class="time">' + timeLabel(e) + '</span>' +
      flagHtml(e) +
      titleHtml(e) +
      stars(e.importance) +
      '<span class="vals">' +
        valCell('预期', e.forecast, false) +
        valCell('前值', e.previous, false) +
        valCell('实际', e.actual, true, actualColorCls(e)) +
      '</span>' +
    '</div>';
  }

  function renderEventRow(e, near) {
    var badge = (e.status != null && e.status !== '')
      ? '<span class="status">' + escapeHtml(e.status) + '</span>' : '';
    return '<div class="ev ev-event' + (near ? ' near' : '') + '">' +
      '<span class="time">' + timeLabel(e) + '</span>' +
      flagHtml(e) +
      titleHtml(e) +
      stars(e.importance) +
      badge +
    '</div>';
  }

  function renderHolidayRow(e) {
    return '<div class="ev ev-holiday">' +
      '<span class="time">' + timeLabel(e) + '</span>' +
      flagHtml(e, false) +
      titleHtml(e) +
    '</div>';
  }

  function render() {
    var list = events
      .filter(function (e) {
        if (!e) return false;
        var kind = e.kind || 'data';       // kind 缺失 → 按 data 处理
        if (kind === 'holiday' && config.hide_holidays) return false;
        if (config.countries.indexOf(e.country) === -1) return false;
        // 假期行不参与星级过滤（纯信息行）
        if (kind !== 'holiday' &&
            clampInt(e.importance, 1, 5, 1) < config.min_stars) return false;
        return true;
      })
      .sort(function (a, b) {
        var ta = evTimeMs(a), tb = evTimeMs(b);
        if (ta == null) return 1;
        if (tb == null) return -1;
        return ta - tb;
      });

    var groups = [];
    var map = {};
    list.forEach(function (e) {
      var t = evTimeMs(e);
      var k = t != null ? dateKey(e.release_at) : 'undated';
      if (!map[k]) { map[k] = []; groups.push(k); }
      map[k].push(e);
    });

    var now = Date.now();
    var html = groups.map(function (k, gi) {
      var rows = map[k].map(function (e) {
        var t = evTimeMs(e);
        var kind = e.kind || 'data';
        // 临近高亮：data/event 行保留，holiday 行不加
        var near = kind !== 'holiday' && t != null && (t - now > 0) && (t - now < NEAR_MS);
        if (kind === 'holiday') return renderHolidayRow(e);
        if (kind === 'event') return renderEventRow(e, near);
        return renderDataRow(e, near);
      }).join('');
      return '<section class="day-group">' +
               '<div class="day-head">' + escapeHtml(dayLabel(k, gi)) + '</div>' +
               rows +
             '</section>';
    }).join('');

    document.getElementById('groups').innerHTML = html;
    document.getElementById('empty').hidden = list.length > 0;
    fitFont();
  }

  /* 字号自适应：#panel 内容溢出则逐级缩小 --fs（步长 1px，最小 10px）直到放下 */
  function fitFont() {
    var panel = document.getElementById('panel');
    var fs = clampInt(config.font_size, 12, 18, 14);
    var root = document.documentElement;
    root.style.setProperty('--fs', fs + 'px');
    var guard = 0;
    while (panel.scrollHeight > panel.clientHeight && fs > MIN_FS && guard < 20) {
      fs -= 1;
      root.style.setProperty('--fs', fs + 'px');
      guard++;
    }
  }

  /* ---------- 启动 ---------- */

  function init() {
    window.addEventListener('resize', function () { fitFont(); });
    if (isTauri()) {
      loadFromTauri();
    } else {
      loadFromSample();
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
