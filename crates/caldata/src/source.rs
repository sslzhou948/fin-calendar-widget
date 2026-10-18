//! DataSource trait + 错误类型 + 自动降级逻辑。
//!
//! 降级策略：Primary 失败 → Backup；都失败 → 保留本地缓存并记录错误。
//! 配置 `SourceMode` 可强制走某一路（设置页"数据源选择"用）。

use async_trait::async_trait;
use chrono::NaiveDate;
use std::fmt;

use crate::model::{CalendarEvent, Source};

/// 浏览器 UA（两路请求共用，仿真真实浏览器，降低反爬概率）。
pub(crate) const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

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
    /// 自动：Primary 失败 → Backup → 缓存
    #[default]
    Auto,
    /// 只走 Primary，失败直接返回错误
    PrimaryOnly,
    /// 只走 Backup，失败直接返回错误
    BackupOnly,
}

/// 一次带降级的拉取结果。
#[derive(Debug)]
pub struct FetchOutcome {
    /// 事件列表（可能来自 Primary / Backup / 本地缓存）。
    pub events: Vec<CalendarEvent>,
    /// 实际命中的数据源；`None` 表示兜底用了缓存。
    pub used: Option<Source>,
    /// 是否来自本地缓存（两路都失败时的兜底）。
    pub from_cache: bool,
    /// 按尝试顺序记录的错误（供"接口测试"与日志展示）。
    pub errors: Vec<(String, SourceError)>,
}

/// 自动降级拉取：Primary → Backup → 本地缓存。
///
/// - `PrimaryOnly` / `BackupOnly`：只尝试指定的一路。
/// - 两路都失败：若有 `cache` 则返回缓存（`from_cache=true`），否则返回空列表 + 错误。
pub async fn fetch_week_with_fallback<P, B>(
    primary: &P,
    backup: &B,
    mode: SourceMode,
    week_start: NaiveDate,
    cache: Option<Vec<CalendarEvent>>,
) -> FetchOutcome
where
    P: DataSource,
    B: DataSource,
{
    let mut errors: Vec<(String, SourceError)> = Vec::new();

    let try_primary = matches!(mode, SourceMode::Auto | SourceMode::PrimaryOnly);
    let try_backup = matches!(mode, SourceMode::Auto | SourceMode::BackupOnly);

    if try_primary {
        match primary.fetch_week(week_start).await {
            Ok(events) => {
                return FetchOutcome {
                    events,
                    used: Some(Source::Primary),
                    from_cache: false,
                    errors,
                };
            }
            Err(e) => errors.push((primary.name().to_string(), e)),
        }
    }
    if try_backup {
        match backup.fetch_week(week_start).await {
            Ok(events) => {
                return FetchOutcome {
                    events,
                    used: Some(Source::Backup),
                    from_cache: false,
                    errors,
                };
            }
            Err(e) => errors.push((backup.name().to_string(), e)),
        }
    }

    // 两路都失败（或被模式禁用）：保留缓存
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

    fn sample_event() -> CalendarEvent {
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
            source: Source::Primary,
        }
    }

    fn monday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()
    }

    #[tokio::test]
    async fn primary_success_no_fallback() {
        let p = MockSource {
            name: "primary",
            result: Ok(vec![sample_event()]),
        };
        let b = MockSource {
            name: "backup",
            result: Err(SourceError::Network("should not be called".into())),
        };
        let out = fetch_week_with_fallback(&p, &b, SourceMode::Auto, monday(), None).await;
        assert_eq!(out.used, Some(Source::Primary));
        assert!(!out.from_cache);
        assert_eq!(out.events.len(), 1);
        assert!(out.errors.is_empty());
    }

    #[tokio::test]
    async fn primary_fails_then_backup() {
        let p = MockSource {
            name: "primary",
            result: Err(SourceError::AuthBlocked("403".into())),
        };
        let b = MockSource {
            name: "backup",
            result: Ok(vec![sample_event()]),
        };
        let out = fetch_week_with_fallback(&p, &b, SourceMode::Auto, monday(), None).await;
        assert_eq!(out.used, Some(Source::Backup));
        assert!(!out.from_cache);
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].0, "primary");
    }

    #[tokio::test]
    async fn both_fail_keeps_cache() {
        let p = MockSource {
            name: "primary",
            result: Err(SourceError::Network("timeout".into())),
        };
        let b = MockSource {
            name: "backup",
            result: Err(SourceError::Parse("bad json".into())),
        };
        let out = fetch_week_with_fallback(
            &p,
            &b,
            SourceMode::Auto,
            monday(),
            Some(vec![sample_event()]),
        )
        .await;
        assert_eq!(out.used, None);
        assert!(out.from_cache);
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.errors.len(), 2);
    }

    #[tokio::test]
    async fn both_fail_no_cache_empty() {
        let p = MockSource {
            name: "primary",
            result: Err(SourceError::Network("timeout".into())),
        };
        let b = MockSource {
            name: "backup",
            result: Err(SourceError::Network("dns".into())),
        };
        let out = fetch_week_with_fallback(&p, &b, SourceMode::Auto, monday(), None).await;
        assert_eq!(out.used, None);
        assert!(out.from_cache);
        assert!(out.events.is_empty());
        assert_eq!(out.errors.len(), 2);
    }

    #[tokio::test]
    async fn primary_only_mode_skips_backup() {
        let p = MockSource {
            name: "primary",
            result: Err(SourceError::Network("down".into())),
        };
        let b = MockSource {
            name: "backup",
            result: Ok(vec![sample_event()]), // 不应被调用
        };
        let out = fetch_week_with_fallback(&p, &b, SourceMode::PrimaryOnly, monday(), None).await;
        assert_eq!(out.used, None);
        assert!(out.from_cache);
        assert!(out.events.is_empty());
        assert_eq!(out.errors.len(), 1); // 只有 primary 的错误
    }
}
