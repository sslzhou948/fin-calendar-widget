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
    autostart: true,
    source: 'auto',
    usd_mode: false,
    notify_events: false,
    hide_holidays: false,
    width_mode: 'auto',
    manual_columns: 3,
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
    var wmEl = document.querySelector('input[name="widthMode"]:checked');
    var colors = {};
    COLOR_FIELDS.forEach(function (pair) {
      colors[pair[1]] = el(pair[0]).value;
    });
    return {
      countries: countries,
      min_stars: parseInt(el('minStars').value, 10),
      side: sideEl ? sideEl.value : 'right',
      font_size: parseInt(el('fontSize').value, 10),
      width_mode: wmEl ? wmEl.value : 'auto',
      manual_columns: parseInt(el('manualColumns').value, 10),
      frost_opacity: parseInt(el('frostOpacity').value, 10),
      frosted: el('frosted').checked,
      notifications: el('notify').checked,
      autostart: el('autostart').checked,
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
    var wmEl = document.querySelector('input[name="widthMode"][value="' + c.width_mode + '"]');
    if (wmEl) wmEl.checked = true;
    el('manualColumns').value = String(c.manual_columns);
    el('frostOpacity').value = String(c.frost_opacity);
    el('frostOpacityVal').textContent = c.frost_opacity + '%';
    el('frosted').checked = !!c.frosted;
    el('notify').checked = !!c.notifications;
    el('autostart').checked = c.autostart !== false;
    el('usdMode').checked = !!c.usd_mode;
    el('notifyEvents').checked = !!c.notify_events;
    el('hideHolidays').checked = !!c.hide_holidays;
    COLOR_FIELDS.forEach(function (pair) {
      el(pair[0]).value = c.colors[pair[1]];
    });
    /* 旧配置的 source="backup"（Investing 已删）一律显示为自动 */
    el('source').value = (c.source === 'primary') ? 'primary' : 'auto';
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
      return '<div class="t-card">' +
        '<div><strong>' + escapeHtml(r.name) + '</strong>　' +
        '<span class="' + cls + '">' + status + '</span></div>' +
        '<div>延迟：' + escapeHtml(String(r.latency_ms == null ? '—' : r.latency_ms + ' ms')) +
        '　解析行数：' + escapeHtml(String(r.rows == null ? '—' : r.rows)) + '</div>' +
        (r.error ? '<div class="fail">错误：' + escapeHtml(r.error) + '</div>' : '') +
        '</div>';
    }).join('');
    box.innerHTML = html;
  }

  function renderSimulatedTest() {
    el('testResult').innerHTML =
      '<div class="t-card">' +
      '<div><strong>主数据源（金十 WebSocket）</strong>　<span class="ok">✓ 连通（模拟）</span></div>' +
      '<div>延迟：218 ms　解析行数：42</div>' +
      '</div>' +
      '<div class="t-card">' +
      '<div><strong>备用数据源（Faireconomy JSON）</strong>　<span class="ok">✓ 连通（模拟）</span></div>' +
      '<div>延迟：305 ms　解析行数：38</div>' +
      '</div>' +
      '<div class="note">浏览器预览模式：以上为模拟说明文字，非真实测试结果。' +
      '真实的接口测试需在 Tauri 应用内运行（调用后端 test_sources）。</div>';
  }

  /* 测试按钮防抖：点击后禁用 60 秒，防止连点触发 429（2026-10-19） */
  function cooldown(btn, origText, secs) {
    btn.disabled = true;
    var left = secs;
    btn.textContent = origText + '（' + left + 's）';
    var timer = setInterval(function () {
      left--;
      if (left <= 0) {
        clearInterval(timer);
        btn.disabled = false;
        btn.textContent = origText;
      } else {
        btn.textContent = origText + '（' + left + 's）';
      }
    }, 1000);
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
      cooldown(btn, '接口测试', 60);
    }
  }

  /* ---------- 实际值回填测试 ---------- */
  async function runActualsTest() {
    var btn = el('testActualsBtn');
    var box = el('actualsResult');
    btn.disabled = true;
    btn.textContent = '测试中…';
    box.innerHTML = '';
    try {
      if (!isTauri()) {
        box.innerHTML = '<div class="t-card"><span class="fail">浏览器预览模式不支持，需在 Tauri 应用内运行。</span></div>';
        return;
      }
      var r = await T.core.invoke('test_actuals');
      var okRate = r.published > 0 && r.with_actual > 0;
      var html = '<div class="t-card">' +
        '<div><strong>实际值覆盖测试</strong>　' +
        (okRate ? '<span class="ok">✓ 正常</span>' : '<span class="fail">✗ 无实际值</span>') + '</div>' +
        '<div>命中数据源：' + escapeHtml(String(r.source)) + '　' +
        '本周事件：' + r.total + ' 条　' +
        '已公布数据行：' + r.published + ' 条　' +
        '带实际值：' + r.with_actual + ' 条</div>';
      if (r.samples && r.samples.length) {
        html += '<div style="margin-top:8px;font-size:12px;color:var(--muted)">样本：</div>';
        r.samples.forEach(function (s) {
          html += '<div style="font-size:12px;margin-top:4px">' +
            escapeHtml(s.title) + ' (' + escapeHtml(s.country) + ')<br>' +
            '预期 ' + escapeHtml(String(s.forecast || '—')) +
            '　前值 ' + escapeHtml(String(s.previous || '—')) +
            '　<span class="ok">实际 ' + escapeHtml(String(s.actual || '—')) + '</span></div>';
        });
      } else {
        html += '<div class="note">暂无已公布数据行（本周数据均未到发布时间）。</div>';
      }
      html += '</div>';
      box.innerHTML = html;
    } catch (e) {
      box.innerHTML =
        '<div class="t-card"><span class="fail">测试调用失败：' +
        escapeHtml(String(e && e.message || e)) + '</span></div>';
    } finally {
      cooldown(btn, '实际值回填测试', 60);
    }
  }

  /* ---------- 启动 ---------- */

  function init() {
    buildCountryChecks();
    /* 竞态护栏（QA G2 Major）：配置加载完成前锁定表单——否则用户抢先
       编辑触发的预览会把 HTML 静态默认当配置应用到主窗，或在回调
       返回时被 fillForm 冲掉正在改的内容。回滚不再归 JS 管（清单
       #13）：关窗后 Rust 对比内存/落盘配置自动恢复未应用的预览。 */
    var wrapEl = document.querySelector('.s-wrap');
    if (wrapEl) { wrapEl.style.pointerEvents = 'none'; wrapEl.style.opacity = '0.55'; }
    loadConfig().then(function (cfg) {
      var shown = cfg || DEFAULT_CONFIG;
      fillForm(shown);
      if (wrapEl) { wrapEl.style.pointerEvents = ''; wrapEl.style.opacity = ''; }
    });

    el('fontSize').addEventListener('input', function () {
      el('fontSizeVal').textContent = el('fontSize').value + ' px';
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
    el('testActualsBtn').addEventListener('click', runActualsTest);

    /* ---------- 桌面挂靠诊断（#17） ---------- */
    async function runPinDiag() {
      var btn = el('pinDiagBtn');
      var box = el('pinDiagResult');
      btn.disabled = true;
      btn.textContent = '诊断中…';
      try {
        if (!isTauri()) {
          box.innerHTML = '<div class="t-card"><span class="fail">浏览器预览模式不支持，需在 Tauri 应用内运行。</span></div>';
          return;
        }
        var d = await T.core.invoke('get_pin_diag');
        var parentLine = d.current_parent
          ? '父窗口 ' + d.current_parent + '（类名：' + (d.current_parent_class || '未知') + '）'
          : '无父窗口（顶层窗口，未挂靠）';
        var cacheLine = d.cached_workerw
          ? '缓存 WorkerW ' + d.cached_workerw + '（' + (d.cached_alive ? '存活' : '已失效') + '）'
          : '无缓存 WorkerW';
        var events = (d.events || []).slice(-12).map(function (e) {
          return escapeHtml('[' + e.time + '] ' + e.kind + '：' + e.detail);
        }).join('<br>');
        box.innerHTML = '<div class="t-card">' +
          '<div><strong>桌面挂靠诊断</strong>　' +
          (d.supported ? '' : '<span class="fail">✗ 非 Windows，挂靠不适用</span>') + '</div>' +
          '<div>主窗口：' + escapeHtml(String(d.window || '—')) + '</div>' +
          '<div>当前状态：' + escapeHtml(parentLine) + '</div>' +
          '<div>' + escapeHtml(cacheLine) + '</div>' +
          '<div style="margin-top:8px;font-size:12px;color:var(--muted)">最近步骤（新→旧只显示最后 12 条）：</div>' +
          '<div style="font-size:12px;margin-top:4px">' + events + '</div>' +
          '<div class="note">日志文件：' + escapeHtml(String(d.log_path || '—')) + '</div>' +
          '</div>';
      } catch (e) {
        box.innerHTML =
          '<div class="t-card"><span class="fail">诊断调用失败：' +
          escapeHtml(String(e && e.message || e)) + '</span></div>';
      } finally {
        btn.disabled = false;
        btn.textContent = '桌面挂靠诊断';
      }
    }
    el('pinDiagBtn').addEventListener('click', runPinDiag);

    /* ---------- 预览 / 应用 / 取消（清单 #7） ----------
     * 任何表单改动防抖后走 preview_config：仅内存生效、主窗口即时
     * 重渲染、不落盘；「应用」才 set_config 落盘并更新快照；
     * 「取消」/关窗（X）以快照回滚预览后关闭。
     * 浏览器模式无后端：预览无外部效果，「应用」写 localStorage，
     * 「取消」恢复表单即可。（旧 liveSave 自动落盘逻辑已删除。） */
    var previewTimer = null;
    function schedulePreview() {
      if (previewTimer) clearTimeout(previewTimer);
      previewTimer = setTimeout(async function () {
        if (!isTauri()) return;
        try {
          await T.core.invoke('preview_config', { config: readForm() });
        } catch (e) { /* 预览失败不打扰，下次改动再试 */ }
      }, 250);
    }
    if (wrapEl) {
      wrapEl.addEventListener('input', schedulePreview);
      wrapEl.addEventListener('change', schedulePreview);
    }

    async function closeSettings() {
      if (isTauri() && T.window && T.window.getCurrentWindow) {
        try { await T.window.getCurrentWindow().close(); return; } catch (e) { /* 落到 window.close */ }
      }
      window.close();
    }

    el('applyBtn').addEventListener('click', async function () {
      var cfg = readForm();
      try {
        await saveConfig(cfg);
        var tip = el('saveTip');
        tip.hidden = false;
        setTimeout(function () { tip.hidden = true; }, 2000);
      } catch (e) {
        alert('应用失败：' + String(e && e.message || e));
      }
    });

    /* 取消 = 直接关窗（清单 #13）：未应用的预览由 Rust 在窗口销毁
       事件里回滚，JS 不拦截关闭——旧拦截流程在 WebView2 上会把
       窗口拆成黑框僵尸（真机实证）。X 按钮同理走原生关闭。 */
    el('cancelBtn').addEventListener('click', function () {
      closeSettings();
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
