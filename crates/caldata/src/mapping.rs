//! 映射表：国家 / 重要性 3→5 星 / 利多利空方向 / 宽容数值解析。
//!
//! 全部可配置、可单测。凡未经 live 核对的一律标注 UNVERIFIED。

use serde::{Deserialize, Serialize};

use crate::model::Country;

/// Investing `countries` 请求参数的默认数字 ID 列表。
///
/// 来源：POC 阶段从日历页面抓到的默认列表。
/// ⚠️ UNVERIFIED：仅 `5=美国` 经社区文档确认，其余 ID ↔ 国家的对应关系
/// 须在开发环境从日历页面核对一次。解析时优先走货币代码
/// （[`Country::from_currency`]），此表只做请求参数的补充。
pub const DEFAULT_COUNTRY_IDS: &str =
    "37,46,6,110,14,48,32,17,10,36,43,35,72,22,41,25,12,5,4,26,178,11,39,42";

impl Country {
    /// 优先的国家映射：货币代码 → 国家。
    ///
    /// HTML 主源（sslecal2）有 `exc_currency` 列（USD/CNY/EUR/GBP/JPY…），
    /// 优先用它，避开数字 ID 表的 UNVERIFIED 坑。
    pub fn from_currency(code: &str) -> Self {
        match code.trim().to_ascii_uppercase().as_str() {
            "USD" => Country::US,
            "CNY" | "CNH" => Country::CN,
            "EUR" => Country::EU,
            "GBP" => Country::UK,
            "JPY" => Country::JP,
            "AUD" => Country::AU,
            "NZD" => Country::NZ,
            "CAD" => Country::CA,
            "CHF" => Country::CH,
            _ => Country::Other,
        }
    }

    /// 数字 ID → 国家（补充映射）。
    ///
    /// ⚠️ UNVERIFIED：社区文档确认的只有 `5=美国`；其余 ID 待开发环境核对，
    /// 在此之前一律返回 `None`（调用方回退为 [`Country::Other`]）。
    pub fn from_numeric_id(id: u32) -> Option<Self> {
        match id {
            5 => Some(Country::US), // 社区文档确认
            _ => None,              // UNVERIFIED：待开发环境从页面核对
        }
    }

    /// 国家名（中/英文）→ 国家。用于 Backup JSON 源的 `country` 字段。
    ///
    /// 匹配前会转小写 + 去首尾空白。未知 → [`Country::Other`]。
    pub fn from_name(name: &str) -> Self {
        let n = name.trim().to_lowercase();
        // 英文名
        if n.contains("united states") || n == "usa" || n == "us" {
            return Country::US;
        }
        if n.contains("united kingdom") || n == "uk" {
            return Country::UK;
        }
        if n.contains("euro zone") || n.contains("european union") || n.contains("eurozone") {
            return Country::EU;
        }
        match n.as_str() {
            "china" => Country::CN,
            "japan" => Country::JP,
            "australia" => Country::AU,
            "new zealand" => Country::NZ,
            "canada" => Country::CA,
            "switzerland" => Country::CH,
            // 中文名（Investing 中文站返回）
            "美国" => Country::US,
            "中国" => Country::CN,
            "欧元区" | "欧盟" => Country::EU,
            "英国" => Country::UK,
            "日本" => Country::JP,
            "澳大利亚" => Country::AU,
            "澳洲" => Country::AU,
            "新西兰" => Country::NZ,
            "加拿大" => Country::CA,
            "瑞士" => Country::CH,
            _ => Country::Other,
        }
    }
}

/// 重要性 3 档（Investing：1 低 / 2 中 / 3 高）→ 内部 5 星制。
///
/// 默认 `{1:1, 2:3, 3:5}`。
/// ⚠️ **用一周真实数据校准**：重点看 3 星线附近（Investing 2 档中偏上 / 3 档中偏下）
/// 的事件是否合理，再微调本结构体的字段。可经配置注入，无需改代码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportanceMap {
    /// Investing 1 档（低）→ 星数
    pub low: u8,
    /// Investing 2 档（中）→ 星数
    pub mid: u8,
    /// Investing 3 档（高）→ 星数
    pub high: u8,
}

impl Default for ImportanceMap {
    fn default() -> Self {
        Self {
            low: 1,
            mid: 3,
            high: 5,
        }
    }
}

impl ImportanceMap {
    /// `level` 为 1..=3；越界返回 `None`（调用方应跳过该行）。
    pub fn stars(&self, level: u8) -> Option<u8> {
        match level {
            1 => Some(self.low),
            2 => Some(self.mid),
            3 => Some(self.high),
            _ => None,
        }
    }
}

