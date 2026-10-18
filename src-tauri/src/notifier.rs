//! 系统通知（TECH-DESIGN §7）：`@tauri-apps/plugin-notification` 一套 API
//! 通吃 Windows toast / macOS 通知中心。
//!
//! 触发：事件驱动唤醒（或全量刷新/对账）拉到新的 `actual`，且事件星级 ≥ 显示阈值，
//! 且通知开关打开 → 发送通知（标题：事件名；正文：实际 x / 预期 y）。
//! 去重：已通知的 event_id 集合持久化在 cache.json，重启不重发。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use caldata::model::CalendarEvent;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// 进程内"已请求过通知权限"标记：macOS 上避免每次事件都弹授权框
static PERMISSION_ASKED: AtomicBool = AtomicBool::new(false);

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
    match app.notification().is_permission_granted() {
        Ok(true) => {}
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
    let mut to_mark: Vec<String> = Vec::new();
    for ev in fresh {
        let Some(actual) = ev.actual.as_ref() else {
            continue;
        };
        if ev.importance < cfg.min_stars {
            continue;
        }
        if notified.contains(&ev.id) {
            continue;
        }
        let forecast = ev.forecast.as_deref().unwrap_or("—");
        let body = format!("实际 {actual} / 预期 {forecast}");
        match app
            .notification()
            .builder()
            .title(ev.title.clone())
            .body(body)
            .show()
        {
            Ok(_) => to_mark.push(ev.id.clone()),
            Err(e) => eprintln!("[notifier] 通知发送失败 {}: {e}", ev.id),
        }
    }
    if !to_mark.is_empty() {
        state.mark_notified(&to_mark);
    }
}

/// 事件类通知（TECH-DESIGN §7 补丁）：kind == Event 的条目无数值可比，
/// 不走 actual 触发；仅当配置 `notify_events` 打开时，在事件时间到达后
/// 2 小时内通知一次（标题：事件名；正文：事件时间已到）。
/// 数据行（Data）走 [`notify_new_actuals`]；假期（Holiday）永不通知。
/// 去重：已通知 id 持久化，重启不重发。
pub async fn notify_due_events(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = state.config_snapshot();
    if !cfg.notifications || !cfg.notify_events {
        return;
    }

    match app.notification().is_permission_granted() {
        Ok(true) => {}
        _ => {
            if !PERMISSION_ASKED.swap(true, Ordering::SeqCst) {
                let _ = app.notification().request_permission();
            }
            return;
        }
    }

    let now = crate::now_sh();
    let window_start = now - chrono::Duration::hours(2);
    let cache = state.cache_snapshot();
    let notified: HashSet<String> = cache.notified_ids.into_iter().collect();
    let mut to_mark: Vec<String> = Vec::new();
    for ev in cache.events.iter().filter(|e| {
        e.kind == caldata::model::EventKind::Event
            && e.release_at <= now
            && e.release_at > window_start
            && e.importance >= cfg.min_stars
            && !notified.contains(&e.id)
    }) {
        let body = match ev.status.as_deref() {
            Some(s) => format!("事件时间已到（{s}）"),
            None => "事件时间已到".to_string(),
        };
        match app
            .notification()
            .builder()
            .title(ev.title.clone())
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
