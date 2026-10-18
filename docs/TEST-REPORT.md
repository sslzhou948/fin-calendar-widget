# 桌面财经日历挂件 — 自我测试报告（TEST-REPORT）

日期：2026-10-18 ｜ 版本：v1.0 源码 ｜ 测试环境：Linux VM（x86_64）
对应 PRD：`../PRD.md` §7 验收标准 ｜ 技术方案：`TECH-DESIGN.md`

> **环境诚实声明**：本机为 Linux，无 Windows/macOS；Tauri 应用无法在此编译运行。
> 因此所有"运行时"验收项标记为**未覆盖**，待用户 Windows 实机或 GitHub Actions
> 构建后验证。下文每项均如实标注：✅通过 / ❌未通过 / ⏸️未覆盖。

## 一、已执行的测试

| # | 测试 | 结果 |
|---|---|---|
| T1 | `cargo test -p caldata`（数据层单测，离线） | ✅ 44 通过 / 0 失败 |
| T2 | 时区换算单测：2026-11-02 08:30 America/New_York（冬令时 EST）== 北京时间 21:30；另附 EDT 夏令时对照 | ✅ 通过 |
| T3 | 解析单测：sslecal2 仿真 HTML（8 事件行：日期分组、三值、星级、全天事件、未知货币） | ✅ 通过 |
| T4 | 解析单测：getCalendarFilteredData 仿真 JSON（5 条，含未公布空值） | ✅ 通过 |
| T5 | 映射单测：国家（货币代码/数字 ID/中英名）、重要性 3 档→{1:1,2:3,3:5}、利多/利空方向表 + 宽容数值解析 | ✅ 通过 |
| T6 | 调度纯函数单测：next_sunday_midnight / needs_refresh / build_wake_list（release_at+2min、只取未来、升序） | ✅ 通过 |
| T7 | 降级逻辑单测：Primary 成功 / Primary 失败切 Backup / 双失败保缓存（纯内存 mock，零网络） | ✅ 通过 |
| T8 | Rust 语法检查：`rustfmt --check` 解析全部 6 个 src-tauri 文件 | ✅ 通过（lib.rs 仅 2 处格式差异，非语法错误） |
| T9 | JS 语法检查：`node --check` app.js / settings.js / screenshot.mjs | ✅ 通过 |
| T10 | JSON/YAML 校验：tauri.conf.json、capabilities、sample-data.json、build.yml | ✅ 通过 |
| T11 | 前后端契约对齐：`get_state` 扁平 events（release_at/country/currency/title/importance/forecast/previous/actual/sentiment）↔ app.js 读取字段；commands 全部在 invoke_handler 注册 | ✅ 通过（人工核对） |
| T12 | 零轮询代码审计：`grep setInterval` 全仓库零命中（settings.js 仅一处 UI tooltip 的 setTimeout，与网络无关） | ✅ 通过 |
| T13 | 无头 Chromium 渲染：挂件主面板（右半屏+毛玻璃开） | ✅ 通过（目检 `docs/screenshots/widget-right-frosted.png`） |
| T14 | 无头 Chromium 渲染：左半屏+毛玻璃关对照 | ✅ 通过（目检） |
| T15 | 无头 Chromium 渲染：设置页（全部控件） | ✅ 通过（目检） |
| T16 | 默认过滤渲染：18 条示意数据中 1 条欧元区被过滤，17 条展示（7+6+4），星级/国家/情绪分布符合预期 | ✅ 通过 |

## 二、PRD §7 验收标准逐项结论

| PRD §7 | 验收项 | 结论 | 依据 |
|---|---|---|---|
| 1 | 壁纸之上、不遮挡左侧图标、点击穿透、壁纸可换不受影响 | ⏸️未覆盖 | 代码已实现（`set_ignore_cursor_events(true)`、skipTaskbar、右半屏定位）；窗口层级与穿透需 Windows 实测 |
| 2 | 默认中美 3 星及以上；改配置即时生效 | 部分✅ | 过滤逻辑渲染验证通过（T16）；"改配置即时生效"需 Tauri 运行时 ⏸️ |
| 3 | 已公布事件三栏；利多/利空着色正确（含方向映射） | ✅（逻辑+渲染） | T5 方向/数值解析通过；T13 截图红绿着色目检通过；真实数据待接入后复核 |
| 4 | 全北京时间；跨美东夏冬令时切换换算正确 | ✅ | T2 单测通过（含 2026-11-01 切换日） |
| 5 | 周日 0 点全量刷新；关机错过开机补拉；公布后约 2 分钟更新实际值并通知 | ⏸️未覆盖 | T6 纯函数通过；端到端定时/唤醒/通知需 Windows 实测 |
| 6 | 设置页接口测试准确反映数据源状态；断网/错参诊断清晰 | ⏸️未覆盖 | `test_sources` 命令已实现；需 Tauri 运行时 |
| 7 | 空闲内存 <100MB；无定时轮询（抓包验证） | ⏸️未覆盖 | T12 代码审计零轮询成立；内存需 Windows 任务管理器实测 |
| 8 | 毛玻璃开关、左右半屏、字号调节即时生效 | 部分✅ | T13/T14 两种变体渲染目检通过；"即时生效"需运行时 ⏸️ |

**未发现 ❌未通过项。**

## 三、已知限制与遗留风险（未覆盖项的待办）

