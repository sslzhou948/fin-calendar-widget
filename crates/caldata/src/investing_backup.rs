//! 备源：`POST https://cn.investing.com/economic-calendar/Service/getCalendarFilteredData`
//!
//! 社区多年在用的非官方接口（MQL5 生态等），字段最规范。
//! 请求要求（社区共识）：POST 表单 + 浏览器 UA + `X-Requested-With: XMLHttpRequest`
//! + `Referer: https://cn.investing.com/economic-calendar/`。
//!
//! ⚠️ 未经 live 验证，build 第一步须实测：
//! - `timeZone` 参数语义：此处按"返回时间已是所请求时区"处理，传 `8` 并按
//!   Asia/Shanghai 解析；若实测返回的是 UTC 或其他时区，改 [`parse_backup_time`]。
//! - `country[]` 数字 ID：与主源共用 [`crate::mapping::DEFAULT_COUNTRY_IDS`]
//!  （仅 `5=美国` 确认，其余 UNVERIFIED）。
//! - `importance[]`：1 低 / 2 中 / 3 高。

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone};
use reqwest::Client;
use serde::Deserialize;

use crate::mapping::{classify_kind, tentative_status, ImportanceMap, DEFAULT_COUNTRY_IDS};
use crate::model::{CalendarEvent, Country, Source};
use crate::schedule::TZ;
use crate::source::{DataSource, SourceError, BROWSER_UA};

const BACKUP_URL: &str =
    "https://cn.investing.com/economic-calendar/Service/getCalendarFilteredData";
const REFERER: &str = "https://cn.investing.com/economic-calendar/";

/// 社区文档中的返回形状：`{"data": [[时间, 国家, 事件名, 今值, 预测值, 前值, 重要性], ...]}`。
#[derive(Debug, Deserialize)]
struct BackupResponse {
    #[serde(default)]
    data: Vec<Vec<String>>,
}

pub struct InvestingBackup {
    client: Client,
    countries: String,
    importance: ImportanceMap,
}

impl InvestingBackup {
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

    pub fn with_countries(mut self, ids: &str) -> Self {
        self.countries = ids.to_string();
        self
    }

    pub fn with_importance(mut self, m: ImportanceMap) -> Self {
        self.importance = m;
        self
    }
}

#[async_trait]
impl DataSource for InvestingBackup {
    fn name(&self) -> &'static str {
        "investing-getCalendarFilteredData"
    }

    async fn fetch_week(&self, week_start: NaiveDate) -> Result<Vec<CalendarEvent>, SourceError> {
        let date_from = week_start.format("%Y-%m-%d").to_string();
        let date_to = (week_start + chrono::Duration::days(6))
            .format("%Y-%m-%d")
            .to_string();

        // 表单：country[] / importance[] 为重复键
        let mut form: Vec<(&str, &str)> = vec![
            ("dateFrom", date_from.as_str()),
            ("dateTo", date_to.as_str()),
            // ⚠️ timeZone 语义待 build 实测；暂按"请求 GMT+8 返回 GMT+8"处理
            ("timeZone", "8"),
            ("timeFrame", "custom"),
        ];
        for id in self.countries.split(',') {
            let id = id.trim();
            if !id.is_empty() {
                form.push(("country[]", id));
            }
        }
        for imp in ["1", "2", "3"] {
            form.push(("importance[]", imp));
        }

        let resp = self
            .client
            .post(BACKUP_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Referer", REFERER)
            .form(&form)
            .send()
            .await
            .map_err(|e| SourceError::Network(format!("请求失败: {e}")))?;

        let status = resp.status();
        if status.as_u16() == 403 {
            return Err(SourceError::AuthBlocked(format!(
                "HTTP 403（可能被 Cloudflare 拦截），URL={BACKUP_URL}"
            )));
        }
        if !status.is_success() {
            return Err(SourceError::Network(format!("HTTP 状态异常: {status}")));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| SourceError::Network(format!("读取响应失败: {e}")))?;
        parse_backup_json(&text, &self.importance)
    }
}

/// 解析 `getCalendarFilteredData` 返回的 JSON（纯函数，可离线单测）。
///
/// 行格式（社区文档）：`[时间, 国家, 事件名, 今值, 预测值, 前值, 重要性]`，
/// 时间如 `"Oct 19, 2026 10:00"`。
pub fn parse_backup_json(
    text: &str,
    importance_map: &ImportanceMap,
) -> Result<Vec<CalendarEvent>, SourceError> {
    let resp: BackupResponse = serde_json::from_str(text)
        .map_err(|e| SourceError::Parse(format!("JSON 解析失败: {e}")))?;

    let mut events: Vec<CalendarEvent> = Vec::new();
    let mut seq: u32 = 0;
    for row in resp.data.iter() {
        if row.len() < 7 {
            continue; // 列数不足的行跳过
        }
        let time_str = row[0].trim();
        let country_name = row[1].trim();
        let title = row[2].trim();
        if title.is_empty() {
            continue;
        }
        let actual = opt_value(&row[3]);
        let forecast = opt_value(&row[4]);
        let previous = opt_value(&row[5]);
        let level: u8 = match row[6].trim().parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let stars = match importance_map.stars(level) {
            Some(s) => s,
            None => continue,
        };
        let release_at = match parse_backup_time(time_str) {
            Some(dt) => dt,
            None => continue,
        };
        let country = Country::from_name(country_name);

        let kind = classify_kind(
            title,
            forecast.as_deref(),
            previous.as_deref(),
            actual.as_deref(),
        );
        // 事件行状态：原始时间文本含 "tentative"（大小写不敏感）→ "待定"
        let status = tentative_status(time_str);

        seq += 1;
        events.push(CalendarEvent {
            id: format!("backup-{}-{seq:04}", release_at.format("%Y%m%d")),
            release_at,
            country,
            title: title.to_string(),
            importance: stars,
            forecast,
            previous,
            actual,
            kind,
            status,
            source: Source::Backup,
        });
    }

    if events.is_empty() {
        return Err(SourceError::Empty(
            "JSON 解析成功但未提取到任何事件（字段格式可能已变化）".to_string(),
        ));
    }
    Ok(events)
}

