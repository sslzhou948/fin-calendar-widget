# 桌面财经日历挂件 — 技术方案设计（TECH-DESIGN）

版本：v1.0 ｜ 日期：2026-10-18 ｜ 对应 PRD：`../PRD.md` v1.0

## 1. 总览

- **框架**：Tauri v2（Rust 后端 + 系统 WebView 前端）。一套代码编译 Windows / macOS。
- **为什么是 Tauri**：二进制 ~10MB、空闲内存几十 MB（目标 <100MB）；透明窗口、点击穿透、托盘、开机自启、系统通知都有官方插件；前端用普通 HTML/CSS/JS。
- **v1 先行 Windows**，macOS 随后（同一套代码，CI 矩阵构建）。

## 2. 仓库结构

```
fin-calendar/
├── PRD.md
├── docs/
│   ├── TECH-DESIGN.md        # 本文件
│   ├── TEST-REPORT.md        # 自测报告
│   └── screenshots/          # 效果图（渲染图，非实拍，需标注）
├── src/                      # 前端（HTML/CSS/JS）
│   ├── index.html            # 挂件主面板
│   ├── settings.html         # 设置页（含接口测试）
│   ├── styles.css
│   ├── app.js                # 主面板渲染逻辑
│   ├── settings.js
│   └── sample-data.json      # 截图/开发用示意数据（明确标注）
├── src-tauri/                # Tauri 应用壳
│   ├── Cargo.toml            # 依赖 caldata（path）
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── capabilities/
│   │   └── default.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs            # commands / events / tray / 窗口管理
│       ├── scheduler.rs      # 刷新调度（tokio tasks）
│       ├── notifier.rs       # 系统通知（去重）
│       └── state.rs          # AppState：缓存 + 配置读写
├── crates/
│   └── caldata/              # 纯 Rust 数据层（无 Tauri 依赖，可独立 cargo test）
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── model.rs      # 统一内部模型 CalendarEvent
│           ├── source.rs     # DataSource trait + Primary/Backup 实现
│           ├── investing_primary.rs  # sslecal2 HTML 解析
│           ├── investing_backup.rs   # getCalendarFilteredData JSON 解析
│           ├── mapping.rs    # 国家 ID 映射、重要性 3→5 星映射、方向映射表
│           ├── schedule.rs   # 纯函数：下次全量刷新时间、对账判定、唤醒时间表
│           └── tests/        # fixtures（HTML/JSON 样本）+ 单测
├── scripts/
│   └── screenshot.mjs        # 无头 Chromium 渲染效果图脚本
└── .github/
    └── workflows/
        └── build.yml         # Windows msi/nsis + macOS dmg 云构建
```

> **注意**：`.github/workflows/*` 须由用户亲手上传到 GitHub（GitHub App 推送固定 403，见 subwrap 项目经验）。交付时在报告里单独列出。

## 3. 数据层（crates/caldata）

### 3.1 统一内部模型

```rust
pub struct CalendarEvent {
    pub id: String,            // source + 日期 + 序号，稳定去重
    pub release_at: DateTime<Tz>, // Asia/Shanghai（见 §6）
    pub country: Country,      // US | CN | EU | UK | JP | ...
    pub title: String,         // 中文事件名
    pub importance: u8,        // 1–5 星（内部统一）
    pub forecast: Option<String>,
    pub previous: Option<String>,
    pub actual: Option<String>,
    pub source: Source,        // Primary | Backup
}
```

`Option<String>` 存原始字符串（如 "-1.0%"、"2,349M"），不做数值解析——
数值解析是利多/利空判定的坑（百分号、单位、"M"/"K" 后缀），v1 只做"可解析才判定"，
解析失败 → 中性（不断言）。

### 3.2 DataSource trait（可插拔）

```rust
#[async_trait]
pub trait DataSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fetch_week(&self, week_start: NaiveDate) -> Result<Vec<CalendarEvent>, SourceError>;
}
```

- **Primary**：`GET https://sslecal2.investing.com`，参数：
  `columns=exc_flags,exc_currency,exc_importance,exc_actual,exc_forecast,exc_previous`
  `&calType=week&timeZone=28&lang=6&countries=<ids>`。
  返回 HTML 表格 → 用 `scraper` 解析行，提取时间/国家/事件名/星级/三值。
  官方公开接口（站长小工具），稳定性最高。
- **Backup**：`POST https://cn.investing.com/economic-calendar/Service/getCalendarFilteredData`
  （表单：dateFrom/dateTo、country[]、importance[]、timeZone；头：浏览器 UA、
  `X-Requested-With: XMLHttpRequest`、Referer）。返回 JSON → serde 解析。
  社区多年用法，字段最规范；开工后在开发环境实测可用性。
