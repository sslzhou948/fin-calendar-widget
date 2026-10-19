//! 实际值回填：从 ForexFactory 官网日历页抓取 Actual 列。
//!
//! 背景：主源 Faireconomy 的 JSON 不含 actual 字段（2026-10-19 确认）。
//! ForexFactory 官网 `https://www.forexfactory.com/calendar` 的表格有 Actual 列，
//! 且实测无 Cloudflare 拦截（2026-10-19）。本模块只做一件事：抓取已公布事件的
//! 实际值，按 (归一化标题, 货币代码) 匹配回填。
//!
//! 设计原则（2026-10-19 用户要求）：
//! - 失败静默：抓不到/解析失败/匹配不上，一律返回空 map，不抛错、不影响主流程。
//! - 只读 Actual 列，不碰其他字段（标题/时间/预期/前值以 Faireconomy 为准）。
//! - 匹配键：标题小写+压缩空格，货币代码原样。POC 验证 10/10 已公布事件匹配成功。

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;
use scraper::{Html, Selector};

use crate::source::{SourceError, BROWSER_UA};

const FF_CALENDAR_URL: &str = "https://www.forexfactory.com/calendar";

/// 标题归一化：小写、压缩连续空白。与 POC 的 normalize_title 一致。
fn normalize_title(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// 匹配键：(归一化标题, 货币代码, 月-日 MM-DD)
/// 日期必须参与匹配，否则同周同名事件（如 GDP 初值/修正值）会误填（2026-10-19 QA major）。
/// 用 MM-DD 而非完整日期，避免跨年时年份推断错误。
pub type ActualKey = (String, String, String);

/// 解析 "SunOct 18" → "10-18"。解析失败返回空串（调用方跳过）。
fn parse_mm_dd(raw: &str) -> String {
    // raw 形如 "SunOct 18" / "MonOct 19"，取后半部分
    let s = raw.trim();
    // 找月份缩写
    const MONTHS: [(&str, &str); 12] = [
        ("Jan", "01"), ("Feb", "02"), ("Mar", "03"), ("Apr", "04"),
        ("May", "05"), ("Jun", "06"), ("Jul", "07"), ("Aug", "08"),
        ("Sep", "09"), ("Oct", "10"), ("Nov", "11"), ("Dec", "12"),
    ];
    for (abbr, num) in MONTHS {
        if let Some(pos) = s.find(abbr) {
            let day_part = s[pos + 3..].trim();
            if let Ok(day) = day_part.parse::<u32>() {
                return format!("{}-{:02}", num, day);
            }
        }
    }
    String::new()
}

/// 抓取 ForexFactory 日历页，返回 { (标题, 货币): actual }。
///
/// 失败时返回空 map（调用方无需处理错误）。
pub async fn fetch_actual_map(client: &Client) -> HashMap<ActualKey, String> {
    match fetch_inner(client).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[ff-actuals] 抓取失败（静默降级）: {e}");
            HashMap::new()
        }
    }
}

async fn fetch_inner(client: &Client) -> Result<HashMap<ActualKey, String>, SourceError> {
    let resp = client
        .get(FF_CALENDAR_URL)
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| SourceError::Network(format!("FF 日历页请求失败: {e}")))?;

    if !resp.status().is_success() {
        return Err(SourceError::Network(format!("FF 日历页 HTTP {}", resp.status())));
    }
    let html = resp
        .text()
        .await
        .map_err(|e| SourceError::Network(format!("FF 日历页读取失败: {e}")))?;

    Ok(parse_actuals(&html))
}