/// 解析返回中的时间字符串。
///
/// ⚠️ 语义假设（待 build 实测）：请求 `timeZone=8` 后返回的时间即为 GMT+8，
/// 此处按 Asia/Shanghai 直接解析。若实测返回 UTC，需在此做换算。
fn parse_backup_time(s: &str) -> Option<chrono::DateTime<chrono_tz::Tz>> {
    let s = s.trim();
    // 社区文档常见格式
    const FORMATS: &[&str] = &[
        "%b %d, %Y %H:%M", // "Oct 19, 2026 10:00"
        "%Y-%m-%d %H:%M",  // "2026-10-19 10:00"
        "%Y/%m/%d %H:%M",  // "2026/10/19 10:00"
        "%b %d, %Y %I:%M %p",
    ];
    for fmt in FORMATS {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return TZ.from_local_datetime(&naive).single();
        }
    }
    None
}

fn opt_value(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t == "—" || t == "-" || t == "–" {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Country;

    const FIXTURE: &str = include_str!("tests/fixtures/backup_sample.json");

    fn parsed() -> Vec<CalendarEvent> {
        parse_backup_json(FIXTURE, &ImportanceMap::default()).expect("fixture 应解析成功")
    }

    #[test]
    fn parses_all_rows() {
        assert_eq!(parsed().len(), 5);
    }

    #[test]
    fn three_value_columns_extracted() {
        let e = parsed()
            .into_iter()
            .find(|e| e.title.contains("Trade Balance"))
            .expect("应有 Trade Balance");
        assert_eq!(e.actual.as_deref(), Some("-775M"));
        assert_eq!(e.forecast.as_deref(), Some("-1,349M"));
        assert_eq!(e.previous.as_deref(), Some("-1,420M"));
        // 未公布：actual 为 None
        let boj = parsed()
            .into_iter()
            .find(|e| e.title.contains("BoJ"))
            .unwrap();
        assert_eq!(boj.actual, None);
        assert_eq!(boj.previous.as_deref(), Some("-0.1%"));
    }

    #[test]
    fn star_mapping() {
        let evs = parsed();
        let gdp = evs.iter().find(|e| e.title.contains("GDP")).unwrap();
        assert_eq!(gdp.importance, 3); // 2 档 → 3 星
        let trade = evs.iter().find(|e| e.title.contains("Trade")).unwrap();
        assert_eq!(trade.importance, 5); // 3 档 → 5 星
    }

    #[test]
    fn country_mapping_from_name() {
        let evs = parsed();
        let get = |t: &str| evs.iter().find(|e| e.title.contains(t)).unwrap().country;
        assert_eq!(get("GDP"), Country::CN); // "China"
        assert_eq!(get("Jobless"), Country::US); // "United States"
        assert_eq!(get("Trade"), Country::NZ); // "New Zealand"
        assert_eq!(get("BoJ"), Country::JP); // "Japan"
                                             // 中文国家名见 chinese_country_name_maps
    }

    #[test]
    fn chinese_country_name_maps() {
        let evs = parsed();
        let cpi = evs.iter().find(|e| e.title == "CPI月率").unwrap();
        assert_eq!(cpi.country, Country::US); // fixture 国家列为"美国"
    }

    #[test]
    fn time_parsed_as_shanghai() {
        let e = parsed()
            .into_iter()
            .find(|e| e.title.contains("GDP"))
            .unwrap();
        assert_eq!(
            e.release_at.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-19 10:00"
        );
    }

    #[test]
    fn ids_unique_and_tagged_backup() {
        let evs = parsed();
        let ids: std::collections::HashSet<_> = evs.iter().map(|e| &e.id).collect();
        assert_eq!(ids.len(), evs.len());
        assert!(evs.iter().all(|e| e.id.starts_with("backup-")));
        assert!(evs.iter().all(|e| e.source == Source::Backup));
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = parse_backup_json("not json", &ImportanceMap::default())
            .expect_err("非法 JSON 应返回 Parse 错误");
        assert!(matches!(err, SourceError::Parse(_)));
    }

    #[test]
    fn empty_data_is_empty_error() {
        let err = parse_backup_json(r#"{"data":[]}"#, &ImportanceMap::default())
            .expect_err("空 data 应返回 Empty 错误");
        assert!(matches!(err, SourceError::Empty(_)));
    }

    #[test]
    fn parse_backup_time_formats() {
        let dt = parse_backup_time("Oct 19, 2026 10:00").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2026-10-19 10:00");
        assert!(parse_backup_time("garbage").is_none());
    }

    // ---------------- kind / status ----------------
    use crate::model::EventKind;

    #[test]
    fn kind_classified_on_fixture() {
        let evs = parsed();
        let get = |t: &str| evs.iter().find(|e| e.title.contains(t)).unwrap().kind;
        assert_eq!(get("GDP"), EventKind::Data);
        assert_eq!(get("Jobless"), EventKind::Data);
        assert_eq!(get("Trade"), EventKind::Data);
        assert_eq!(get("BoJ"), EventKind::Data); // 前值 -0.1%
        assert_eq!(get("CPI"), EventKind::Data);
    }

    #[test]
    fn fixture_rows_have_no_tentative_status() {
        // 正常时间文本 → status 为 None
        assert!(parsed().iter().all(|e| e.status.is_none()));
    }
}