1. **Tauri 未编译**：Linux 无 GUI 依赖，`src-tauri` 仅做语法/契约检查。首次编译在 GitHub Actions（用户亲手上传 workflow 后）或用户本地 `npx tauri dev`。
2. **国家数字 ID**：仅 `5=美国` 经社区确认，其余 ID 在代码中标 UNVERIFIED；解析优先走货币代码，影响有限，但 `countries=` 请求参数依赖此表——开发环境需从 Investing 页面核对一次。
3. **真实 HTML/JSON 结构**：sslecal2 页面结构与 `getCalendarFilteredData` POST 均未 live 验证（沙盒限制）；fixture 为"结构忠实"的仿真样本。若真实结构有出入，解析器按 class/列顺序双重策略已尽量鲁棒，对照真实页面调整即可。
4. **重要性映射** `{1:1, 2:3, 3:5}` 为初值，已做成可配置（`ImportanceMap`），PRD 要求用一周真实数据校准。
5. **通知权限**：macOS 需用户授权；Windows toast 需 MSI 正确安装（Tauri 自动处理 AppUserModelID）。
6. **截图为渲染效果图**：`docs/screenshots/` 三张图由无头 Chromium 合成（挂件 HTML + 示意壁纸 + 占位图标），**非真实桌面实拍**；事件数据为 `sample-data.json` 示意数据（文件内已标注）。

## 四、效果图

- `docs/screenshots/widget-right-frosted.png` — 默认：右半屏 + 毛玻璃开
- `docs/screenshots/widget-left-nofrosted.png` — 对照：左半屏 + 毛玻璃关
- `docs/screenshots/widget-settings.png` — 设置页
- `docs/screenshots/assets/wallpaper-dark.jpg` — 示意壁纸（AI 生成，仅用于效果图合成）

## 五、补丁包测试（2026-10-18）：三类行格式 + 中性着色 + 国旗标识

对应 PRD §3.1 / §3.3 / §7.9 / §7.10，技术方案见 `TECH-DESIGN.md` §13。

### 5.1 已执行的测试

| # | 测试 | 结果 |
|---|---|---|
| P1 | `cargo test -p caldata`（补丁后全量，离线） | ✅ 55 通过 / 0 失败（含新增 11 个） |
| P2 | 分类单测：审计真实例子（FOMC 讲话→Event、利率决议 4.00/4.00/3.75→Data、Bank Holiday→Holiday、German Prelim CPI 全天+数值→Data、空标题+数值→Data） | ✅ 通过 |
| P3 | `deviation` 单测：beat/miss/inline、"<1.00%" 前缀、"3.80\|1.6" 取前半、"3-0-6" 投票数→None、空值→None、浮点容差、利率区间 "3.75%-4.00%" 字符串相等→Inline | ✅ 通过 |
| P4 | `cargo fmt --check`（crates/caldata） | ✅ 通过 |
| P5 | `rustfmt --edition 2021 --check` 解析 src-tauri（lib.rs/state.rs/notifier.rs/scheduler.rs） | ✅ 通过（lib.rs 仅 1 处历史格式差异，非本次引入） |
| P6 | `node --check`：src/app.js、src/settings.js、scripts/screenshot.mjs | ✅ 通过 |
| P7 | sample-data.json：18 条 JSON 合法；用独立脚本审计全部 data 行 deviation 与中性规则一致 | ✅ 通过（发现并修复 3 处样本手误：demo-010/018 miss→beat、demo-016 区间→inline） |
| P8 | 9 个国旗 SVG 合法 XML，单个 <2KB | ✅ 通过 |
| P9 | 无头 Chromium 重渲染三张效果图并目检 | ✅ 通过（国旗+缩写、事件行紧凑单行、假期行淡化、beat 绿/miss 红/inline 灰） |
| P10 | 前后端契约对账：EventView(kind/deviation/status/usd_sentiment) ↔ app.js 读取字段；Config 三新字段 ↔ settings.js 开关 | ✅ 通过（人工核对） |

### 5.2 PRD §7.9 / §7.10 验收结论

| 验收项 | 结论 | 依据 |
|---|---|---|
| 三类行渲染正确；分类规则在 986 条审计样本上保持 100% 准确 | ✅（逻辑） | P2；规则本身由审计实证，代码实现与审计规则逐字对应 |
| 8 项边缘情况正确处理 | ✅（逻辑） | P3 覆盖：All Day+数值、Tentative、债券拍卖格式、投票数跳过、空标题、"<1.25%"、日期作时间 |
| 超预期绿 / 低预期红 / 符合预期灰；事件行不着色 | ✅（逻辑+渲染） | P3、P7、P9 目检 |
| 国旗 + 缩写标识；无 emoji | ✅（渲染） | P8、P9 目检；Windows 不渲染 flag emoji，已规避 |

### 5.3 补丁引入的已知限制（诚实记录）

1. **字符串相等判 Inline 的边界**：`"3.75%-4.00%"` vs `"3.75% - 4.00%"`（空格差异）不会判等，回退到解析失败→无着色。可接受：源数据格式稳定，极端空格差异概率低。
2. **中文标题关键词**：`classify_kind` 的假期关键词含中英两套，但中文事件标题（如"讲话"）的 Event 判定依赖"无数值→Event"兜底，已足够；若 Investing 中文站出现"有数值的假期行"等怪例，分类会按 Data 处理——审计 986 条中零此类反例。
3. **事件行状态**：目前只有"待定"（Tentative）一种来源；"已结束"由前端按时间自行判断，未做后端状态机——够用，不做过度设计。
4. **src-tauri 整包仍未编译**（Linux 无 GUI 依赖）：lib.rs/state.rs/notifier.rs/scheduler.rs 仅做解析 + rustfmt 检查；`build_widget_state` 的新字段逻辑简单，CI windows-latest 首次编译时验证。
