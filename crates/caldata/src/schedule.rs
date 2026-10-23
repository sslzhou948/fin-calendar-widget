//! 刷新调度纯函数：下次全量刷新时间、对账判定、事件驱动唤醒时间表。
//!
//! 全部纯函数 + 单测，无 IO、无网络。时区统一 [`TZ`] = Asia/Shanghai。
//! 美东夏冬令时由 `chrono-tz` 数据库自动处理（见时区单测）。

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Weekday};
use chrono_tz::Tz;

use crate::model::CalendarEvent;

/// 内部统一时区：北京时间。
pub const TZ: Tz = chrono_tz::Asia::Shanghai;

/// 下一个周日 00:00（Asia/Shanghai），严格晚于 `now`。
///
/// - 周日当天 00:00 之前调用 → 返回今天 00:00（已过则顺延下周，见下）。
/// - 触发点已过（含恰好等于）→ 顺延 7 天。
pub fn next_sunday_midnight(now: DateTime<Tz>) -> DateTime<Tz> {
    let midnight = TZ
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .expect("当日 00:00 应为有效本地时间");
    // 周一=0 … 周日=6；距下个周日的天数：周日→0，周一→6
    let days_to_sunday = (6 - now.weekday().num_days_from_monday() as i64).rem_euclid(7);
    let candidate = midnight + Duration::days(days_to_sunday);
    if candidate <= now {
        candidate + Duration::weeks(1)
    } else {
        candidate
    }
}