- **SourceError**：`Network` / `Parse` / `AuthBlocked(403/Cloudflare)` / `Empty`，
  携带可读信息，供设置页"接口测试"展示。
- **自动降级**：`fetch_week` 先 Primary，失败 → Backup；都失败 → 保留本地缓存并记录错误。
  配置 `source: auto | primary | backup` 可强制。

### 3.3 映射表（mapping.rs，全部可配置、可单测）

1. **国家数字 ID**：Investing `countries` 参数用数字 ID。已知片段（社区）：`5=美国`；
   POC 抓到的默认列表 `37,46,6,110,14,48,32,17,10,36,43,35,72,22,41,25,12,5,4,26,178,11,39,42`
   需在开发环境从页面一次性核对。实现策略：**ID 表 + 文本回退**——HTML 源有
   `exc_currency`（USD/CNY/EUR/GBP/JPY…）列，优先用货币代码映射国家
   （USD→US、CNY/CNH→CN、EUR→EU、GBP→UK、JPY→JP…），ID 表只做补充。
   未知国家 → `Country::Other`，默认被过滤（用户只看配置的国家）。
2. **重要性 3 档 → 5 星**：默认 `{1:1, 2:3, 3:5}`，写进配置文件可调；
   PRD 要求用一周真实数据校准（重点看 3 星线附近事件是否合理）。
3. **利多/利空方向表**：`higher_better`（CPI/PPI/GDP/非农/零售/PMI/贸易帐…，
   中英关键词匹配）、`lower_better`（失业率、初请失业金人数…）、未知 → 中性不着色。
   判定仅在 `actual` 与 `forecast` 都能解析为数值时进行。

### 3.4 纪律

- 解析逻辑必须用本地 fixture 单测（`crates/caldata/src/tests/`：HTML 样本 +
  JSON 样本，来自 POC 样本与浏览器抓到的真实结构，可手写仿真但必须标注）。
- 需要 live 验证时走 browser 工具，**禁止**用 exec+curl 直接抓网页。

## 4. 刷新调度（src-tauri/src/scheduler.rs）

全部基于 tokio 定时任务，**零轮询**：

1. **整周全量刷新**：每周日 00:00（Asia/Shanghai）`fetch_week`。用 `schedule.rs` 纯函数
   `next_sunday_midnight(now) -> DateTime` 计算下一次触发点，单测覆盖。
2. **开机/唤醒对账**：启动时 + 系统唤醒事件（Tauri 可监听）→ `reconcile()`：
   - 本地缓存记录 `covered_week: (year, week)`；若当前周 ≠ covered_week → 立即全量拉取。
   - 对本周已过发布时间但 `actual` 为空的事件 → 补拉一次（只拉当天/当周增量）。
   - 关机错过周日 0 点 → 下次开机自动补上（PRD §3.4 已确认）。
3. **事件驱动唤醒**：全量数据回来后建唤醒时间表 `[(release_at + 2min, event_id)]`；
   每个事件一个 tokio sleep，到点 `fetch` 当天数据刷新该事件的 `actual`。
   时间表重建时机：全量刷新后、配置变更后、唤醒对账后。
4. **兜底扫尾**（2026-10-18 新增）：每 15 分钟 `sweep_stale_actuals()`，用纯函数
   `schedule::stale_weeks()` 找出"已过发布时间 2 分钟但 `actual` 仍为空"的数据行
   （48h 内的），按周重试拉取。覆盖两种缺口：唤醒那次拉取失败（两路源都挂），
   或数据源公布延迟（+2min 时还没出数）。成功抓到后走正常通知流程。
5. **去重与幂等**：事件 `id` 稳定；重复触发只更新 `actual` 字段。

## 5. 叠加层窗口实现

### Windows（v1 重点）

- Tauri 窗口配置：`transparent: true`、`decorations: false`、`skip_taskbar: true`、
  `always_on_top: false`；运行时 `set_ignore_cursor_events(true)` 实现**纯点击穿透**。
- 定位：主屏工作区右半部分（`available_monitors` 取主屏，宽 42%、全高减任务栏边距）；
  设置可切换左/右。**图标在左 → 面板天然不遮挡**。
- z-order：置于应用窗口层底部（`set_always_on_top(false)` + 启动时 `lower` 一次）。
  **不做** WorkerW-behind-icons 注入（把窗口挂到桌面图标层之下）：该技巧依赖未公开
  行为、随 Windows 版本漂移，v1 不引入此风险；右半屏布局已解决图标遮挡问题。
  如用户后续要求"图标之下"，再评估。

### macOS（v2）

- 同样透明+无边框+点击穿透（`set_ignore_cursor_events`）；`window level` 保持普通层，
  右半屏定位；图标在右侧的用户切到左半屏（设置项已预留）。