/// 利多/利空方向：实际值相对预期值，"好" 的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 越高越好（CPI/PPI/GDP/非农/零售/PMI/贸易帐…）
    HigherBetter,
    /// 越低越好（失业率、初请失业金人数…）
    LowerBetter,
    /// 未知 → 中性，不着色、不断言
    Neutral,
}

/// 中英关键词匹配事件标题 → 方向。未知 → [`Direction::Neutral`]。
pub fn direction_for(title: &str) -> Direction {
    let t = title.to_lowercase();
    const LOWER_BETTER: &[&str] = &[
        "失业率",
        "初请失业金",
        "unemployment",
        "jobless",
        "initial claims",
    ];
    const HIGHER_BETTER: &[&str] = &[
        "cpi",
        "ppi",
        "gdp",
        "非农",
        "nonfarm",
        "payroll",
        "零售",
        "retail",
        "pmi",
        "贸易帐",
        "trade balance",
    ];
    if LOWER_BETTER.iter().any(|k| t.contains(k)) {
        return Direction::LowerBetter;
    }
    if HIGHER_BETTER.iter().any(|k| t.contains(k)) {
        return Direction::HigherBetter;
    }
    Direction::Neutral
}

/// 宽容数值解析：去 `%`、逗号、空白，处理 `M/K/B` 后缀（百万/千/十亿）。
///
/// - `"-1.0%"` → `-1.0`
/// - `"2,349M"` → `2349000.0`
/// - `"-775M"` → `-775000000.0`
/// - `""` / `"—"` / 无法解析 → `None`
pub fn parse_number(s: &str) -> Option<f64> {
    let mut s = s.trim();
    if s.is_empty() || s == "—" || s == "-" || s == "–" {
        return None;
    }
    let mut mult = 1.0f64;
    if let Some(stripped) = s.strip_suffix('%') {
        s = stripped.trim();
    }
    // M/K/B 后缀（ASCII 单字节，切片安全）
    if let Some(last) = s.chars().last() {
        match last.to_ascii_uppercase() {
            'M' => {
                mult = 1e6;
                s = &s[..s.len() - 1];
            }
            'K' => {
                mult = 1e3;
                s = &s[..s.len() - 1];
            }
            'B' => {
                mult = 1e9;
                s = &s[..s.len() - 1];
            }
            _ => {}
        }
    }
    let cleaned: String = s
        .chars()
        .filter(|c| *c != ',' && !c.is_whitespace())
        .collect();
    cleaned.parse::<f64>().ok().map(|v| v * mult)
}

/// 利多/利空判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentiment {
    /// 利多（实际好于预期）
    Bullish,
    /// 利空（实际差于预期）
    Bearish,
    /// 中性：方向未知，或任一值解析失败 → 不断言
    Neutral,
}