/// 本周周一（Asia/Shanghai 自然日）。
pub fn week_monday(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

/// 对账判定：本地缓存覆盖的 ISO 周 ≠ 当前 ISO 周 → 需要全量刷新。
///
/// `covered` 为缓存中记录的 `(iso_year, iso_week)`。
/// 覆盖场景：周日 00:00 关机错过 → 下次开机 `covered` 落后 → 返回 true 立即补拉。
pub fn needs_refresh(covered: (i32, u32), now: DateTime<Tz>) -> bool {
    let iso = now.date_naive().iso_week();
    (iso.year(), iso.week()) != covered
}

/// 事件驱动唤醒时间表：`[(唤醒时间, 事件 id)]`。
///
/// 唤醒时间 = `release_at + 2min`（给接口留出数据更新时间）；
/// 只取唤醒时间晚于 `now` 的未来事件，按时间升序。
pub fn build_wake_list(events: &[CalendarEvent], now: DateTime<Tz>) -> Vec<(DateTime<Tz>, String)> {
    let mut list: Vec<(DateTime<Tz>, String)> = events
        .iter()
        .map(|e| (e.release_at + Duration::minutes(2), e.id.clone()))
        .filter(|(wake, _)| *wake > now)
        .collect();
    list.sort_by_key(|(wake, _)| *wake);
    list
}

/// 扫尾重试的周（纯函数）。
///
/// 注意（2026-10-23）：生产调度已改用 scheduler 内的事件驱动 sweep
/// （按 10min–4h 窗口 + 数据源分流），本函数无生产调用方，仅保留供
/// 单元测试与未来复用，勿与 scheduler::reconcile 的同名局部变量混淆。
///
/// 选中条件（需同时满足）：
/// - `kind == Data`（只有数据行才有 actual 可抓）；
/// - `release_at <= now - 2min`（已过发布时间，给接口留出更新时间）；
/// - `actual.is_none()`（还没抓到）；
/// - `release_at > now - 48h`（48 小时前的陈旧事件不再扫，等周日全量刷新）。
///
/// 返回去重后的周一日期（`week_monday`），调用方按周拉取。
pub fn stale_weeks(events: &[CalendarEvent], now: DateTime<Tz>) -> Vec<NaiveDate> {
    use crate::model::EventKind;
    let cutoff = now - Duration::minutes(2);
    let floor = now - Duration::hours(48);
    let mut weeks = Vec::new();
    for e in events.iter().filter(|e| {
        e.kind == EventKind::Data
            && e.release_at <= cutoff
            && e.actual.is_none()
            && e.release_at > floor
    }) {
        let ws = week_monday(e.release_at.date_naive());
        if !weeks.contains(&ws) {
            weeks.push(ws);
        }
    }
    weeks
}

/// 星期几（中文），渲染用。
pub fn weekday_cn(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "一",
        Weekday::Tue => "二",
        Weekday::Wed => "三",
        Weekday::Thu => "四",
        Weekday::Fri => "五",
        Weekday::Sat => "六",
        Weekday::Sun => "日",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Country, EventKind, Source};

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Tz> {
        TZ.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    fn event(id: &str, release_at: DateTime<Tz>) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            release_at,
            country: Country::US,
            title: "t".to_string(),
            importance: 5,
            forecast: None,
            previous: None,
            actual: None,
            kind: EventKind::Event,
            status: None,
            source: Source::Primary,
        }
    }

    #[test]
    fn next_sunday_from_sunday_afternoon() {
        // 2026-10-18 是周日；17:49 调用 → 下个周日 2026-10-25 00:00
        let now = dt(2026, 10, 18, 17, 49);
        assert_eq!(now.weekday(), Weekday::Sun);
        let next = next_sunday_midnight(now);
        assert_eq!(
            next.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-25 00:00"
        );
    }

    #[test]
    fn next_sunday_from_saturday() {
        // 周六 10:00 → 明天（周日）00:00
        let next = next_sunday_midnight(dt(2026, 10, 17, 10, 0));
        assert_eq!(
            next.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-18 00:00"
        );
    }

    #[test]
    fn next_sunday_from_monday() {
        // 周一 → 本周日 00:00（6 天后）
        let next = next_sunday_midnight(dt(2026, 10, 19, 9, 0));
        assert_eq!(
            next.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-25 00:00"
        );
    }

    #[test]
    fn next_sunday_from_wednesday() {
        let next = next_sunday_midnight(dt(2026, 10, 21, 23, 59));
        assert_eq!(
            next.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-25 00:00"
        );
    }

    #[test]
    fn next_sunday_exactly_midnight_rolls_over() {
        // 恰好是周日 00:00 → 顺延一周（避免立即重复触发）
        let next = next_sunday_midnight(dt(2026, 10, 18, 0, 0));
        assert_eq!(
            next.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-25 00:00"
        );
    }

    #[test]
    fn needs_refresh_cases() {
        // 2026-10-18（周日）所在 ISO 周
        let now = dt(2026, 10, 18, 17, 49);
        let iso = now.date_naive().iso_week();
        assert!(!needs_refresh((iso.year(), iso.week()), now));
        // 缓存是上周 → 需要刷新
        assert!(needs_refresh((iso.year(), iso.week() - 1), now));
        // 跨年边界：2025 年最后一周的缓存
        assert!(needs_refresh((2025, 52), now));
    }

    #[test]
    fn build_wake_list_future_only_sorted() {
        let now = dt(2026, 10, 19, 10, 0);
        let events = vec![
            event("past", dt(2026, 10, 19, 9, 0)), // 唤醒 09:02 ≤ now → 排除
            event("future-b", dt(2026, 10, 19, 20, 30)), // 唤醒 20:32
            event("future-a", dt(2026, 10, 19, 10, 30)), // 唤醒 10:32
            event("edge", dt(2026, 10, 19, 9, 58)), // 唤醒 10:00 == now → 排除（严格大于）
        ];
        let list = build_wake_list(&events, now);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1, "future-a");
        assert_eq!(list[0].0.format("%H:%M").to_string(), "10:32");
        assert_eq!(list[1].1, "future-b");
        assert_eq!(list[1].0.format("%H:%M").to_string(), "20:32");
    }

    #[test]
    fn week_monday_cases() {
        // 2026-10-18 周日 → 周一 2026-10-12
        let sun = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        assert_eq!(
            week_monday(sun),
            NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
        );
        // 周一本身 → 当天
        let mon = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        assert_eq!(week_monday(mon), mon);
    }

    /// 时区单测：美国事件 2026-11-02 08:30 America/New_York（冬令时 EST）
    /// == 北京时间 2026-11-02 21:30。
    ///
    /// 2026 年美东冬令时切换日为 11 月 1 日（周日），11 月 2 日已是 EST（UTC-5）。
    #[test]
    fn us_est_to_beijing_conversion() {
        use chrono_tz::America::New_York;
        let ny_time = New_York
            .with_ymd_and_hms(2026, 11, 2, 8, 30, 0)
            .single()
            .expect("2026-11-02 08:30 在纽约应为唯一有效时间（EST）");
        let bj = ny_time.with_timezone(&TZ);
        assert_eq!(bj.format("%Y-%m-%d %H:%M").to_string(), "2026-11-02 21:30");
        // 反查：确实是 EST（UTC-5）
        assert_eq!(ny_time.format("%:z").to_string(), "-05:00");
    }

    /// 夏令时对照：2026-07-01 08:30 EDT（UTC-4）== 北京时间 20:30。
    #[test]
    fn us_edt_to_beijing_conversion() {
        use chrono_tz::America::New_York;
        let ny_time = New_York
            .with_ymd_and_hms(2026, 7, 1, 8, 30, 0)
            .single()
            .unwrap();
        let bj = ny_time.with_timezone(&TZ);
        assert_eq!(bj.format("%Y-%m-%d %H:%M").to_string(), "2026-07-01 20:30");
        assert_eq!(ny_time.format("%:z").to_string(), "-04:00");
    }

    fn data_event(id: &str, release_at: DateTime<Tz>, actual: Option<&str>) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            release_at,
            country: Country::US,
            title: "t".to_string(),
            importance: 5,
            forecast: None,
            previous: None,
            actual: actual.map(|s| s.to_string()),
            kind: EventKind::Data,
            status: None,
            source: Source::Primary,
        }
    }

    /// stale_weeks：只选中"已过发布时间 2 分钟、有 actual 缺口"的数据行
    #[test]
    fn stale_weeks_picks_only_missing_actuals() {
        // now = 2026-10-21（周三）12:00
        let now = dt(2026, 10, 21, 12, 0);
        let events = vec![
            // 命中：昨天 10:00 发布，actual 为空
            data_event("a", dt(2026, 10, 20, 10, 0), None),
            // 排除：已有 actual
            data_event("b", dt(2026, 10, 20, 11, 0), Some("1.2")),
            // 排除：1 分钟前发布（未过 2 分钟 cutoff）
            data_event("c", dt(2026, 10, 21, 11, 59), None),
            // 排除：3 天前（48h 外）
            data_event("d", dt(2026, 10, 18, 10, 0), None),
            // 排除：事件行（无 actual 概念）
            event("e", dt(2026, 10, 20, 9, 0)),
        ];
        let weeks = stale_weeks(&events, now);
        // a 落在 2026-10-19（周一）那周
        assert_eq!(weeks, vec![NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()]);
    }

    /// stale_weeks：多周去重（now 取周二，使 48h 窗口跨过周一）
    #[test]
    fn stale_weeks_dedups_weeks() {
        let now = dt(2026, 10, 20, 12, 0); // 周二
        let events = vec![
            data_event("a", dt(2026, 10, 19, 10, 0), None), // 本周一
            data_event("b", dt(2026, 10, 19, 15, 0), None), // 本周一（同周去重）
            data_event("c", dt(2026, 10, 18, 14, 0), None), // 上周日（48h 内，另一周）
        ];
        let mut weeks = stale_weeks(&events, now);
        weeks.sort();
        assert_eq!(
            weeks,
            vec![
                NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
            ]
        );
    }

    /// stale_weeks：无缺口时返回空（扫尾任务直接跳过，不发请求）
    #[test]
    fn stale_weeks_empty_when_nothing_missing() {
        let now = dt(2026, 10, 21, 12, 0);
        let events = vec![
            data_event("a", dt(2026, 10, 20, 10, 0), Some("1.2")),
            data_event("b", dt(2026, 10, 22, 10, 0), None), // 未来事件
        ];
        assert!(stale_weeks(&events, now).is_empty());
    }
}
