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
}
