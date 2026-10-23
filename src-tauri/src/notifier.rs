//! 系统通知（TECH-DESIGN §7）：`@tauri-apps/plugin-notification` 一套 API
//! 通吃 Windows toast / macOS 通知中心。
//!
//! 触发：事件驱动唤醒（或全量刷新/对账）拉到新的 `actual`，且事件星级 ≥ 显示阈值，
//! 且通知开关打开 → 发送通知（标题：国家 · 事件名；正文：日期时间 + 实际/预期/前值）。
//! 去重：已通知的 event_id 集合持久化在 cache.json，重启不重发。
//! 新鲜度（清单 #5）：只弹发布后 30 分钟内的条目；开机/唤醒补拉的过期条目
//! 只填数不弹窗。事件到点提醒同理（30 分钟窗口）且开关与数据通知互相独立（#6）。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use caldata::model::CalendarEvent;
use chrono::{DateTime, Datelike, Timelike};
use chrono_tz::Tz;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// 通知新鲜度窗口（分钟）：发布超过此时长的一律不弹（含补拉场景）。
const FRESH_WINDOW_MIN: i64 = 30;

/// 国家中文名（通知标题用；与 `Country::code()` 对照）。
fn country_zh(code: &str) -> &str {
    match code {
        "US" => "美国",
        "CN" => "中国",
        "EU" => "欧元区",
        "UK" => "英国",
        "JP" => "日本",
        "AU" => "澳大利亚",
        "NZ" => "新西兰",
        "CA" => "加拿大",
        "CH" => "瑞士",
        _ => "其他",
    }
}

/// "10月23日 20:30"（北京时间，release_at 本身即上海时区）。
fn when_label(ev: &CalendarEvent) -> String {
    let t = ev.release_at;
    format!(
        "{}月{}日 {:02}:{:02}",
        t.month(),
        t.day(),
        t.hour(),
        t.minute()
    )
}

/// 是否在新鲜度窗口内（容忍 5 分钟时钟偏差的"未来"）。
/// 用精确 Duration 比较，与事件提醒的窗口过滤同口径（QA G3 Minor：
/// 旧实现 num_minutes() 向零截断，两路边界差最多 59 秒）。
fn is_fresh(ev: &CalendarEvent, now: &DateTime<Tz>) -> bool {
    let age = *now - ev.release_at;
    age <= chrono::Duration::minutes(FRESH_WINDOW_MIN) && age >= chrono::Duration::minutes(-5)
}

/// 进程内"已请求过通知权限"标记：macOS 上避免每次事件都弹授权框
static PERMISSION_ASKED: AtomicBool = AtomicBool::new(false);

/// 批量合并阈值：单次调用内新鲜条目 ≥ 此数合并为一条汇总通知。
/// 动机（用户 2026-10-23 拍板）：开机补拉会一次带回多条 30 分钟内
/// 公布的数据，逐条连环弹会吵；实时路径通常一次 1 条、不受影响，
/// 20:30 那种 3+ 条同刻公布合并成一条信息反而更紧凑。
const BATCH_MERGE_THRESHOLD: usize = 3;

