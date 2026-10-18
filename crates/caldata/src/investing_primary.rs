//! 主源：`GET https://sslecal2.investing.com`
//!
//! 官方公开小工具接口（Investing 站长小工具），稳定性最高。
//! 参数：`columns=exc_flags,exc_currency,exc_importance,exc_actual,exc_forecast,exc_previous`
//! `&calType=week&timeZone=28&lang=6&countries=<ids>`。
//!
//! - `timeZone=28`：Investing 时区编号，对应 GMT+8（北京时间）。
//!   返回的时间已是 GMT+8，解析后仍显式归一化到 [`crate::schedule::TZ`]（防源变更）。
//! - `calType=week`：返回当前自然周；`week_start` 参数仅为 trait 兼容保留。
//!
//! ⚠️ 真实 HTML 结构未经 live 验证：解析器按 class 提取（`time/flagCur/event/
//! sentiment/act/fore/prev`），列顺序变化不影响；日期分组行用
//! `YYYY年M月D日` 文本识别。若真实页面结构不同，单测 fixture 会先暴露。

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone};
use reqwest::Client;
use scraper::{Html, Selector};

use crate::mapping::{classify_kind, tentative_status, ImportanceMap, DEFAULT_COUNTRY_IDS};
use crate::model::{CalendarEvent, Country, Source};
use crate::schedule::TZ;
use crate::source::{DataSource, SourceError, BROWSER_UA};

const PRIMARY_URL: &str = "https://sslecal2.investing.com";
const COLUMNS: &str = "exc_flags,exc_currency,exc_importance,exc_actual,exc_forecast,exc_previous";

pub struct InvestingPrimary {
    client: Client,
    countries: String,
    importance: ImportanceMap,
}

impl InvestingPrimary {
    pub fn new() -> Result<Self, SourceError> {
        let client = Client::builder()
            .user_agent(BROWSER_UA)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| SourceError::Network(format!("构建 HTTP 客户端失败: {e}")))?;
        Ok(Self {
            client,
            countries: DEFAULT_COUNTRY_IDS.to_string(),
            importance: ImportanceMap::default(),
        })
    }

    /// 覆盖默认的 countries ID 列表（UNVERIFIED 项核对后更新）。
    pub fn with_countries(mut self, ids: &str) -> Self {
        self.countries = ids.to_string();
        self
    }

    /// 注入重要性映射（默认 `{1:1, 2:3, 3:5}`，一周真实数据后校准）。
    pub fn with_importance(mut self, m: ImportanceMap) -> Self {
        self.importance = m;
        self
    }
}

#[async_trait]
impl DataSource for InvestingPrimary {
    fn name(&self) -> &'static str {
        "investing-sslecal2"
    }

    async fn fetch_week(&self, _week_start: NaiveDate) -> Result<Vec<CalendarEvent>, SourceError> {
        // calType=week 返回当前周；week_start 仅为 trait 签名兼容。
        let resp = self
            .client
            .get(PRIMARY_URL)
            .query(&[
                ("columns", COLUMNS),
                ("calType", "week"),
                ("timeZone", "28"), // GMT+8
                ("lang", "6"),      // 中文
                ("countries", self.countries.as_str()),
            ])
            .send()
            .await
            .map_err(|e| SourceError::Network(format!("请求失败: {e}")))?;

        let status = resp.status();
        if status.as_u16() == 403 {
            return Err(SourceError::AuthBlocked(format!(
                "HTTP 403（可能被 Cloudflare 拦截），URL={PRIMARY_URL}"
            )));
        }
        if !status.is_success() {
            return Err(SourceError::Network(format!("HTTP 状态异常: {status}")));
        }
        let html = resp
            .text()
            .await
            .map_err(|e| SourceError::Network(format!("读取响应失败: {e}")))?;
        parse_primary_html(&html, &self.importance)
    }
}

