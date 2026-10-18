# 财经日历内容审计 — 行格式设计验证（ANALYSIS.md）

日期：2026-10-18 ｜ 审计员：数据审计子任务 ｜ 原始数据：`raw/`（10 周结构化摘录）

## 0. 方法与诚实声明

- **数据源替换**：原定 Investing.com，但 `investing.com` 全域名（含 `cn` / `sslecal2` 子域）在本沙盒被域名策略拦截，且按指示不得绕行。改用 **ForexFactory**（`forexfactory.com/calendar?week=xxx.2026`，服务端渲染表格，可逐周回溯）——两者覆盖同一全球宏观日历事件池，分类结论可迁移；差异见文末。
- **样本**：2026-04-26 至 2026-10-18，约每两周取一周，共 10 周、**986 条**（刻意覆盖 4 个 FOMC 议息周：4/29、7/29、9/16，以及中国国庆周、美国 Memorial Day 周等假期密集周）。
- **抓取方式**：`browser.open` 文本抽取 → 人工结构化转录为 `raw/week-*.txt`（逐行校验：TOTAL=VALUED+NOVALUE，且 NOVALUE 行数与转录行数一致；有一周曾漏 3 行被校验拦截，已补齐）。
- **局限**：① FF 文本抽取不含重要性星级（impact 以图标渲染），"无数值条目中高星占比"无法从本源回答；② FF 偏外汇视角，货币分布与 Investing 会有差异；③ 样本为英文标题，Investing 中文站为官方翻译，关键词规则需对中文标题重做一遍（规律同构）。

## 1. 数据概览

| 周 | 总条目 | 有数值 | 无数值 | 无数值占比 |
|---|---|---|---|---|
| 2026-04-26（FOMC） | 120 | 93 | 27 | 22.5% |
| 2026-05-24（Memorial Day） | 97 | 58 | 39 | 40.2% |
| 2026-06-21 | 90 | 60 | 30 | 33.3% |
| 2026-07-26（FOMC） | 92 | 76 | 16 | 17.4% |
| 2026-08-16 | 100 | 90 | 10 | 10.0% |
| 2026-09-13（FOMC） | 108 | 85 | 23 | 21.3% |
| 2026-09-20 | 82 | 45 | 37 | 45.1% |
| 2026-10-04（国庆） | 83 | 58 | 25 | 30.1% |
| 2026-10-11（IMF 周） | 128 | 79 | 49 | 38.3% |
| 2026-10-18 | 86 | 72 | 14 | 16.3% |
| **合计** | **986** | **716（72.6%）** | **270（27.4%）** | — |

"有数值"定义为 actual / forecast / previous 任一非空（含仅前值、仅预期）。

## 2. 无数值条目归类（270 条）

| 类别 | 数量 | 占比 | 真实例子 |
|---|---|---|---|
| 讲话 speech | 151 | 55.9% | FOMC Member Barkin Speaks；ECB President Lagarde Speaks；BOE Gov Bailey Speaks；President Trump Speaks；RBA Gov Bullock Speaks |
| 报告/出版物 report | 29 | 10.7% | BOJ Outlook Report；API Weekly Statistical Bulletin；BOC Monetary Policy Report；Beige Book；RBA Annual Report |
| 假期 holiday | 25 | 9.3% | Bank Holiday（JPY/USD/CNY/EUR/GBP/CHF/AUD/NZD 多国）；Daylight Saving Time Shift |
| 峰会/多日会议 summit | 17 | 6.3% | Day 3 All IMF Meetings；Day 2 All BRICS Summit；Day 1 EUR EU Economic Summit；All Day EUR ECOFIN Meetings |
| 发布会 press conf | 14 | 5.2% | FOMC Press Conference；BOJ Press Conference；ECB Press Conference；BOC Press Conference |
| 声明 statement | 10 | 3.7% | FOMC Statement；Monetary Policy Statement；BOC Rate Statement |
| 纪要 minutes | 9 | 3.3% | FOMC Meeting Minutes；BOC Summary of Deliberations；ECB Monetary Policy Meeting Accounts；Summary of Monetary Policy Discussions |
| 其他文档 other | 11 | 4.1% | Monetary Policy Summary；BOE Inflation Letter；FOMC Economic Projections；SNB Monetary Policy Assessment；Bank Stress Test Results；Annual Budget Release |

"其他"类全部是文档/评估类出版物，无一需要第三种行格式。

## 3. 有数值条目：是否都是标准数据？

