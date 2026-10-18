# 设置页验收测试（TEST-SETTINGS）

> 范围：`src/settings.html` + `src/settings.js`（设置页）与 `src/app.js`（主面板消费配置）。
> 后端（`src-tauri`）行为不在本次无头测试范围内，只做字段契约核对。
> 测试方式：headless Chromium 加载本地页面，localStorage 模拟持久化，
> `src/sample-data.json`（18 条：13 data / 3 event / 2 holiday）为唯一数据源。
> 证据截图：`docs/test-evidence/`。

## 验收标准（11 项）

### S1 国家/地区多选
- 前置：全新浏览器上下文（无 localStorage）。
- 操作：打开设置页。
- 预期：CN、US 勾选，其余（EU/UK/JP/CA/AU/CH/NZ）不勾选；与 `DEFAULT_CONFIG.countries=['CN','US']` 一致。
- 主面板：`countries=['CN']` 时渲染行中无 US/EU/JP（`.ccode` 文本断言）；默认配置下 EU/JP 行被过滤。

### S2 星级阈值
- 前置：同上。
- 操作：打开设置页。
- 预期：`#minStars` 值为 `3`。
- 主面板：`min_stars=5` 时所有渲染行 importance=5；`min_stars=1` 时 2 星行出现；假期行不受阈值影响（始终显示，除非 S9 开启）。

### S3 半屏位置
- 预期：默认 `right` 单选框选中。
- 主面板：`side='left'` 时 `body.side-left` 类存在；`right` 时不存在。持久化往返一致。

### S4 字号
- 预期：默认 14，`min=12 max=18 step=1`；拖动滑块时旁边的 `14 px` 标签实时更新。
- 主面板：`font_size=18` 且内容未溢出时，`documentElement` 内联 `--fs` 为 `18px`；溢出时 `fitFont()` 允许逐级缩小（设计行为，需记录实际值）。

### S5 毛玻璃面板
- 预期：默认勾选。
- 主面板：`frosted=false` 时 `body.no-frosted` 存在，且 `#panel` 计算样式 `backdrop-filter: none`；`true` 时为 `blur(18px)...`。

### S6 数据公布系统通知
- 预期：默认勾选；修改→保存→刷新后回填一致（localStorage 键 `notify`）。
- ⚠️ 契约问题：前端键名为 `notify`，Rust `Config` 字段为 `notifications`（serde 默认忽略未知字段）——Tauri 内保存后后端恒为默认值 `true`，用户关闭通知**无法生效**。已记录为后端契约 bug，本次不改。

### S7 美元口径利多利空着色
- 预期：默认不勾选。
- 主面板：`usd_mode=true` 时，有 `usd_sentiment` 的行按 bull/bear 着色（bull 红 / bear 绿），且与中性口径颜色不同（例：纽约联储制造业指数 dev=beat 中性绿 → usd bull 红；成屋销售 dev=miss 中性红 → usd bear 绿）；`usd_sentiment=null` 的行不受影响。

### S8 事件类通知
- 预期：默认不勾选；持久化往返一致（行为在后端，前端仅持久化）。

### S9 隐藏假期行
- 预期：默认不勾选。
- 主面板：`hide_holidays=true` 时 `.ev-holiday` 数量为 0；默认 `false` 时为 2（国庆假期、银行假日）。

### S10 数据源
- 预期：默认 `auto`；三选项（auto/primary/backup）修改→保存→刷新后回填一致（行为在后端）。

### S11 接口测试按钮
- 预期（浏览器预览模式）：点击后按钮短暂显示"测试中…"并恢复；`#testResult` 渲染 **2 张** `.t-card` 模拟卡片（含"✓ 连通（模拟）"、延迟、解析行数）+ 1 段"浏览器预览模式…模拟说明文字"注释。

## 通用往返测试
对 S1/S2/S3/S5/S6/S7/S8/S9/S10：逐项改为非默认值 → 点保存 → 断言 `localStorage['fin-cal-config']` JSON 一致 → 整页刷新 → 断言表单回填与保存值一致。

## 测试结果（2026-10-18，无头 Chromium）

14/14 通过。证据：`docs/test-evidence/`。

| 用例 | 覆盖 | 结果 | 证据 |
|---|---|---|---|
| T1 | 11 项默认值与 HTML 标注一致 | 通过 | t1-settings-default.png |
| T2 | 字号滑块拖动→标签实时更新 | 通过 | — |
| T3 | 接口测试按钮→2 张模拟卡片+说明文字 | 通过 | t3-interface-test.png |
| T4a/b | 全量改值→保存→localStorage→刷新回填 | 通过 | t4-settings-modified.png |
| T5 | 主面板默认渲染基线（15 行，假期 2） | 通过 | t5-panel-default.png |
| T6 | hideHolidays=true → 假期行 0 | 通过 | — |
| T7 | usdMode=true → bull/bear 着色翻转（中性绿↔美元红等） | 通过 | t7-panel-usdmode.png |
| T8 | font_size=18 → `--fs:18px` | 通过 | — |
| T9 | frosted=false → `no-frosted` + `backdrop-filter:none` | 通过 | t9-panel-nofrosted.png |
| T10a/b | minStars=5 只剩 5 星行；=4 过滤 3 星行 | 通过 | t10-panel-minstars5.png |
| T11 | countries=[CN] → 无 US 行 | 通过 | — |
| T12 | side=left → `body.side-left` | 通过 | — |

测试环境注记：Chromium 152 对 `http://127.0.0.1` 有 Local Network Access 拦截，
改用 `file://` + `--allow-file-access-from-files` 加载（localStorage 与
`fetch('sample-data.json')` 均正常）。

## 发现的问题

### BUG-1（后端契约，已修复 2026-10-18）：通知开关字段名不一致
- 前端（`settings.js`/`app.js`）键名 `notify`；Rust `Config` 字段 `notifications`
 （`state.rs:71`，`notifier.rs:27,82` 消费）。
- 后果（Tauri 内）：① 保存时 serde 忽略未知字段 `notify`，`notifications` 恒为默认
  `true`——用户关闭"数据公布系统通知"**无效**；② `get_state` 返回 `notifications`，
  前端 `fillForm` 读 `c.notify` 得 `undefined`——设置页打开时该开关**恒显示未勾选**。
- 修复：前端统一改用 `notifications` 键（`settings.js` DEFAULT_CONFIG/readForm/fillForm、
  `app.js` DEFAULT_CONFIG+注释），与后端字段对齐；无历史数据迁移问题（新产品）。
- 回归验证（headless Chromium，`/tmp/pptr/regress-bug1.mjs`）：R1 默认勾选 / R2
  取消勾选保存后 localStorage 为 `{"notifications":false}` 且无残留 `notify` 键 /
  R3 刷新回填未勾选 / R4 重新开启保存 `true`——4/4 通过。
