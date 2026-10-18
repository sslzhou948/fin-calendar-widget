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
 *            font_size(12-18), frosted(bool), notifications(bool), source('auto'|'primary'|'backup'),
 *            usd_mode(bool), notify_events(bool), hide_holidays(bool),
 *            panel_width_pct(25-75), colors:{time,stars,beat,miss,inline,text,muted} }
 */
(function () {
  'use strict';

  var DEFAULT_CONFIG = {
    countries: ['CN', 'US'],
    min_stars: 3,
    side: 'right',
    font_size: 14,
    frosted: true,
    notifications: true,
    source: 'auto',
    usd_mode: false,       // 美元口径：按 usd_sentiment(bull=红/bear=绿) 给实际值着色
    notify_events: false,   // 事件类（讲话/纪要）也发系统通知
    hide_holidays: false,   // 隐藏假期行
    panel_width_pct: 42,    // 面板宽度占屏幕宽度的百分比 25–75（Rust 端调窗口）
    colors: {               // 颜色主题（与 styles.css :root / Rust 默认值保持一致）
      time: '#f0a832', stars: '#ffd166',
      beat: '#4ade80', miss: '#f87171', inline: '#9ca3af',
      text: '#eceff4', muted: '#9aa4b5'
    }
  };

  var MIN_FS = 9;   /* 方案A floor：再小中文即不可读 */
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
    var root = document.documentElement;
    root.style.setProperty('--fs', fs + 'px');
    document.body.classList.toggle('no-frosted', !config.frosted);
    document.body.classList.toggle('side-left', config.side === 'left');
    /* 颜色主题：配置缺失时回退默认值 */
    var dc = DEFAULT_CONFIG.colors;
    var cc = Object.assign({}, dc, config.colors || {});
    root.style.setProperty('--amber', cc.time);
    root.style.setProperty('--gold', cc.stars);
    root.style.setProperty('--c-beat', cc.beat);
    root.style.setProperty('--c-miss', cc.miss);
    root.style.setProperty('--c-inline', cc.inline);
    root.style.setProperty('--text', cc.text);
    root.style.setProperty('--muted', cc.muted);
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
    var suffix = idx === 0 ? ' · 今天' : (idx === 1 ? ' · 明天' : '');
    return '星期' + WEEKDAYS[d.getDay()] + ' ' + md + suffix;
  }

  /* 每日格言：按年积日轮换，零维护 */
  var QUOTES = [
    '以时间换空间，守拙出奇。',
    '截断亏损，让利润奔跑。',
    '计划你的交易，交易你的计划。',
    '市场永远是对的，错的总是自己。',
    '耐心是交易者最稀缺的筹码。',
    '先求不败，再求胜。',
    '风控第一，盈利自来。'
  ];
  function dayQuote() {
    var now = new Date();
    var start = new Date(now.getFullYear(), 0, 0);
    var doy = Math.floor((now - start) / 86400000);
    return QUOTES[doy % QUOTES.length];
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

  /* data 行：v4 紧凑两段式 —— 时间+国旗+全名+星级 / 预期·前值·实际
   * （原型的三段式在忙周放不下一周，合并首行以满足方案A"一周全览"） */
  function renderDataRow(e, near) {
    return '<div class="ev' + (near ? ' near' : '') + '">' +
      '<div class="ev-line1"><span class="time">' + timeLabel(e) + '</span>' +
        flagHtml(e) + titleHtml(e) + stars(e.importance) + '</div>' +
      valsHtml(e) +
    '</div>';
  }

  /* 预期 / 前值 / 实际单行；实际值按中性口径（或美元口径）着色 */
  function valsHtml(e) {
    function v(x) { return (x == null || x === '') ? '—' : escapeHtml(x); }
    var ac = actualColorCls(e);
    return '<div class="vals">' +
      '<span class="k">预期</span><b>' + v(e.forecast) + '</b>' +
      '<span class="sep">·</span>' +
      '<span class="k">前值</span><b>' + v(e.previous) + '</b>' +
      '<span class="sep">·</span>' +
      '<span class="k">实际</span><b' + (ac ? ' class="' + ac + '"' : '') + '>' + v(e.actual) + '</b>' +
    '</div>';
  }

  function renderEventRow(e, near) {
    var badge = (e.status != null && e.status !== '')
      ? '<span class="status">' + escapeHtml(e.status) + '</span>' : '';
    return '<div class="ev ev-event' + (near ? ' near' : '') + '">' +
      '<div class="ev-line1"><span class="time">' + timeLabel(e) + '</span>' +
        flagHtml(e) + titleHtml(e) + stars(e.importance) + badge + '</div>' +
    '</div>';
  }

  function renderHolidayRow(e) {
    return '<div class="ev ev-holiday">' +
      '<div class="ev-line1">' + flagHtml(e, false) + titleHtml(e) + '</div>' +
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
    document.getElementById('p-quote').textContent = '每日格言：' + dayQuote();
    fitFont();
    /* 触底仍溢出：页脚如实提示（极端忙周） */
    var note = document.querySelector('.p-foot .p-note');
    if (note) {
      note.textContent = '事件驱动刷新 · 每周日 00:00(北京时间)全量更新 · 开机自动对账' +
        (document.getElementById('groups').dataset.overflow ? '（本周事件较多，已压缩至最小字号）' : '');
    }
  }

  /* 字号自适应（方案A）：#groups 内容溢出则逐级缩小 --fs（步长 1px，最小 10px）直到放下。
   * 注意：必须量 #groups（flex:1 的裁剪容器）而非 #panel——panel 自身被 max-height
   * 封顶，scrollHeight 恒等于 clientHeight，量它永远测不出溢出。 */
  function fitFont() {
    var groups = document.getElementById('groups');
    var fs = clampInt(config.font_size, 12, 18, 14);
    var root = document.documentElement;
    root.style.setProperty('--fs', fs + 'px');
    var guard = 0;
    while (groups.scrollHeight > groups.clientHeight && fs > MIN_FS && guard < 24) {
      fs -= 1;
      root.style.setProperty('--fs', fs + 'px');
      guard++;
    }
    /* 触底仍溢出：如实记录（调用方可据此提示"还有 N 条未显示"） */
    groups.dataset.overflow = groups.scrollHeight > groups.clientHeight ? '1' : '';
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