/// 解析 sslecal2 返回的 HTML 表格（纯函数，可离线单测）。
///
/// 行识别：
/// - 日期分组行：首个 `<td>` 文本含 `YYYY年M月D日`（如"2026年10月19日星期一"）
/// - 事件行：按 class 提取各列；重要性 = 实心牛头图标（class 含 `FullBullish`）个数
pub fn parse_primary_html(
    html: &str,
    importance_map: &ImportanceMap,
) -> Result<Vec<CalendarEvent>, SourceError> {
    let doc = Html::parse_document(html);
    let tr_sel = Selector::parse("tr").expect("静态选择器 tr");
    let td_sel = Selector::parse("td").expect("静态选择器 td");
    let icon_sel = Selector::parse("i").expect("静态选择器 i");

    let mut events: Vec<CalendarEvent> = Vec::new();
    let mut current_date: Option<NaiveDate> = None;
    let mut seq: u32 = 0;

    for tr in doc.select(&tr_sel) {
        let cells: Vec<_> = tr.select(&td_sel).collect();
        if cells.is_empty() {
            continue;
        }
        let first_text = text_of(&cells[0]);

        // 1) 日期分组行
        if let Some(d) = parse_cn_date(&first_text) {
            current_date = Some(d);
            continue;
        }

        // 2) 事件行（必须已有日期上下文）
        let date = match current_date {
            Some(d) => d,
            None => continue,
        };

        let time_text = cell_by_class(&cells, "time");
        let currency_text = cell_by_class(&cells, "flagCur");
        let title = cell_by_class(&cells, "event");
        let sentiment_cell = cells.iter().find(|c| has_class(c, "sentiment"));
        let actual = opt_value(cell_by_class(&cells, "act"));
        let forecast = opt_value(cell_by_class(&cells, "fore"));
        let previous = opt_value(cell_by_class(&cells, "prev"));

        if title.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
            continue; // 无标题的行跳过
        }
        let title = title.unwrap();

        // 重要性：数实心牛头图标
        let level: u8 = match sentiment_cell {
            Some(cell) => {
                let mut full = 0u8;
                let mut any_icon = false;
                for icon in cell.select(&icon_sel) {
                    any_icon = true;
                    if icon
                        .value()
                        .attr("class")
                        .map(|c| c.contains("FullBullish"))
                        .unwrap_or(false)
                    {
                        full = full.saturating_add(1);
                    }
                }
                if !any_icon {
                    continue; // 无重要性图标的行跳过（结构异常）
                }
                full.clamp(1, 3)
            }
            None => continue,
        };
        let stars = match importance_map.stars(level) {
            Some(s) => s,
            None => continue,
        };

        // 国家：优先货币代码
        let currency = currency_text
            .as_deref()
            .and_then(extract_currency_code)
            .unwrap_or_default();
        let country = Country::from_currency(&currency);

        // 时间：HH:MM（GMT+8）或 全天
        let release_at = match parse_time_cell(time_text.as_deref(), date) {
            Some(dt) => dt,
            None => continue,
        };

        let kind = classify_kind(
            &title,
            forecast.as_deref(),
            previous.as_deref(),
            actual.as_deref(),
        );
        // 事件行状态：原始时间文本含 "tentative"（大小写不敏感）→ "待定"
        let status = time_text.as_deref().and_then(tentative_status);

        seq += 1;
        events.push(CalendarEvent {
            id: format!("primary-{}-{seq:04}", date.format("%Y%m%d")),
            release_at,
            country,
            title,
            importance: stars,
            forecast,
            previous,
            actual,
            kind,
            status,
            source: Source::Primary,
        });
    }

    if events.is_empty() {
        return Err(SourceError::Empty(
            "HTML 解析成功但未提取到任何事件（页面结构可能已变化）".to_string(),
        ));
    }
    Ok(events)
}

/// "2026年10月19日星期一" → NaiveDate。非日期文本返回 None。
fn parse_cn_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    let year_end = s.find('年')?;
    let rest = &s[year_end + '年'.len_utf8()..];
    let month_end = rest.find('月')?;
    let rest2 = &rest[month_end + '月'.len_utf8()..];
    let day_end = rest2.find('日')?;
    let year: i32 = s[..year_end].trim().parse().ok()?;
    let month: u32 = rest[..month_end].trim().parse().ok()?;
    let day: u32 = rest2[..day_end].trim().parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// 时间列："HH:MM" → 当日该时刻（Asia/Shanghai）；"全天" → 当日 00:00。
///
/// 时间文本含 `"tentative"`（如 OPEC 会议）也按当日 00:00 处理：这类行若被
/// 丢弃，[`crate::mapping::tentative_status`] 赋值的 `status` 将永远到不了
/// 构造点。
fn parse_time_cell(text: Option<&str>, date: NaiveDate) -> Option<chrono::DateTime<chrono_tz::Tz>> {
    let t = text?.trim();
    if t == "全天" || t.is_empty() || t.to_lowercase().contains("tentative") {
        return TZ
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
            .single();
    }
    let (h, m) = t.split_once(':')?;
    let hour: u32 = h.trim().parse().ok()?;
    let min: u32 = m.trim().parse().ok()?;
    if hour > 23 || min > 59 {
        return None;
    }
    TZ.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, min, 0)
        .single()
}