- 真正"图标层之下"需 `kCGDesktopWindowLevel`（经 cocoa 取 NSWindow handle 设置），
  列为 v2 调研项，不阻塞 v1。

### 风险

- 点击穿透后窗口完全不可交互是**需求**（PRD），所有操作走托盘/设置页。
- 全屏应用（游戏）时面板会被盖住——可接受，壁纸挂件本就不该打扰全屏。

## 6. 时区

- 内部统一 `chrono-tz::Asia::Shanghai`。Investing `timeZone=28` 返回的已是 GMT+8，
  仍做一次显式归一化（防源变更）。
- 美东夏冬令时：`chrono-tz` 数据库处理 EDT/EST 自动切换；单测用 2026-11-01
  （冬令时切换日）前后的美国事件做换算断言。
- 显示格式：`MM-DD HH:mm` + 星期；临近事件显示"X小时Y分后公布"。

## 7. 系统通知（notifier.rs）

- `@tauri-apps/plugin-notification`，一套 API 通吃 Windows toast / macOS 通知中心。
- 触发：事件驱动唤醒拉到新的 `actual` 且事件星级 ≥ 显示阈值 → 通知
  （标题：事件名；正文：实际 x / 预期 y）。
- **去重**：已通知的 `event_id` 集合持久化，重启不重发。
- 设置可开关，默认开；macOS 需用户授权通知权限（首次触发时请求）。

## 8. 设置页与接口测试

- 独立 `WebviewWindow`（settings.html），非穿透、可交互；从托盘打开。
- 设置项：国家多选、星级阈值、左右半屏、字号、毛玻璃开关（默认开）、通知开关、
  数据源选择（auto/primary/backup）。
- **接口测试按钮**：调 backend `test_sources()` → 对 Primary/Backup 分别报告：
  连通（HTTP 状态）、延迟、解析出行数、最近一次错误原文。一键复制诊断文本。
- 配置持久化：`app_data_dir/config.json`；缓存：`app_data_dir/cache.json`
 （含 covered_week 与已通知 id 集合）。

## 9. 前端渲染（src/）

- `index.html`：按天分组（今天/明天/星期X），每条：时间、国旗/货币码、事件名、
  星级（★）、预期/前值/实际三栏；利多红 / 利空绿 / 中性默认；距公布 <X 小时高亮边框。
- 毛玻璃：**仅面板区域** `backdrop-filter: blur(18px) saturate(1.2)` + `rgba(10,12,20,.55)` 底；
  开关 → 切 class，默认开。
- 字号：CSS 变量 `--fs`，设置可调（12–18px）；JS 在渲染后检查 `scrollHeight > clientHeight`，
  溢出则逐级缩小 `--fs` 直至放下（PRD：溢出自动缩小）。
- 与后端通信：`invoke('get_state')` 拿全量渲染数据；监听 `calendar-updated` 事件增量刷新。
- `sample-data.json`：示意数据，**文件头注明"示意数据，非真实接口返回"**。

## 10. 构建与发布

- GitHub Actions（`.github/workflows/build.yml`）：matrix
  `windows-latest`（tauri-action → msi + nsis）、`macos-latest`（→ dmg）。
- **`.github/workflows/*` 必须由用户亲手上传**（GitHub App 推送 403，subwrap 经验）。
- 本地开发指引：Windows 装 Rust + Node + WebView2（系统自带）→ `npm i` →
  `npx tauri dev`；`npx tauri build` 出安装包。

## 11. 测试策略

- `cargo test -p caldata`：HTML/JSON 解析、映射表、调度纯函数、时区换算（必须全过）。
- 前端：无头 Chromium 渲染截图，人眼验收布局/毛玻璃/着色。
- 验收映射见 `docs/TEST-REPORT.md`（PRD §7 逐项：通过 / 未通过 / 未覆盖，诚实写）。
- 轻量验证：Linux 下无法直接测 Windows 内存，CI 构建产物在用户 Windows 上用任务管理器
  抽查；事件驱动无轮询 → 代码审查保证（无 setInterval 网络请求）+ 日志审计。

## 12. 风险与诚实边界

1. 本 Linux VM **无法编译 Windows/macOS 安装包**：交付物 = 完整源码 + Actions 云构建 +
   本地构建指引。安装包由用户在其机器或 Actions 产出。
2. 效果图为**无头 Chromium 渲染图**（挂件 HTML 叠在示意壁纸上），非真实桌面实拍，
   如实标注。
3. Investing 接口为非官方/官方小工具两条路：小工具接口是官方公开的（最稳）；
   `getCalendarFilteredData` POST 开工后在开发环境实测，沙盒内未 live 验证。