/// 解析 HTML，提取每行的 (标题, 货币, actual)。
///
/// 表格结构（2026-10-19 实测）：每行是 `<tr>`，单元格用 class 区分：
/// `calendar__event`（标题）、`calendar__currency`（货币）、`calendar__actual`（实际值）。
/// 只收录 actual 非空的行（未公布的事件 actual 为空，无需回填）。
fn parse_actuals(html: &str) -> HashMap<ActualKey, String> {
    let doc = Html::parse_document(html);
    let tr_sel = Selector::parse("tr").expect("hardcoded selector tr");
    let cell = |tr: &scraper::ElementRef, cls: &str| -> String {
        let sel = Selector::parse(&format!(".{}", cls)).expect("hardcoded selector class");
        tr.select(&sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default()
    };

    let mut map = HashMap::new();
    for tr in doc.select(&tr_sel) {
        let title = cell(&tr, "calendar__event");
        if title.is_empty() {
            continue;
        }
        let actual = cell(&tr, "calendar__actual");
        if actual.is_empty() {
            continue; // 未公布，无需回填
        }
        let currency = cell(&tr, "calendar__currency");
        let mm_dd = parse_mm_dd(&cell(&tr, "calendar__date"));
        if mm_dd.is_empty() {
            continue; // 日期解析失败，跳过（避免误匹配）
        }
        let key = (normalize_title(&title), currency, mm_dd);
        // 同一事件只保留第一个 actual（避免重复行覆盖）
        map.entry(key).or_insert(actual);
    }
    map
}

/// 用 actual map 回填事件列表。返回回填成功的条数。
///
/// 注意：回填 actual 后，若 kind 原为 Event（无数值时判的），需重算 kind，
/// 否则前端按事件行渲染、实际值不可见（2026-10-19 QA blocker）。
pub fn fill_actuals(
    events: &mut [crate::model::CalendarEvent],
    actual_map: &HashMap<ActualKey, String>,
) -> usize {
    let mut filled = 0;
    for e in events.iter_mut() {
        // 已有 actual 的跳过（理论上 Faireconomy 恒为 None，防御性保留）
        if e.actual.is_some() {
            continue;
        }
        let country_code = country_to_code(&e.country);
        let mm_dd = e.release_at.format("%m-%d").to_string();
        let key = (normalize_title(&e.title), country_code, mm_dd);
        if let Some(actual) = actual_map.get(&key) {
            e.actual = Some(actual.clone());
            // kind 重算：有 actual 的事件应为数据行
            if e.kind == crate::model::EventKind::Event {
                e.kind = crate::mapping::classify_kind(
                    &e.title,
                    e.forecast.as_deref(),
                    e.previous.as_deref(),
                    e.actual.as_deref(),
                );
            }
            filled += 1;
        }
    }
    filled
}

/// Country → Faireconomy 货币代码（与 faireconomy.rs 的 map_country 互逆）。
fn country_to_code(c: &crate::model::Country) -> String {
    use crate::model::Country::*;
    match c {
        US => "USD",
        CN => "CNY",
        EU => "EUR",
        UK => "GBP",
        JP => "JPY",
        AU => "AUD",
        NZ => "NZD",
        CA => "CAD",
        CH => "CHF",
        Other => "All",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_title_basic() {
        assert_eq!(normalize_title("GDP q/y"), "gdp q/y");
        assert_eq!(normalize_title("  CPI   m/m "), "cpi m/m");
        assert_eq!(normalize_title("Unemployment Rate"), "unemployment rate");
    }

    #[test]
    fn parse_actuals_sample() {
        // 最小可复现的 FF 表格片段（结构仿 2026-10-19 实测）
        let html = r#"
        <table><tbody>
        <tr>
          <td class="calendar__cell calendar__date">SunOct 18</td>
          <td class="calendar__cell calendar__event">GDP q/y</td>
          <td class="calendar__cell calendar__currency">CNY</td>
          <td class="calendar__cell calendar__actual">4.5%</td>
          <td class="calendar__cell calendar__forecast">4.4%</td>
          <td class="calendar__cell calendar__previous">4.3%</td>
        </tr>
        <tr>
          <td class="calendar__cell calendar__date">MonOct 19</td>
          <td class="calendar__cell calendar__event">CPI m/m</td>
          <td class="calendar__cell calendar__currency">CAD</td>
          <td class="calendar__cell calendar__actual"></td>
          <td class="calendar__cell calendar__forecast">0.2%</td>
          <td class="calendar__cell calendar__previous">-0.1%</td>
        </tr>
        </tbody></table>
        "#;
        let map = parse_actuals(html);
        // 只有 actual 非空的 GDP 被收录
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&("gdp q/y".to_string(), "CNY".to_string(), "10-18".to_string())),
            Some(&"4.5%".to_string())
        );
    }

    #[test]
    fn parse_mm_dd_basic() {
        assert_eq!(parse_mm_dd("SunOct 18"), "10-18");
        assert_eq!(parse_mm_dd("MonOct 19"), "10-19");
        assert_eq!(parse_mm_dd("WedJan 1"), "01-01");
        assert_eq!(parse_mm_dd(""), "");
        assert_eq!(parse_mm_dd("garbage"), "");
    }

    #[test]
    fn parse_actuals_empty_html() {
        // 空/异常 HTML 不 panic，返回空 map
        assert!(parse_actuals("").is_empty());
        assert!(parse_actuals("<html></html>").is_empty());
    }
}