/// 判定利多/利空：**只有 actual 与 forecast 都解析成功才判定**，否则 [`Sentiment::Neutral`]。
pub fn sentiment(actual: Option<&str>, forecast: Option<&str>, direction: Direction) -> Sentiment {
    let (a, f) = match (
        actual.and_then(parse_number),
        forecast.and_then(parse_number),
    ) {
        (Some(a), Some(f)) => (a, f),
        _ => return Sentiment::Neutral,
    };
    match direction {
        Direction::Neutral => Sentiment::Neutral,
        Direction::HigherBetter => {
            if a > f {
                Sentiment::Bullish
            } else if a < f {
                Sentiment::Bearish
            } else {
                Sentiment::Neutral
            }
        }
        Direction::LowerBetter => {
            // 越低越好：实际 < 预期 = 利多
            if a < f {
                Sentiment::Bullish
            } else if a > f {
                Sentiment::Bearish
            } else {
                Sentiment::Neutral
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_mapping_priority() {
        assert_eq!(Country::from_currency("USD"), Country::US);
        assert_eq!(Country::from_currency("CNY"), Country::CN);
        assert_eq!(Country::from_currency("CNH"), Country::CN);
        assert_eq!(Country::from_currency("EUR"), Country::EU);
        assert_eq!(Country::from_currency("GBP"), Country::UK);
        assert_eq!(Country::from_currency("JPY"), Country::JP);
        assert_eq!(Country::from_currency("AUD"), Country::AU);
        assert_eq!(Country::from_currency("NZD"), Country::NZ);
        assert_eq!(Country::from_currency("CAD"), Country::CA);
        assert_eq!(Country::from_currency("CHF"), Country::CH);
        assert_eq!(Country::from_currency("usd"), Country::US); // 大小写宽容
        assert_eq!(Country::from_currency("ZAR"), Country::Other); // 未知 → Other
        assert_eq!(Country::from_currency(""), Country::Other);
    }

    #[test]
    fn numeric_id_only_verified_five() {
        assert_eq!(Country::from_numeric_id(5), Some(Country::US));
        // 其余 UNVERIFIED，一律 None
        assert_eq!(Country::from_numeric_id(6), None);
        assert_eq!(Country::from_numeric_id(37), None);
        assert_eq!(Country::from_numeric_id(110), None);
    }

    #[test]
    fn name_mapping_cn_en() {
        assert_eq!(Country::from_name("United States"), Country::US);
        assert_eq!(Country::from_name("美国"), Country::US);
        assert_eq!(Country::from_name("China"), Country::CN);
        assert_eq!(Country::from_name("中国"), Country::CN);
        assert_eq!(Country::from_name("Euro Zone"), Country::EU);
        assert_eq!(Country::from_name("欧元区"), Country::EU);
        assert_eq!(Country::from_name("United Kingdom"), Country::UK);
        assert_eq!(Country::from_name("英国"), Country::UK);
        assert_eq!(Country::from_name("Japan"), Country::JP);
        assert_eq!(Country::from_name("日本"), Country::JP);
        assert_eq!(Country::from_name("Australia"), Country::AU);
        assert_eq!(Country::from_name("New Zealand"), Country::NZ);
        assert_eq!(Country::from_name("新西兰"), Country::NZ);
        assert_eq!(Country::from_name("Canada"), Country::CA);
        assert_eq!(Country::from_name("Switzerland"), Country::CH);
        assert_eq!(Country::from_name("南非"), Country::Other);
        assert_eq!(Country::from_name("South Africa"), Country::Other);
    }

    #[test]
    fn importance_default_mapping() {
        let m = ImportanceMap::default();
        assert_eq!(m.stars(1), Some(1));
        assert_eq!(m.stars(2), Some(3));
        assert_eq!(m.stars(3), Some(5));
        assert_eq!(m.stars(0), None);
        assert_eq!(m.stars(4), None);
    }

    #[test]
    fn importance_configurable() {
        let m = ImportanceMap {
            low: 2,
            mid: 3,
            high: 4,
        };
        assert_eq!(m.stars(1), Some(2));
        assert_eq!(m.stars(2), Some(3));
        assert_eq!(m.stars(3), Some(4));
    }

    #[test]
    fn direction_keywords() {
        assert_eq!(direction_for("美国失业率"), Direction::LowerBetter);
        assert_eq!(direction_for("美国初请失业金人数"), Direction::LowerBetter);
        assert_eq!(
            direction_for("US Unemployment Rate"),
            Direction::LowerBetter
        );
        assert_eq!(
            direction_for("Initial Jobless Claims"),
            Direction::LowerBetter
        );
        assert_eq!(direction_for("美国CPI年率"), Direction::HigherBetter);
        assert_eq!(direction_for("美国PPI月率"), Direction::HigherBetter);
        assert_eq!(direction_for("中国季度GDP年率"), Direction::HigherBetter);
        assert_eq!(direction_for("美国非农就业人数"), Direction::HigherBetter);
        assert_eq!(direction_for("Nonfarm Payrolls"), Direction::HigherBetter);
        assert_eq!(direction_for("美国零售销售月率"), Direction::HigherBetter);
        assert_eq!(direction_for("中国官方制造业PMI"), Direction::HigherBetter);
        assert_eq!(direction_for("新西兰贸易帐"), Direction::HigherBetter);
        assert_eq!(direction_for("Trade Balance"), Direction::HigherBetter);
        // 未知 → 中性
        assert_eq!(
            direction_for("英国Rightmove平均房屋要价指数月率"),
            Direction::Neutral
        );
        assert_eq!(direction_for("日本央行利率决议"), Direction::Neutral);
    }

    #[test]
    fn parse_number_cases() {
        assert_eq!(parse_number("-1.0%"), Some(-1.0));
        assert_eq!(parse_number("5.2%"), Some(5.2));
        assert_eq!(parse_number("2,349M"), Some(2_349_000_000.0));
        assert_eq!(parse_number("-775M"), Some(-775_000_000.0));
        assert_eq!(parse_number("-1,349M"), Some(-1_349_000_000.0));
        assert_eq!(parse_number("210K"), Some(210_000.0));
        assert_eq!(parse_number("1.41B"), Some(1_410_000_000.0));
        assert_eq!(parse_number("0.3"), Some(0.3));
        assert_eq!(parse_number("  2.8%  "), Some(2.8));
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("—"), None);
        assert_eq!(parse_number("-"), None);
        assert_eq!(parse_number("待公布"), None);
    }

    #[test]
    fn sentiment_needs_both_values() {
        // 新西兰贸易帐：实际 -775M > 预期 -1,349M，越高越好 → 利多
        assert_eq!(
            sentiment(Some("-775M"), Some("-1,349M"), Direction::HigherBetter),
            Sentiment::Bullish
        );
        // 失业率：实际 4.1% < 预期 4.2%，越低越好 → 利多
        assert_eq!(
            sentiment(Some("4.1%"), Some("4.2%"), Direction::LowerBetter),
            Sentiment::Bullish
        );
        // 失业率：实际高于预期 → 利空
        assert_eq!(
            sentiment(Some("4.3%"), Some("4.2%"), Direction::LowerBetter),
            Sentiment::Bearish
        );
        // CPI：实际低于预期 → 利空
        assert_eq!(
            sentiment(Some("2.7%"), Some("2.8%"), Direction::HigherBetter),
            Sentiment::Bearish
        );
        // 相等 → 中性
        assert_eq!(
            sentiment(Some("2.8%"), Some("2.8%"), Direction::HigherBetter),
            Sentiment::Neutral
        );
        // 缺任一值 → 中性（不断言）
        assert_eq!(
            sentiment(None, Some("2.8%"), Direction::HigherBetter),
            Sentiment::Neutral
        );
        assert_eq!(
            sentiment(Some("2.7%"), None, Direction::HigherBetter),
            Sentiment::Neutral
        );
        assert_eq!(
            sentiment(Some("待公布"), Some("2.8%"), Direction::HigherBetter),
            Sentiment::Neutral
        );
        // 方向未知 → 中性
        assert_eq!(
            sentiment(Some("1.0%"), Some("0.5%"), Direction::Neutral),
            Sentiment::Neutral
        );
    }

    // ---------------- 数据审计 fixture：classify_kind ----------------

    use crate::model::EventKind;

    #[test]
    fn classify_kind_audit_fixtures() {
        // 有数值 → Data
        assert_eq!(
            classify_kind(
                "Federal Funds Rate",
                Some("4.00%"),
                Some("3.75%"),
                Some("4.00%")
            ),
            EventKind::Data
        );
        // All Day 行但有数值 → Data（禁用时间字段判断）
        assert_eq!(
            classify_kind(
                "German Prelim CPI m/m",
                Some("0.7%"),
                Some("1.1%"),
                Some("0.6%")
            ),
            EventKind::Data
        );
        // 无数值 → Event
        assert_eq!(
            classify_kind("FOMC Member Barkin Speaks", None, None, None),
            EventKind::Event
        );
        assert_eq!(
            classify_kind("FOMC Meeting Minutes", Some(""), None, None),
            EventKind::Event
        );
        // 假期关键词 → Holiday（优先于数值判定）
        assert_eq!(
            classify_kind("Bank Holiday", None, None, None),
            EventKind::Holiday
        );
        assert_eq!(
            classify_kind("Daylight Saving Time Shift", None, None, None),
            EventKind::Holiday
        );
        // 空标题但有值 → Data
        assert_eq!(classify_kind("", Some("5.0%"), None, None), EventKind::Data);
    }

    #[test]
    fn classify_kind_blank_values() {
        // 空值变体均视为空
        for blank in ["", " ", "—", "-", "–"] {
            assert_eq!(
                classify_kind("某某讲话", Some(blank), Some(blank), Some(blank)),
                EventKind::Event
            );
        }
        // 假期关键词中英文、大小写宽容
        assert_eq!(
            classify_kind("bank HOLIDAY", None, None, None),
            EventKind::Holiday
        );
        assert_eq!(
            classify_kind("银行假期", None, None, None),
            EventKind::Holiday
        );
        assert_eq!(
            classify_kind("欧洲夏令时开始", None, None, None),
            EventKind::Holiday
        );
        assert_eq!(
            classify_kind("港股休市", None, None, None),
            EventKind::Holiday
        );
        // 假期标题 + 数值 → 仍是 Holiday（假期优先）
        assert_eq!(
            classify_kind("Bank Holiday", Some("5.0%"), None, None),
            EventKind::Holiday
        );
        // 只有前值有数 → Data
        assert_eq!(
            classify_kind("日本央行利率决议", None, Some("-0.1%"), None),
            EventKind::Data
        );
    }

    #[test]
    fn deviation_cases() {
        // 相等 → Inline
        assert_eq!(
            deviation(Some("5.2%"), Some("5.2%")),
            Some(Deviation::Inline)
        );
        // 150K < 180K → Miss
        assert_eq!(deviation(Some("150K"), Some("180K")), Some(Deviation::Miss));
        // 前缀 < 剥离后相等 → Inline
        assert_eq!(
            deviation(Some("<1.00%"), Some("<1.00%")),
            Some(Deviation::Inline)
        );
        // | 前半比较：3.80 > 3.70 → Beat
        assert_eq!(
            deviation(Some("3.80|1.6"), Some("3.70|1.5")),
            Some(Deviation::Beat)
        );
        // 投票数格式 → None
        assert_eq!(deviation(Some("3-0-6"), Some("3-0-6")), None);
        // 利率区间：原始字符串一致 → Inline（无需解析）
        assert_eq!(
            deviation(Some("3.75%-4.00%"), Some("3.75%-4.00%")),
            Some(Deviation::Inline)
        );
        // 双方都是缺失标记 → None（不是"符合预期"）
        assert_eq!(deviation(Some("—"), Some("—")), None);
        assert_eq!(deviation(Some("-"), Some("-")), None);
        // 任一缺失 → None
        assert_eq!(deviation(None, Some("5.0%")), None);
        assert_eq!(deviation(Some(""), Some("5.0%")), None);
        // 负数：-1.0 < -0.5 → Miss
        assert_eq!(
            deviation(Some("-1.0%"), Some("-0.5%")),
            Some(Deviation::Miss)
        );
        // 浮点容差：极小差值算 Inline
        assert_eq!(
            deviation(Some("5.2000000001%"), Some("5.2%")),
            Some(Deviation::Inline)
        );
        // 无法解析 → None
        assert_eq!(deviation(Some("待公布"), Some("5.0%")), None);
        // 负数不是投票格式
        assert_eq!(
            deviation(Some("-775M"), Some("-1,349M")),
            Some(Deviation::Beat)
        );
    }

    #[test]
    fn deviation_serialization() {
        assert_eq!(
            serde_json::to_string(&Deviation::Beat).unwrap(),
            r#""beat""#
        );
        assert_eq!(
            serde_json::to_string(&Deviation::Miss).unwrap(),
            r#""miss""#
        );
        assert_eq!(
            serde_json::to_string(&Deviation::Inline).unwrap(),
            r#""inline""#
        );
    }

    #[test]
    fn tentative_status_cases() {
        assert_eq!(tentative_status("Tentative").as_deref(), Some("待定"));
        assert_eq!(tentative_status("tentative").as_deref(), Some("待定"));
        assert_eq!(
            tentative_status("Oct 19, 2026 Tentative").as_deref(),
            Some("待定")
        );
        assert_eq!(tentative_status("10:00"), None);
        assert_eq!(tentative_status("全天"), None);
        assert_eq!(tentative_status(""), None);
    }
}

/// 数值为空的判定（分类用）：trim 后为 `""`/`"—"`/`"-"`/`"–"` 视为空。
fn is_blank_value(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t == "—" || t == "-" || t == "–"
}

/// 事件行/数据行/假期行分类（见 [`crate::model::EventKind`]）。
///
/// 判定顺序：
/// 1. 标题含假期/夏令时关键词（大小写不敏感）→ [`EventKind::Holiday`]
/// 2. 任一数值字段（今值/预测值/前值）非空 → [`EventKind::Data`]
/// 3. 否则 → [`EventKind::Event`]
///
/// **必须用数值字段判断，禁用时间字段**："All Day" 行也可能带数值
/// （如 German Prelim CPI）。
pub fn classify_kind(
    title: &str,
    forecast: Option<&str>,
    previous: Option<&str>,
    actual: Option<&str>,
) -> crate::model::EventKind {
    use crate::model::EventKind;
    let t = title.to_lowercase();
    const HOLIDAY_KEYWORDS: &[&str] = &[
        "bank holiday",
        "daylight saving",
        "休市",
        "假期",
        "假日",
        "夏令时",
    ];
    if HOLIDAY_KEYWORDS.iter().any(|k| t.contains(k)) {
        return EventKind::Holiday;
    }
    let has_value = [forecast, previous, actual]
        .into_iter()
        .any(|v| v.map(|s| !is_blank_value(s)).unwrap_or(false));
    if has_value {
        return EventKind::Data;
    }
    EventKind::Event
}

/// 中性预期差判定：**只比较数值大小，不含方向含义**。
///
/// 用途：UI 着色（超预期=绿/低预期=红/符合预期=灰）。方向（利多/利空）
/// 仍由 [`Direction`]/[`direction_for`]/[`sentiment`] 负责（供"美元口径"
/// 可选项用），与本类型无关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Deviation {
    /// 实际值高于预期值（超预期）
    Beat,
    /// 实际值低于预期值（低预期）
    Miss,
    /// 符合预期（数值相等，含极小容差）
    Inline,
}

/// 投票数格式：`^\d+\s*-\s*\d+\s*-\s*\d+$`（如利率决议的 "3-0-6"）。
/// 这种格式没有数值可比性 → [`deviation`] 返回 None。
fn is_vote_format(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    parts.iter().all(|p| {
        let p = p.trim();
        !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())
    })
}