/// 从货币列文本中提取 3 位大写货币代码（如 "CNY"）。
fn extract_currency_code(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|tok| tok.len() == 3 && tok.chars().all(|c| c.is_ascii_uppercase()))
        .map(|s| s.to_string())
}

/// 空字符串 / "—" / "-" 视为缺失值。
fn opt_value(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let t = v.trim();
        if t.is_empty() || t == "—" || t == "-" || t == "–" {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn text_of(cell: &scraper::ElementRef) -> String {
    cell.text().collect::<Vec<_>>().join(" ").trim().to_string()
}

fn has_class(cell: &scraper::ElementRef, class: &str) -> bool {
    cell.value()
        .attr("class")
        .map(|c| c.split_whitespace().any(|p| p == class))
        .unwrap_or(false)
}

fn cell_by_class(cells: &[scraper::ElementRef], class: &str) -> Option<String> {
    cells
        .iter()
        .find(|c| has_class(c, class))
        .map(text_of)
        .filter(|s| !s.is_empty())
}

use chrono::Datelike as _;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Country;

    const FIXTURE: &str = include_str!("tests/fixtures/primary_sample.html");

    fn parsed() -> Vec<CalendarEvent> {
        parse_primary_html(FIXTURE, &ImportanceMap::default()).expect("fixture 应解析成功")
    }

    #[test]
    fn parses_all_rows() {
        let evs = parsed();
        assert_eq!(evs.len(), 8);
    }

    #[test]
    fn three_value_columns_extracted() {
        // 新西兰贸易帐：三值齐全
        let e = parsed()
            .into_iter()
            .find(|e| e.title == "新西兰贸易帐")
            .expect("应有新西兰贸易帐");
        assert_eq!(e.actual.as_deref(), Some("-775M"));
        assert_eq!(e.forecast.as_deref(), Some("-1,349M"));
        assert_eq!(e.previous.as_deref(), Some("-1,420M"));
        // 未公布的事件：actual 为 None
        let boj = parsed()
            .into_iter()
            .find(|e| e.title == "日本央行利率决议")
            .expect("应有日本央行利率决议");
        assert_eq!(boj.actual, None);
        assert_eq!(boj.previous.as_deref(), Some("-0.1%"));
    }

    #[test]
    fn star_mapping_two_three_one() {
        let evs = parsed();
        let gdp = evs.iter().find(|e| e.title == "中国季度GDP年率").unwrap();
        assert_eq!(gdp.importance, 3); // Investing 2 档 → 3 星
        let rightmove = evs
            .iter()
            .find(|e| e.title == "英国Rightmove平均房屋要价指数月率")
            .unwrap();
        assert_eq!(rightmove.importance, 1); // Investing 1 档 → 1 星
        assert_eq!(rightmove.actual.as_deref(), Some("-1.0%"));
        let nz = evs.iter().find(|e| e.title == "新西兰贸易帐").unwrap();
        assert_eq!(nz.importance, 5); // Investing 3 档 → 5 星
        let cpi = evs.iter().find(|e| e.title == "美国CPI年率").unwrap();
        assert_eq!(cpi.importance, 5);
    }

    #[test]
    fn country_mapping_from_currency() {
        let evs = parsed();
        let get = |t: &str| evs.iter().find(|e| e.title == t).unwrap().country;
        assert_eq!(get("中国季度GDP年率"), Country::CN);
        assert_eq!(get("英国Rightmove平均房屋要价指数月率"), Country::UK);
        assert_eq!(get("欧元区财长会议"), Country::EU);
        assert_eq!(get("新西兰贸易帐"), Country::NZ);
        assert_eq!(get("美国初请失业金人数"), Country::US);
        assert_eq!(get("日本央行利率决议"), Country::JP);
        assert_eq!(get("南非CPI年率"), Country::Other); // ZAR 未知 → Other
    }

    #[test]
    fn time_conversion_gmt8() {
        let evs = parsed();
        let gdp = evs.iter().find(|e| e.title == "中国季度GDP年率").unwrap();
        assert_eq!(
            gdp.release_at.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-19 10:00"
        );
        // 美国事件 20:30（GMT+8 页面时间，直接采用）
        let cpi = evs.iter().find(|e| e.title == "美国CPI年率").unwrap();
        assert_eq!(
            cpi.release_at.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-20 20:30"
        );
        // 跨日期分组：10-20 的事件落在 10-20
        let nz = evs.iter().find(|e| e.title == "新西兰贸易帐").unwrap();
        assert_eq!(
            nz.release_at.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-20 05:45"
        );
    }

    #[test]
    fn all_day_event_is_midnight() {
        let e = parsed()
            .into_iter()
            .find(|e| e.title == "欧元区财长会议")
            .unwrap();
        assert_eq!(
            e.release_at.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-19 00:00"
        );
    }

    #[test]
    fn event_ids_stable_and_unique() {
        let evs = parsed();
        let ids: std::collections::HashSet<_> = evs.iter().map(|e| &e.id).collect();
        assert_eq!(ids.len(), evs.len());
        assert!(evs[0].id.starts_with("primary-20261019-"));
        assert!(evs.iter().all(|e| e.source == Source::Primary));
    }

    #[test]
    fn empty_html_returns_empty_error() {
        let err = parse_primary_html("<html><body></body></html>", &ImportanceMap::default())
            .expect_err("空页面应返回 Empty 错误");
        assert!(matches!(err, SourceError::Empty(_)));
    }

    #[test]
    fn parse_cn_date_cases() {
        assert_eq!(
            parse_cn_date("2026年10月19日星期一"),
            NaiveDate::from_ymd_opt(2026, 10, 19)
        );
        assert_eq!(
            parse_cn_date("  2026年1月5日星期一  "),
            NaiveDate::from_ymd_opt(2026, 1, 5)
        );
        assert_eq!(parse_cn_date("时间"), None);
        assert_eq!(parse_cn_date(""), None);
    }

    #[test]
    fn extract_currency_code_cases() {
        assert_eq!(extract_currency_code("CNY").as_deref(), Some("CNY"));
        assert_eq!(extract_currency_code(" USD ").as_deref(), Some("USD"));
        assert_eq!(extract_currency_code("xxx").as_deref(), None);
        assert_eq!(extract_currency_code("").as_deref(), None);
    }

    // ---------------- kind / status ----------------
    use crate::model::EventKind;

    #[test]
    fn kind_classified_on_fixture() {
        let evs = parsed();
        let get = |t: &str| evs.iter().find(|e| e.title == t).unwrap().kind;
        assert_eq!(get("中国季度GDP年率"), EventKind::Data);
        assert_eq!(get("英国Rightmove平均房屋要价指数月率"), EventKind::Data);
        assert_eq!(get("欧元区财长会议"), EventKind::Event); // 三值全空
        assert_eq!(get("新西兰贸易帐"), EventKind::Data);
        assert_eq!(get("美国初请失业金人数"), EventKind::Data);
        assert_eq!(get("日本央行利率决议"), EventKind::Data); // 前值 -0.1%
        assert_eq!(get("南非CPI年率"), EventKind::Data);
        assert_eq!(get("美国CPI年率"), EventKind::Data);
    }

    #[test]
    fn fixture_rows_have_no_tentative_status() {
        assert!(parsed().iter().all(|e| e.status.is_none()));
    }

    #[test]
    fn tentative_time_sets_status_and_midnight() {
        // 时间文本含 Tentative → 按当日 00:00 处理 + status="待定"，该行不被丢弃
        let html = r#"<table><tbody>
<tr><td colspan="7">2026年10月19日星期一</td></tr>
<tr>
  <td class="time">Tentative</td>
  <td class="flagCur">USD</td>
  <td class="event">FOMC Member Barkin Speaks</td>
  <td class="sentiment"><i class="grayFullBullishIcon FullBullish"></i><i class="grayFullBullishIcon FullBullish"></i><i class="grayFullBullishIcon FullBullish"></i></td>
  <td class="act"></td>
  <td class="fore"></td>
  <td class="prev"></td>
</tr>
<tr>
  <td class="time">10:00</td>
  <td class="flagCur">USD</td>
  <td class="event">美国CPI年率</td>
  <td class="sentiment"><i class="grayFullBullishIcon FullBullish"></i></td>
  <td class="act">2.9%</td>
  <td class="fore">2.8%</td>
  <td class="prev">2.9%</td>
</tr>
</tbody></table>"#;
        let evs = parse_primary_html(html, &ImportanceMap::default()).expect("应解析成功");
        assert_eq!(evs.len(), 2);
        let barkin = evs
            .iter()
            .find(|e| e.title == "FOMC Member Barkin Speaks")
            .unwrap();
        assert_eq!(barkin.status.as_deref(), Some("待定"));
        assert_eq!(barkin.kind, EventKind::Event);
        assert_eq!(
            barkin.release_at.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-19 00:00"
        );
        let cpi = evs.iter().find(|e| e.title == "美国CPI年率").unwrap();
        assert_eq!(cpi.status, None);
        assert_eq!(cpi.kind, EventKind::Data);
    }
}
