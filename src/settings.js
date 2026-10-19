/* 财经日历 — 设置页逻辑
 *
 * Tauri 内：配置经 invoke('get_state').config 读取、invoke('set_config', {config})
 *   保存；「接口测试」调 invoke('test_sources')，期望返回
 *   [ { name, ok, latency_ms, rows, error } ] 并渲染。
 * 浏览器预览：配置读写 localStorage('fin-cal-config')；接口测试显示模拟说明。
 */
(function () {
  'use strict';

  var COUNTRIES = [
    ['CN', '中国'], ['US', '美国'], ['EU', '欧元区'], ['UK', '英国'],
    ['JP', '日本'], ['CA', '加拿大'], ['AU', '澳大利亚'],
    ['CH', '瑞士'], ['NZ', '新西兰']
  ];

  var DEFAULT_CONFIG = {
    countries: ['CN', 'US'],
    min_stars: 3,
    side: 'right',
    font_size: 14,
    frosted: true,
    notifications: true,
    source: 'auto',
    usd_mode: false,
    notify_events: false,
    hide_holidays: false,
    panel_width_pct: 42,
    frost_opacity: 70,
    colors: {
      time: '#f0a832', stars: '#ffd166',
      beat: '#4ade80', miss: '#f87171', inline: '#9ca3af',
      text: '#eceff4', muted: '#9aa4b5'
    }
  };

  /* 颜色字段 id → config.colors 键 */
  var COLOR_FIELDS = [
    ['cTime', 'time'], ['cStars', 'stars'],
    ['cBeat', 'beat'], ['cMiss', 'miss'], ['cInline', 'inline'],
    ['cText', 'text'], ['cMuted', 'muted']
  ];

  var T = (typeof window !== 'undefined') ? window.__TAURI__ : undefined;

  function isTauri() {
    return !!(T && T.core && typeof T.core.invoke === 'function');
  }

  function el(id) { return document.getElementById(id); }

  function escapeHtml(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  /* ---------- 表单构建 ---------- */

  function buildCountryChecks() {
    var box = el('countries');
    box.innerHTML = '';
    COUNTRIES.forEach(function (pair) {
      var label = document.createElement('label');
      label.className = 'item';
      var input = document.createElement('input');
      input.type = 'checkbox';
      input.name = 'country';
      input.value = pair[0];
      input.checked = DEFAULT_CONFIG.countries.indexOf(pair[0]) !== -1;
      label.appendChild(input);
      label.appendChild(document.createTextNode(' ' + pair[1]));
      box.appendChild(label);
    });
  }

  function readForm() {
    var countries = Array.prototype.slice.call(
      document.querySelectorAll('input[name="country"]:checked')
    ).map(function (i) { return i.value; });
    var sideEl = document.querySelector('input[name="side"]:checked');
    var colors = {};
    COLOR_FIELDS.forEach(function (pair) {
      colors[pair[1]] = el(pair[0]).value;
    });
    return {
      countries: countries,
      min_stars: parseInt(el('minStars').value, 10),
      side: sideEl ? sideEl.value : 'right',
      font_size: parseInt(el('fontSize').value, 10),
      panel_width_pct: parseInt(el('panelWidth').value, 10),
      frost_opacity: parseInt(el('frostOpacity').value, 10),
      frosted: el('frosted').checked,
      notifications: el('notify').checked,
      usd_mode: el('usdMode').checked,
      notify_events: el('notifyEvents').checked,
      hide_holidays: el('hideHolidays').checked,
      colors: colors,
      source: el('source').value
    };
  }

  function fillForm(cfg) {
    var c = Object.assign({}, DEFAULT_CONFIG, cfg);
    c.colors = Object.assign({}, DEFAULT_CONFIG.colors, cfg.colors || {});
    Array.prototype.forEach.call(
      document.querySelectorAll('input[name="country"]'),
      function (i) { i.checked = c.countries.indexOf(i.value) !== -1; }
    );
    el('minStars').value = String(c.min_stars);
    var sideEl = document.querySelector('input[name="side"][value="' + c.side + '"]');
    if (sideEl) sideEl.checked = true;
    el('fontSize').value = String(c.font_size);
    el('fontSizeVal').textContent = c.font_size + ' px';
    el('panelWidth').value = String(c.panel_width_pct);
    el('panelWidthVal').textContent = c.panel_width_pct + '%';
    el('frostOpacity').value = String(c.frost_opacity);
    el('frostOpacityVal').textContent = c.frost_opacity + '%';
    el('frosted').checked = !!c.frosted;
    el('notify').checked = !!c.notifications;
    el('usdMode').checked = !!c.usd_mode;
    el('notifyEvents').checked = !!c.notify_events;
    el('hideHolidays').checked = !!c.hide_holidays;
    COLOR_FIELDS.forEach(function (pair) {
      el(pair[0]).value = c.colors[pair[1]];
    });
    el('source').value = c.source;
  }

  /* ---------- 配置读写 ---------- */

  async function loadConfig() {
    if (isTauri()) {
      try {
        var state = await T.core.invoke('get_state');
        return (state && state.config) || null;
      } catch (e) { return null; }
    }
    try {
      var raw = localStorage.getItem('fin-cal-config');
      return raw ? JSON.parse(raw) : null;
    } catch (e) { return null; }
  }

  async function saveConfig(cfg) {
    if (isTauri()) {
      await T.core.invoke('set_config', { config: cfg });
    } else {
      localStorage.setItem('fin-cal-config', JSON.stringify(cfg));
    }
  }

  /* ---------- 接口测试 ---------- */

  function renderTestResults(results) {
    var box = el('testResult');
    var html = results.map(function (r) {
      var cls = r.ok ? 'ok' : 'fail';
      var status = r.ok ? '✓ 连通' : '✗ 失败';
      /* 备源 Investing 已知被 Cloudflare 拦截（2026-10-19 确认），失败属预期，标注清楚免误会 */
      var note = '';
      if (!r.ok && /investing/i.test(r.name)) {
        note = '<div class="note">备源：已知被 Cloudflare 按机器人拦截（HTTP 403），属预期失败，不影响主源使用。</div>';
      }
      return '<div class="t-card">' +
        '<div><strong>' + escapeHtml(r.name) + '</strong>　' +
        '<span class="' + cls + '">' + status + '</span></div>' +
        '<div>延迟：' + escapeHtml(String(r.latency_ms == null ? '—' : r.latency_ms + ' ms')) +
        '　解析行数：' + escapeHtml(String(r.rows == null ? '—' : r.rows)) + '</div>' +
        (r.error ? '<div class="fail">错误：' + escapeHtml(r.error) + '</div>' : '') +
        note +
        '</div>';
    }).join('');
    box.innerHTML = html;
  }

  function renderSimulatedTest() {
    el('testResult').innerHTML =
      '<div class="t-card">' +
      '<div><strong>主数据源（Investing 小工具）</strong>　<span class="ok">✓ 连通（模拟）</span></div>' +
      '<div>延迟：218 ms　解析行数：42</div>' +
      '</div>' +
      '<div class="t-card">' +
      '<div><strong>备用数据源（JSON 接口）</strong>　<span class="ok">✓ 连通（模拟）</span></div>' +
      '<div>延迟：305 ms　解析行数：38</div>' +
      '</div>' +
      '<div class="note">浏览器预览模式：以上为模拟说明文字，非真实测试结果。' +
      '真实的接口测试需在 Tauri 应用内运行（调用后端 test_sources）。</div>';
  }

  async function runTest() {
    var btn = el('testBtn');
    btn.disabled = true;
    btn.textContent = '测试中…';
    try {
      if (isTauri()) {
        var results = await T.core.invoke('test_sources');
        renderTestResults(Array.isArray(results) ? results : []);
      } else {
        renderSimulatedTest();
      }
    } catch (e) {
      el('testResult').innerHTML =
        '<div class="t-card"><span class="fail">测试调用失败：' +
        escapeHtml(String(e && e.message || e)) + '</span></div>';
    } finally {
      btn.disabled = false;
      btn.textContent = '接口测试';
    }
  }

  /* ---------- 启动 ---------- */

  function init() {
    buildCountryChecks();
    loadConfig().then(function (cfg) { fillForm(cfg || DEFAULT_CONFIG); });

    el('fontSize').addEventListener('input', function () {
      el('fontSizeVal').textContent = el('fontSize').value + ' px';
    });

    el('panelWidth').addEventListener('input', function () {
      el('panelWidthVal').textContent = el('panelWidth').value + '%';
    });

    el('frostOpacity').addEventListener('input', function () {
      el('frostOpacityVal').textContent = el('frostOpacity').value + '%';
    });

    el('resetColorsBtn').addEventListener('click', function () {
      COLOR_FIELDS.forEach(function (pair) {
        el(pair[0]).value = DEFAULT_CONFIG.colors[pair[1]];
      });
    });

    el('testBtn').addEventListener('click', runTest);

    /* 样式类控件：防抖自动保存，主窗口经 calendar-updated 即时重渲染 */
    var liveTimer = null;
    function liveSave() {
      if (liveTimer) clearTimeout(liveTimer);
      liveTimer = setTimeout(async function () {
        try { await saveConfig(readForm()); } catch (e) { /* 下次再试 */ }
      }, 350);
    }
    ['fontSize', 'panelWidth', 'frostOpacity', 'frosted'].forEach(function (id) {
      var node = el(id);
      if (node) node.addEventListener('input', liveSave);
    });
    /* 颜色取色器同样实时生效 */
    document.querySelectorAll('input[type="color"][data-color-key]').forEach(function (node) {
      node.addEventListener('input', liveSave);
    });

    el('saveBtn').addEventListener('click', async function () {
      var cfg = readForm();
      try {
        await saveConfig(cfg);
        var tip = el('saveTip');
        tip.hidden = false;
        setTimeout(function () { tip.hidden = true; }, 2000);
      } catch (e) {
        alert('保存失败：' + String(e && e.message || e));
      }
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
