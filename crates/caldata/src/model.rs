//! 统一内部模型。数值一律存原始字符串（如 `"-1.0%"`、`"2,349M"`），
//! **不做数值解析** —— 解析只发生在利多/利空判定的宽容解析器里
//! （见 [`crate::mapping::parse_number`]），解析失败 → 中性（不断言）。

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 事件来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    /// 金十经济日历 WebSocket（`wss://wss-flash-2.jin10.com/`）
    ///（2026-10-23 起为默认主源：中文标题、原生星级、自带实际值）
    Jin10,
    /// `GET https://nfs.faireconomy.media/ff_calendar_thisweek.json`
    ///（ForexFactory 日历公开 JSON，免认证，备源）
    Faireconomy,
    /// `GET https://sslecal2.investing.com`（官方公开小工具接口）
    Primary,
    /// `POST https://cn.investing.com/economic-calendar/Service/getCalendarFilteredData`（社区接口）
    Backup,
}

/// 国家/地区。未知 → [`Country::Other`]，默认被用户配置的国家过滤掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Country {
    US,
    CN,
    EU,
    UK,
    JP,
    AU,
    NZ,
    CA,
    CH,
    Other,
}

impl Country {
    /// 展示用货币/地区码（Other 显示 `--`）。
    pub fn code(self) -> &'static str {
        match self {
            Country::US => "US",
            Country::CN => "CN",
            Country::EU => "EU",
            Country::UK => "UK",
            Country::JP => "JP",
            Country::AU => "AU",
            Country::NZ => "NZ",
            Country::CA => "CA",
            Country::CH => "CH",
            Country::Other => "--",
        }
    }
}

/// 条目类型（来自数据审计：10 周 986 条真实样本）。
///
/// 分类规则（100% 准确）：
/// - 标题含假期/夏令时关键词 → [`EventKind::Holiday`]
/// - 任一数值字段（今值/预测值/前值）非空 → [`EventKind::Data`]
/// - 否则 → [`EventKind::Event`]（讲话/纪要/发布会等）
///
/// **必须用数值字段判断，禁用时间字段**："All Day" 行也可能带数值
/// （如 German Prelim CPI）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    /// 数据行：有数值（72.6%）
    Data,
    /// 事件行：无数值（讲话/纪要/发布会等）
    Event,
    /// 假期行：银行假日/夏令时切换等
    Holiday,
}
///
/// 统一日历事件。
///
/// `release_at` 序列化为 RFC3339 字符串（如 `"2026-10-19T10:00:00+08:00"`），
/// 反序列化时解析回 Asia/Shanghai 时间（`chrono-tz::Tz` 未实现 serde 的
/// `TimeZone` trait，故手写实现）。
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    /// 稳定去重 id：`{source}-{YYYYMMDD}-{序号}`。
    pub id: String,
    /// 发布时间（内部统一 [`crate::schedule::TZ`] = Asia/Shanghai）。
    pub release_at: DateTime<Tz>,
    pub country: Country,
    /// 中文事件名（Investing 官方中文站直接返回中文，无需机翻）。
    pub title: String,
    /// 重要性，1–5 星（内部统一口径）。
    pub importance: u8,
    /// 预期值（原始字符串）。
    pub forecast: Option<String>,
    /// 前值（原始字符串）。
    pub previous: Option<String>,
    /// 实际公布值（原始字符串，未公布为 None）。
    pub actual: Option<String>,
    /// 条目类型（见 [`EventKind`]）：解析时用
    /// [`crate::mapping::classify_kind`] 计算。
    pub kind: EventKind,
    /// 事件行状态：原始时间文本含 "tentative"（大小写不敏感）时为
    /// `Some("待定")`，否则 None。
    pub status: Option<String>,
    pub source: Source,
}

impl CalendarEvent {
    /// 该事件所属自然日（Asia/Shanghai）。
    pub fn date(&self) -> NaiveDate {
        self.release_at.date_naive()
    }
}

impl Serialize for CalendarEvent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("CalendarEvent", 11)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("release_at", &self.release_at.to_rfc3339())?;
        st.serialize_field("country", &self.country)?;
        st.serialize_field("title", &self.title)?;
        st.serialize_field("importance", &self.importance)?;
        st.serialize_field("forecast", &self.forecast)?;
        st.serialize_field("previous", &self.previous)?;
        st.serialize_field("actual", &self.actual)?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("status", &self.status)?;
        st.serialize_field("source", &self.source)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for CalendarEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Helper {
            id: String,
            release_at: String,
            country: Country,
            title: String,
            importance: u8,
            forecast: Option<String>,
            previous: Option<String>,
            actual: Option<String>,
            kind: EventKind,
            status: Option<String>,
            source: Source,
        }
        let h = Helper::deserialize(d)?;
        let release_at = chrono::DateTime::parse_from_rfc3339(&h.release_at)
            .map_err(serde::de::Error::custom)?
            .with_timezone(&chrono_tz::Asia::Shanghai);
        Ok(CalendarEvent {
            id: h.id,
            release_at,
            country: h.country,
            title: h.title,
            importance: h.importance,
            forecast: h.forecast,
            previous: h.previous,
            actual: h.actual,
            kind: h.kind,
            status: h.status,
            source: h.source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn serde_roundtrip() {
        let e = CalendarEvent {
            id: "primary-20261019-0001".to_string(),
            release_at: chrono_tz::Asia::Shanghai
                .with_ymd_and_hms(2026, 10, 19, 10, 0, 0)
                .single()
                .unwrap(),
            country: Country::CN,
            title: "中国季度GDP年率".to_string(),
            importance: 3,
            forecast: Some("5.1%".to_string()),
            previous: Some("5.2%".to_string()),
            actual: Some("5.2%".to_string()),
            kind: EventKind::Data,
            status: None,
            source: Source::Primary,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: CalendarEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, e.id);
        assert_eq!(back.release_at, e.release_at);
        assert_eq!(back.country, e.country);
        assert_eq!(back.title, e.title);
        assert_eq!(back.importance, e.importance);
        assert_eq!(back.forecast, e.forecast);
        assert_eq!(back.actual, e.actual);
        assert_eq!(back.kind, EventKind::Data);
        assert_eq!(back.status, None);
    }

    #[test]
    fn event_kind_serialization() {
        // 序列化为 "data"/"event"/"holiday"，视图层可直接用
        assert_eq!(
            serde_json::to_string(&EventKind::Data).unwrap(),
            r#""data""#
        );
        assert_eq!(
            serde_json::to_string(&EventKind::Event).unwrap(),
            r#""event""#
        );
        assert_eq!(
            serde_json::to_string(&EventKind::Holiday).unwrap(),
            r#""holiday""#
        );
        let back: EventKind = serde_json::from_str(r#""holiday""#).unwrap();
        assert_eq!(back, EventKind::Holiday);
    }
}