/// 原始时间文本 → 事件行状态。
///
/// 时间文本含 `"tentative"`（大小写不敏感，如 OPEC 会议等时间待定事件）
/// → `Some("待定")`，否则 None。解析器在构造 [`crate::model::CalendarEvent`]
/// 时调用。
pub fn tentative_status(time_text: &str) -> Option<String> {
    if time_text.to_lowercase().contains("tentative") {
        Some("待定".to_string())
    } else {
        None
    }
}
///
/// 归一化后解析两边数值并比较预期差。
///
/// 归一化步骤：
/// 0. 任一为空值（`""`/`"—"`/`"-"`/`"–"`）→ None；
/// 1. 投票数格式（`"3-0-6"`）→ None（无数值可比性；必须先于字符串相等判断，
///    否则 "3-0-6"=="3-0-6" 会被误判为符合预期）；
/// 2. 原始字符串（去首尾空白）一致 → [`Deviation::Inline`]（覆盖利率区间
///    `"3.75%-4.00%"` 等无法解析为单个数值、但显然"符合预期"的情形）；
/// 3. 去首尾空白；
/// 4. 剥离首字符 `<`/`>`/`≤`/`≥`（如 `"<1.00%"`）；
/// 5. 含 `|` 取 `|` 前半（如 `"3.80|1.6"` → `"3.80"`）；
/// 6. 用 [`parse_number`] 解析，任一失败 → None；
/// 7. 相等容差 `eps = 1e-9 * max(1, |a|, |b|)`：差值 ≤ eps → [`Deviation::Inline`]，
///    `a > b` → [`Deviation::Beat`]，否则 [`Deviation::Miss`]。
pub fn deviation(actual: Option<&str>, forecast: Option<&str>) -> Option<Deviation> {
    let (a, f) = (actual?, forecast?);
    let (a, f) = (a.trim(), f.trim());
    if is_blank_value(a) || is_blank_value(f) {
        return None;
    }
    // 投票数格式（"3-0-6"）无数值可比性，先排除——否则会被下面的字符串相等误判
    if is_vote_format(a) || is_vote_format(f) {
        return None;
    }
    // 原始字符串一致 → 符合预期（覆盖利率区间 "3.75%-4.00%" 等无法解析为
    // 单个数值、但显然"符合预期"的情形）
    if a == f {
        return Some(Deviation::Inline);
    }
    let (a, f) = (norm_value(a), norm_value(f));
    let (a, b) = (parse_number(a)?, parse_number(f)?);
    let eps = 1e-9 * 1.0f64.max(a.abs()).max(b.abs());
    if (a - b).abs() <= eps {
        Some(Deviation::Inline)
    } else if a > b {
        Some(Deviation::Beat)
    } else {
        Some(Deviation::Miss)
    }
}

/// [`deviation`] 的归一化：去首尾空白 → 剥离首字符 `<`/`>`/`≤`/`≥`
/// → 含 `|` 取前半。
fn norm_value(s: &str) -> &str {
    let mut s = s.trim();
    if let Some(c) = s.chars().next() {
        if matches!(c, '<' | '>' | '≤' | '≥') {
            s = &s[c.len_utf8()..];
        }
    }
    if let Some(idx) = s.find('|') {
        s = &s[..idx];
    }
    s.trim()
}