4. 国家数字 ID 映射以货币代码回退为主，ID 表需开发环境核对一次（已在代码注释标注）。

## 13. 设计变更：补丁包（2026-10-18，用户拍板）

背景：用户指出日历不全是数据发布（还有讲话/纪要/假期等），且利多/利空断言因品种而异；
数据审计（`docs/data-audit/ANALYSIS.md`，10 周 986 条）实证三类行格式全覆盖、分类规则 100% 准确。
本章记录相对 v1.0 的设计变更。

### 13.1 三类行格式与分类规则

- `CalendarEvent` 新增 `kind: EventKind { Data, Event, Holiday }`（解析时确定，稳定不变）
  与 `status: Option<String>`（事件行状态，如原始时间为 "Tentative" 时记"待定"）。
- 分类规则（`mapping::classify_kind`）：标题含假期/夏令时关键词（中英：bank holiday /
  daylight saving / 休市 / 假期 / 假日 / 夏令时）→ Holiday；否则任一数值字段
  （今值/预测值/前值）非空 → Data；否则 → Event。**必须用数值字段判断，禁用时间字段**
  （审计证实 All Day 行也可能带数值，如 German Prelim CPI）。
- 利率决议自带数值（Fed Funds Rate 4.00%/4.00%/3.75% 等），天然落入 Data 行，无需特殊规则。
- 渲染：Data 行保留三栏；Event 行紧凑单行（时间 + 国旗 + 国家缩写 + 标题 + 星级 + 状态）；
  Holiday 行淡化单行（`hide_holidays` 配置可隐藏）。

### 13.2 中性预期差着色（替代利多/利空断言）

- `mapping::deviation(actual, forecast) -> Option<Deviation::{Beat,Miss,Inline}>`：
  纯数值比较，不下方向结论。归一化处理审计发现的边缘格式：
  剥离首字符 `</>/≤/≥`（如 "<1.00%"）、`|` 取前半（债券拍卖 "3.80|1.6"）、
  投票数 "3-0-6" 直接跳过（不可比 → None）、空值/破折号 → None；
  相等容差 `eps = 1e-9 * max(1,|a|,|b|)`。
- 颜色：超预期（Beat）→ 绿、低预期（Miss）→ 红、符合预期（Inline）→ 灰；
  仅适用于 Data 行，Event/Holiday 行不着色。
- **deviation 在视图层（`build_widget_state`）实时计算**，不存入事件：
  actual 会在公布后被后续更新刷新，若解析时存死则颜色停留在快照时刻。
- 原 `Direction` / `direction_for` / `sentiment` 保留：`EventView.usd_sentiment`
  仅在配置 `usd_mode=true`（设置可选项"美元口径利多/利空着色"，默认关闭）时计算，
  前端据此恢复 bull→红 / bear→绿的旧着色。

### 13.3 国家标识：国旗 + 缩写（替代货币代码）

- 前端国家标识位移除货币代码（USD/CNY 等），改为圆形小国旗
  （`src/flags/*.svg` 本地打包，零运行时网络开销）+ 国家双字母缩写
  （CN/US/EU/UK/JP…）；SVG 缺失时 fallback 为色块圆 + 缩写。
- 不用 emoji 国旗：Windows 不渲染 flag emoji（显示字母）。

### 13.4 通知逻辑按类型区分（notifier.rs）

- Data 行：沿用原有 `notify_new_actuals`（新 actual + 星级达标 + 开关开）。
- Event 行：新增 `notify_due_events`——仅当配置 `notify_events=true`
  （设置可选项"事件类通知"，默认关闭）时，对发布时间已到 2 小时内、
  星级达标、未通知过的事件发一次"事件时间已到"通知；去重持久化。
- Holiday 行：永不通知。
- 两处通知在 scheduler 的三个调用点（reconcile / full_refresh / fire_due）并列触发。

### 13.5 新增配置项（state.rs Config / 设置页）

| 字段 | 默认 | 说明 |
|---|---|---|
| `usd_mode` | false | 美元口径利多/利空着色（可选项） |
| `notify_events` | false | 事件类（讲话/纪要等）通知 |
| `hide_holidays` | false | 隐藏假期行 |

### 13.6 CI 扩展（.github/workflows/build.yml）

- 新增 `test` job（windows-latest，真机）：`cargo test --manifest-path
  crates/caldata/Cargo.toml`、前端 `node --check`、无头 Chrome 重渲染效果图
  并上传 artifacts；`build` job 加 `needs: [test]` 门禁。
- tauri-driver WebDriver 交互测试已评估并**诚实降级**：runner 无交互式桌面会话，
  透明置底/点击穿透等行为无法被 WebDriver 有效验证，且引入 flaky 风险；
  真机交互与最终视觉确认在用户本机完成。