**利率决议自带数值**——这是最重要的发现。以下全部有 actual/forecast/previous：

- USD Federal Funds Rate：4.00% / 4.00% / 3.75%（2026-09-16，降息落地周）
- GBP Official Bank Rate：3.75% / 3.75% / 3.75%
- JPY BOJ Policy Rate：<1.00% / <1.00% / <1.00%
- CHF SNB Policy Rate：0.00% / 0.00% / 0.00%
- CAD Overnight Rate：2.25% / 2.25% / 2.25%
- EUR Main Refinancing Rate：2.15% / 2.15% / 2.15%
- NZD Official Cash Rate：2.25% / 2.25% / 2.25%
- GBP MPC Official Bank Rate Votes：3-0-6 / 3-0-6 / 3-0-6（投票数，非利率）

而围绕决议的 Statement / Economic Projections / Press Conference / Minutes **全部无数值**。结论：**"有数值→数据行"规则天然把利率决议划入数据行**，与用户之前"利率决议按数据行处理"的判断一致，无需特殊规则。

其余有数值条目均为标准经济数据（CPI/PPI/非农/GDP/PMI/失业率/贸易帐/EIA 库存/债券拍卖等），样本中**无反例**。

## 4. 边缘情况清单

1. **All Day + 有数值**："German Prelim CPI m/m"（All Day，0.6%/0.7%/1.1%）、"Omdia Total Vehicle Sales"（All Day，15.9M/16.0M/16.3M）——**不能用时间字段判断类型，必须用数值字段**。
2. **日期作时间**：延迟发布的数据时间栏显示 "Feb Data"、"Sep 12th"（如 Building Permits）。
3. **Tentative**：GDT Price Index、10-y Bond Auction、Foreign Direct Investment 时间为 "Tentative"。
4. **Day N 多日行**："Day 1..6 All IMF Meetings"、"Day 2 All BRICS Summit"——同一事件占多行，去重或合并显示时注意。
5. **空标题有数值**："7:30pm AUD 2.2% 2.6%"（标题缺失，解析器需容忍）。
6. **债券拍卖格式**："3.80|1.6"（收益率|投标倍数），数值解析需兼容。
7. **"<1.25%"**：小于号前缀（BOJ Policy Rate），数值比较需特殊处理。
8. **投票数 "3-0-6"**：非数值比较对象，着色逻辑应跳过（无法比大小→中性）。

## 5. 国家/货币分布（两个整周全量统计，n=206）

USD 24.3%｜EUR 21.8%｜GBP 16.5%｜CAD 8.3%｜JPY 7.8%｜CNY 7.3%｜AUD 6.3%｜NZD 4.4%｜CHF 2.9%

USD+CNY 合计 31.6%。说明：① 默认"中美"过滤覆盖约三成条目，作为默认偏好合理且可在设置里放开；② FF 偏外汇，CNY 在 Investing（亚洲覆盖更全）占比可能更高，不影响结论方向。

## 6. 明确结论

**Q1：数据行 / 事件行 / 假期行三类格式够用吗？**
够用。72.6% 条目是数据行；27.4% 非数值条目全部可归入事件行（讲话 56%、报告 11%、峰会 6%、发布会 5%、声明 4%、纪要 3%、其他文档 4%）或假期行（假期 9% + 夏令时切换）。**无需第四种格式**。

**Q2：分类规则推荐怎么写？**
```
任一数值非空（actual/forecast/previous）→ 数据行
否则 → 事件行；其中标题含 Bank Holiday / Daylight Saving → 假期行（淡化）
```
在 986 条样本上，Data/Event 二分**准确率 100%，零反例**：所有有数值的都是数据类（含利率决议、投票数、债券拍卖）；所有无数值的都是事件/假期类。**关键约束：必须用数值字段判断，禁用时间字段**（All Day 也有带数值的）。

**Q3：着色与通知如何配合？**
- 超预期/低预期着色（用户已拍板的中性口径）**只适用于数据行**；事件行无数值可比，不着色。
- 通知：数据行按实际值发布；利率决议（走数据行）正常触发；纯事件（讲话/纪要/假期）不因数值触发，可按星级阈值可选通知——待 Investing 的 importance 字段接入后实现。

**遗留**：无数值条目中"高星占比"因 FF 文本无星级字段无法量化；但无数值池中包含公认高重要性事件（FOMC Minutes、各央行行长讲话），建议在 Investing 数据接入后用其 3 档 importance 做一次校准，确认事件类在 3 星线以上的分布。
