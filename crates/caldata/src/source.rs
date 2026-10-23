//! DataSource trait + 错误类型 + 自动降级逻辑。
//!
//! 降级策略（2026-10-23 起，v0.2 精简）：金十 WebSocket（主） →
//! Faireconomy（备，成功后 FF 实际值回填）；都失败 → 保留本地缓存。
//! 配置 `SourceMode` 可强制只走主源（设置页"数据源选择"用）。
//! （Investing 两路 2026-10-19 起被 Cloudflare 全拦，v0.2 已删除，清单 #8。）

use async_trait::async_trait;
use chrono::NaiveDate;
use std::fmt;
use std::sync::OnceLock;

use crate::model::{CalendarEvent, Source};

/// 浏览器 UA（两路请求共用，仿真真实浏览器，降低反爬概率）。
pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 全局复用 Client（避免每次回填新建连接池，2026-10-19 QA major）。
static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest Client 构建失败")
    })
}

/// 数据源错误。携带可读信息，供设置页"接口测试"展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// 网络/HTTP 错误（超时、DNS、非 403 的 HTTP 错误…）
    Network(String),
    /// 响应解析失败（HTML/JSON 结构变化）
    Parse(String),
    /// 疑似反爬拦截（HTTP 403 / Cloudflare 挑战页）
    AuthBlocked(String),
    /// 解析成功但事件数为 0（可能被静默限流或参数错误）
    Empty(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::Network(msg) => write!(f, "网络错误: {msg}"),
            SourceError::Parse(msg) => write!(f, "解析失败: {msg}"),
            SourceError::AuthBlocked(msg) => write!(f, "疑似被反爬拦截: {msg}"),
            SourceError::Empty(msg) => write!(f, "返回为空: {msg}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// 可插拔数据源。
#[async_trait]
pub trait DataSource: Send + Sync {
    fn name(&self) -> &'static str;
    /// 拉取 `week_start` 所在周（周一为周首）的全量事件。
    async fn fetch_week(&self, week_start: NaiveDate) -> Result<Vec<CalendarEvent>, SourceError>;
}

/// 数据源选择模式（设置页可配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    /// 自动：主源失败 → 备源 → 缓存
    #[default]
    Auto,
    /// 只走主源，失败直接返回错误
    PrimaryOnly,
}

/// 一次带降级的拉取结果。
#[derive(Debug)]
pub struct FetchOutcome {
    /// 事件列表（可能来自 Primary / Backup / 本地缓存）。
    pub events: Vec<CalendarEvent>,
    /// 实际命中的数据源；`None` 表示兜底用了缓存。
    pub used: Option<Source>,
    /// 是否来自本地缓存（全链都失败时的兜底）。
    pub from_cache: bool,
    /// 按尝试顺序记录的错误（供"接口测试"与日志展示）。
    pub errors: Vec<(String, SourceError)>,
}

/// Faireconomy 命中时抓 ForexFactory 实际值回填（2026-10-19）。
/// 失败静默（空 map），不影响主流程。Client 全局复用。
async fn fill_ff_actuals(events: &mut Vec<CalendarEvent>) {
    if events
        .first()
        .is_some_and(|e| e.source == Source::Faireconomy)
    {
        let actual_map = crate::forexfactory_actuals::fetch_actual_map(shared_client()).await;
        let filled = crate::forexfactory_actuals::fill_actuals(events, &actual_map);
        if filled > 0 {
            eprintln!("[ff-actuals] 回填 {filled} 条实际值");
        }
    }
}

/// 自动降级拉取：金十（主） → Faireconomy（备） → 本地缓存。
///
/// - `PrimaryOnly`：只尝试主源。
/// - 全部失败：若有 `cache` 则返回缓存（`from_cache=true`），否则空列表 + 错误。
/// - `used` 取获胜事件自带的来源标记（事件为空时用该路的来源常量）。
pub async fn fetch_week_with_fallback<P, S>(
    primary: &P,
    secondary: &S,
    mode: SourceMode,
    week_start: NaiveDate,
    cache: Option<Vec<CalendarEvent>>,
) -> FetchOutcome
where
    P: DataSource,
    S: DataSource,
{
    fetch_week_chain(primary, secondary, mode, week_start, cache, true).await
}

/// 降级链本体。`fill_ff=true` 时 Faireconomy 命中后抓 FF 页面回填 actual
/// （生产路径）；单元测试传 false 保持 hermetic（不发真实网络请求）。
async fn fetch_week_chain<P, S>(
    primary: &P,
    secondary: &S,
    mode: SourceMode,
    week_start: NaiveDate,
    cache: Option<Vec<CalendarEvent>>,
    fill_ff: bool,
) -> FetchOutcome
where
    P: DataSource,
    S: DataSource,
{
    let mut errors: Vec<(String, SourceError)> = Vec::new();

    match primary.fetch_week(week_start).await {
        Ok(mut events) => {
            if fill_ff {
                fill_ff_actuals(&mut events).await;
            }
            let used = events.first().map(|e| e.source).unwrap_or(Source::Jin10);
            return FetchOutcome {
                events,
                used: Some(used),
                from_cache: false,
                errors,
            };
        }
        Err(e) => errors.push((primary.name().to_string(), e)),
    }
    if matches!(mode, SourceMode::Auto) {
        match secondary.fetch_week(week_start).await {
            Ok(mut events) => {
                if fill_ff {
                    fill_ff_actuals(&mut events).await;
                }
                let used = events
                    .first()
                    .map(|e| e.source)
                    .unwrap_or(Source::Faireconomy);
                return FetchOutcome {
                    events,
                    used: Some(used),
                    from_cache: false,
                    errors,
                };
            }
            Err(e) => errors.push((secondary.name().to_string(), e)),
        }
    }

    // 全部失败（或被模式禁用）：保留缓存
    FetchOutcome {
        events: cache.unwrap_or_default(),
        used: None,
        from_cache: true,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSource {
        name: &'static str,
        result: Result<Vec<CalendarEvent>, SourceError>,
    }

    #[async_trait]
    impl DataSource for MockSource {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn fetch_week(
            &self,
            _week_start: NaiveDate,
        ) -> Result<Vec<CalendarEvent>, SourceError> {
            // 纯内存 mock，不发任何网络请求
            match &self.result {
                Ok(events) => Ok(events.clone()),
                Err(e) => Err(e.clone()),
            }
        }
    }

    fn sample_event(source: Source) -> CalendarEvent {
        use crate::schedule::TZ;
        use chrono::TimeZone;
        CalendarEvent {
            id: "test-1".to_string(),
            release_at: TZ.with_ymd_and_hms(2026, 10, 19, 10, 0, 0).unwrap(),
            country: crate::model::Country::CN,
            title: "测试事件".to_string(),
            importance: 3,
            forecast: None,
            previous: None,
            actual: None,
            kind: crate::model::EventKind::Event,
            status: None,
            source,
        }
    }

    fn ok(name: &'static str, source: Source) -> MockSource {
        MockSource {
            name,
            result: Ok(vec![sample_event(source)]),
        }
    }

    fn err(name: &'static str, msg: &str) -> MockSource {
        MockSource {
            name,
            result: Err(SourceError::Network(msg.into())),
        }
    }

    fn monday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()
    }

    #[tokio::test]
    async fn primary_success_no_fallback() {
        let p = ok("jin10", Source::Jin10);
        let s = err("faireconomy", "should not be called");
        let out = fetch_week_chain(&p, &s, SourceMode::Auto, monday(), None, false).await;
        assert_eq!(out.used, Some(Source::Jin10));
        assert!(!out.from_cache);
        assert_eq!(out.events.len(), 1);
        assert!(out.errors.is_empty());
    }

    #[tokio::test]
    async fn primary_fails_then_secondary() {
        let p = err("jin10", "ws down");
        let s = ok("faireconomy", Source::Faireconomy);
        let out = fetch_week_chain(&p, &s, SourceMode::Auto, monday(), None, false).await;
        // used 以获胜事件自带来源为准；测试走 fetch_week_chain(fill=false)
        // 不触发 FF 回填网络请求
        assert_eq!(out.used, Some(Source::Faireconomy));
        assert!(!out.from_cache);
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].0, "jin10");
    }

    #[tokio::test]
    async fn all_fail_keeps_cache() {
        let p = err("jin10", "timeout");
        let s = err("faireconomy", "429");
        let out = fetch_week_chain(
            &p,
            &s,
            SourceMode::Auto,
            monday(),
            Some(vec![sample_event(Source::Jin10)]),
            false,
        )
        .await;
        assert_eq!(out.used, None);
        assert!(out.from_cache);
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.errors.len(), 2);
    }

    #[tokio::test]
    async fn all_fail_no_cache_empty() {
        let p = err("jin10", "timeout");
        let s = err("faireconomy", "dns");
        let out = fetch_week_chain(&p, &s, SourceMode::Auto, monday(), None, false).await;
        assert_eq!(out.used, None);
        assert!(out.from_cache);
        assert!(out.events.is_empty());
        assert_eq!(out.errors.len(), 2);
    }

    #[tokio::test]
    async fn primary_only_mode_skips_others() {
        let p = err("jin10", "down");
        let s = ok("faireconomy", Source::Faireconomy);
        let out = fetch_week_chain(&p, &s, SourceMode::PrimaryOnly, monday(), None, false)
            .await;
        assert_eq!(out.used, None);
        assert!(out.from_cache);
        assert!(out.events.is_empty());
        assert_eq!(out.errors.len(), 1);
    }

}