/// 对一批"刚刚拿到 actual"的事件发送通知（满足阈值/开关/去重才会真正发送）。
pub async fn notify_new_actuals(app: &AppHandle, fresh: &[CalendarEvent]) {
    if fresh.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let cfg = state.config_snapshot();
    if !cfg.notifications {
        return;
    }

    // 权限：Windows 通常默认允许；macOS 首次需要用户授权（只问一次）
    match app.notification().permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted) => {}
        _ => {
            if !PERMISSION_ASKED.swap(true, Ordering::SeqCst) {
                let _ = app.notification().request_permission();
            }
            // 本次先跳过：等用户授权后，后续事件会正常通知
            // （未标记 notified，下次对账/唤醒仍会重试）
            return;
        }
    }

    let notified: HashSet<String> = state.cache_snapshot().notified_ids.into_iter().collect();
    let now = crate::now_sh();
    // 先筛出真正要弹的（有 actual / 过阈值 / 在关注国家内 / 未弹过 / 新鲜度窗口内）
    // 国家过滤：用户 2026-10-23 拍板——通知与面板同口径，没勾的国家不弹
    let eligible: Vec<&CalendarEvent> = fresh
        .iter()
        .filter(|ev| {
            ev.actual.is_some()
                && ev.importance >= cfg.min_stars
                && cfg.countries.iter().any(|c| c == ev.country.code())
                && !notified.contains(&ev.id)
                && is_fresh(ev, &now)
        })
        .collect();
    if eligible.is_empty() {
        return;
    }

    let mut to_mark: Vec<String> = Vec::new();
    if eligible.len() >= BATCH_MERGE_THRESHOLD {
        // 汇总一条：逐条列"国家 · 事件 实际/预期"，最多 4 条，余者收尾
        let mut lines: Vec<String> = eligible
            .iter()
            .take(4)
            .map(|ev| {
                format!(
                    "{} · {} 实际 {}/预期 {}",
                    country_zh(ev.country.code()),
                    ev.title,
                    ev.actual.as_deref().unwrap_or("—"),
                    ev.forecast.as_deref().unwrap_or("—")
                )
            })
            .collect();
        if eligible.len() > 4 {
            lines.push(format!("…共 {} 条，详情见挂件", eligible.len()));
        }
        let title = format!("{} 条数据刚公布", eligible.len());
        match app
            .notification()
            .builder()
            .title(title)
            .body(lines.join("\n"))
            .show()
        {
            Ok(_) => to_mark.extend(eligible.iter().map(|ev| ev.id.clone())),
            Err(e) => eprintln!("[notifier] 汇总通知发送失败: {e}"),
        }
    } else {
        for ev in eligible {
            let actual = ev.actual.as_deref().unwrap_or("—");
            let forecast = ev.forecast.as_deref().unwrap_or("—");
            let mut body = format!(
                "{} · 实际 {actual} / 预期 {forecast}",
                when_label(ev)
            );
            if let Some(prev) = ev.previous.as_deref() {
                body += &format!(" / 前值 {prev}");
            }
            let title = format!("{} · {}", country_zh(ev.country.code()), ev.title);
            match app
                .notification()
                .builder()
                .title(title)
                .body(body)
                .show()
            {
                Ok(_) => to_mark.push(ev.id.clone()),
                Err(e) => eprintln!("[notifier] 通知发送失败 {}: {e}", ev.id),
            }
        }
    }
    if !to_mark.is_empty() {
        state.mark_notified(&to_mark);
    }
}

/// 事件到点提醒（TECH-DESIGN §7 补丁）：kind == Event 的条目无数值可比，
/// 不走 actual 触发；仅当配置 `notify_events` 打开时（与数据通知开关
/// 互相独立，清单 #6），在事件时间到达后 30 分钟内（新鲜度窗口，#5）
/// 通知一次（标题：国家 · 事件名；正文：日期 · 事件时间已到）。
/// 数据行（Data）走 [`notify_new_actuals`]；假期（Holiday）永不通知。
/// 去重：已通知 id 持久化，重启不重发。
pub async fn notify_due_events(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = state.config_snapshot();
    if !cfg.notify_events {
        return;
    }

    match app.notification().permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted) => {}
        _ => {
            if !PERMISSION_ASKED.swap(true, Ordering::SeqCst) {
                let _ = app.notification().request_permission();
            }
            return;
        }
    }

    let now = crate::now_sh();
    let window_start = now - chrono::Duration::minutes(FRESH_WINDOW_MIN);
    let cache = state.cache_snapshot();
    let notified: HashSet<String> = cache.notified_ids.into_iter().collect();
    let mut to_mark: Vec<String> = Vec::new();
    for ev in cache.events.iter().filter(|e| {
        e.kind == caldata::model::EventKind::Event
            && e.release_at <= now
            && e.release_at > window_start
            && e.importance >= cfg.min_stars
            && cfg.countries.iter().any(|c| c == e.country.code())
            && !notified.contains(&e.id)
    }) {
        let body = match ev.status.as_deref() {
            Some(s) => format!("{} · 事件时间已到（{s}）", when_label(ev)),
            None => format!("{} · 事件时间已到", when_label(ev)),
        };
        let title = format!("{} · {}", country_zh(ev.country.code()), ev.title);
        match app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            Ok(_) => to_mark.push(ev.id.clone()),
            Err(e) => eprintln!("[notifier] 事件通知发送失败 {}: {e}", ev.id),
        }
    }
    if !to_mark.is_empty() {
        state.mark_notified(&to_mark);
    }
}
