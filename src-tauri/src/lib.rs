//! FinCalendarWidget —— Tauri 应用壳（TECH-DESIGN §5/§7/§8/§10）。
//!
//! 职责：透明穿透主窗口、托盘、设置窗口、commands、事件分发、后台调度装配。
//! 数据获取 / 解析 / 映射 / 调度纯函数全部在 `caldata` crate，本壳只做编排。
//!
//! 调用的 caldata 公开 API（`crates/caldata/src/lib.rs` 有再导出）：
//! - `caldata::investing_primary::InvestingPrimary::new()` / `.with_importance()`
//! - `caldata::investing_backup::InvestingBackup::new()` / `.with_importance()`
//! - `caldata::source::{DataSource::fetch_week, fetch_week_with_fallback,
//!   SourceMode, FetchOutcome, SourceError}`
//! - `caldata::schedule::{TZ, next_sunday_midnight, needs_refresh,
//!   build_wake_list, week_monday, weekday_cn}`
//! - `caldata::model::{CalendarEvent, Country::code(), Source}`
//! - `caldata::mapping::ImportanceMap { low, mid, high }`
//!   （本壳 `Config.importance_map` 经 `Config::to_importance_map()` 转换后注入）

mod notifier;
mod scheduler;
mod state;

use std::time::Duration;

use caldata::investing_backup::InvestingBackup;
use caldata::investing_primary::InvestingPrimary;
use caldata::model::Source;
use caldata::schedule::{self, TZ};
use caldata::source::{DataSource, FetchOutcome, SourceError};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder,
};

use state::{AppState, Config};

/// 上海时区当前时间（内部统一时区，TECH-DESIGN §6）
pub(crate) fn now_sh() -> DateTime<Tz> {
    Utc::now().with_timezone(&TZ)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 已有实例在跑：把主窗口拉出来（主窗口平时点击穿透、无焦点）
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("无法解析 app_data_dir");
            std::fs::create_dir_all(&dir).ok();

            let (state, wake_rx) = AppState::load(dir)?;
            app.manage(state);

            position_main_window(app.handle());

            // 开机自启（best-effort，失败只打日志不阻断启动）
            {
                use tauri_plugin_autostart::ManagerExt;
                if let Err(e) = app.autostart().enable() {
                    eprintln!("[app] 开机自启开启失败: {e}");
                }
            }

            build_tray(app.handle());
            scheduler::spawn(app.handle().clone(), wake_rx);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_config,
            set_config,
            test_sources,
            refresh_now
        ])
        .run(tauri::generate_context!())
        .expect("FinCalendarWidget 启动失败");
}

// ---------------- 窗口与托盘 ----------------

/// 主窗口定位：主显示器工作区右半屏（宽 42%），或按配置切左半屏；
/// 随后开启点击穿透（TECH-DESIGN §5）。
fn position_main_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let side = app.state::<AppState>().config_snapshot().side;
    // 主显示器工作区（排除任务栏）；取不到则回退 1920x1080
    let (wx, wy, ww, wh) = match app.primary_monitor().ok().flatten() {
        Some(m) => {
            let r = m.work_area();
            (r.position.x, r.position.y, r.size.width, r.size.height)
        }
        None => (0, 0, 1920, 1080),
    };
    let w = (ww as f64 * 0.42) as u32;
    let h = wh;
    let x = if side == "left" {
        wx
    } else {
        wx + ww as i32 - w as i32
    };
    let _ = win.set_size(tauri::PhysicalSize::new(w, h));
    let _ = win.set_position(tauri::PhysicalPosition::new(x, wy));
    // 纯点击穿透：挂件不可交互，所有操作走托盘 / 设置页
    if let Err(e) = win.set_ignore_cursor_events(true) {
        eprintln!("[window] 点击穿透开启失败: {e}");
    }
    // 注：alwaysOnTop=false 即普通窗口层级；Tauri 未暴露 lower()，
    // "图标层之下"列为 v2 调研项（§5），v1 不做。
}

/// 托盘：打开设置 / 立即刷新 / 退出
fn build_tray(app: &AppHandle) {
    let open_item = MenuItemBuilder::with_id("tray_open_settings", "打开设置")
        .build(app)
        .expect("托盘菜单创建失败");
    let refresh_item = MenuItemBuilder::with_id("tray_refresh", "立即刷新")
        .build(app)
        .expect("托盘菜单创建失败");
    let quit_item = MenuItemBuilder::with_id("tray_quit", "退出")
        .build(app)
        .expect("托盘菜单创建失败");
    let menu = MenuBuilder::new(app)
        .items(&[&open_item, &refresh_item, &quit_item])
        .build()
        .expect("托盘菜单创建失败");

    // 图标：优先用 bundle.icon 提供的窗口图标，缺失时用 1x1 透明占位
    let icon = app
        .default_window_icon()
        .cloned()
        .unwrap_or_else(|| tauri::image::Image::new_owned(vec![0u8; 4], 1, 1));

    TrayIconBuilder::new()
        .menu(&menu)
        .icon(icon)
        .tooltip("财经日历挂件")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray_open_settings" => open_settings(app),
            "tray_refresh" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    scheduler::full_refresh(&app).await;
                });
            }
            "tray_quit" => app.exit(0),
            _ => {}
        })
        .build(app)
        .expect("托盘图标创建失败");
}

/// 设置窗口：独立 WebviewWindow，非穿透、可交互（TECH-DESIGN §8）
fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Err(e) = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("设置 — 财经日历挂件")
        .inner_size(600.0, 680.0)
        .min_inner_size(520.0, 560.0)
        .decorations(true)
        .resizable(true)
        .skip_taskbar(false)
        .build()
    {
        eprintln!("[window] 设置窗口创建失败: {e}");
    }
}

// ---------------- commands ----------------

/// 前端渲染用全量状态：扁平事件数组（前端自行过滤/分组/排序）+ 当前配置 + 更新元信息
#[tauri::command]
async fn get_state(state: State<'_, AppState>) -> Result<WidgetState, String> {
    Ok(build_widget_state(&state))
}

/// 读取当前配置
#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    Ok(state.config_snapshot())
}

/// 写入配置：校验 → 落盘（含重建数据源）→ 重定位窗口 → 后台立即全量刷新一次
/// （刷新完成后经 `calendar-updated` 事件通知前端重渲染）
#[tauri::command]
async fn set_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
) -> Result<(), String> {
    config.validate()?;
    state.save_config(config)?;
    position_main_window(&app);
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        scheduler::full_refresh(&app2).await;
    });
    Ok(())
}

/// 接口测试（TECH-DESIGN §8）：对 primary / backup 分别实测一次本周拉取，
/// 返回连通性、延迟、解析行数、错误原文（不受配置的 source 模式影响，两路都测）
#[tauri::command]
async fn test_sources() -> Result<Vec<SourceTestResult>, String> {
    let week_start = schedule::week_monday(now_sh().date_naive());
    let candidates: Vec<(&str, Result<Box<dyn DataSource>, SourceError>)> = vec![
        (
            "primary",
            InvestingPrimary::new().map(|s| Box::new(s) as Box<dyn DataSource>),
        ),
        (
            "backup",
            InvestingBackup::new().map(|s| Box::new(s) as Box<dyn DataSource>),
        ),
    ];
    let mut out = Vec::with_capacity(2);
    for (label, res) in candidates {
        let src = match res {
            Ok(s) => s,
            Err(e) => {
                out.push(SourceTestResult {
                    name: label.to_string(),
                    ok: false,
                    latency_ms: 0,
                    rows: 0,
                    error: Some(format!("数据源构造失败: {e}")),
                });
                continue;
            }
        };
        let name = src.name().to_string();
        let t0 = std::time::Instant::now();
        let res = tokio::time::timeout(Duration::from_secs(30), src.fetch_week(week_start)).await;
        let latency_ms = t0.elapsed().as_millis();
        match res {
            Ok(Ok(events)) => out.push(SourceTestResult {
                name,
                ok: true,
                latency_ms,
                rows: events.len(),
                error: None,
            }),
            Ok(Err(e)) => out.push(SourceTestResult {
                name,
                ok: false,
                latency_ms,
                rows: 0,
                error: Some(e.to_string()),
            }),
            Err(_) => out.push(SourceTestResult {
                name,
                ok: false,
                latency_ms,
                rows: 0,
                error: Some("超时（30s 未返回）".to_string()),
            }),
        }
    }
    Ok(out)
}

/// 立即全量刷新（托盘 / 设置页调用）
#[tauri::command]
async fn refresh_now(app: AppHandle) -> Result<RefreshReport, String> {
    let outcome: FetchOutcome = scheduler::full_refresh(&app).await;
    let ok = !outcome.from_cache;
    Ok(RefreshReport {
        ok,
        events: outcome.events.len(),
        used: outcome.used.map(|s| match s {
            Source::Primary => "primary".to_string(),
            Source::Backup => "backup".to_string(),
        }),
        from_cache: outcome.from_cache,
        error: if ok {
            None
        } else {
            Some(scheduler::format_errors(&outcome))
        },
    })
}

// ---------------- 前端视图 ----------------
//
// 注意：get_state 返回扁平 events 数组（前端 app.js 自行按国家/星级过滤、
// 按天分组、按 release_at 排序），字段名与前端读取的保持一致：
// release_at（ISO 8601）、country（双字母码）、kind（"data"|"event"|"holiday"）、
// title、importance、forecast、previous、actual、
// deviation（"beat"|"miss"|"inline"|null，中性预期差，视图层实时计算）、
// status（事件行状态如"待定"|null）、
// usd_sentiment（"bull"|"bear"|null，仅 usd_mode 开启时计算，供美元口径着色）。

#[derive(Debug, Clone, Serialize)]
struct EventView {
    id: String,
    release_at: String,
    time: String, // "HH:mm"（备用，前端主要用 release_at）
    country: String,
    kind: String,
    title: String,
    importance: u8,
    forecast: Option<String>,
    previous: Option<String>,
    actual: Option<String>,
    deviation: Option<String>,
    status: Option<String>,
    usd_sentiment: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WidgetState {
    events: Vec<EventView>,
    config: Config,
    updated_at: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SourceTestResult {
    name: String,
    ok: bool,
    latency_ms: u128,
    rows: usize,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RefreshReport {
    ok: bool,
    events: usize,
    used: Option<String>,
    from_cache: bool,
    error: Option<String>,
}

/// 组装前端渲染状态：全量事件（扁平，不在后端过滤——前端自行过滤/分组/排序）。
/// deviation（中性预期差）在视图层实时计算，保证 actual 更新后颜色永远与最新数据一致；
/// usd_sentiment 仅在配置 usd_mode 开启时计算（美元口径利多/利空，可选项）。
pub(crate) fn build_widget_state(state: &AppState) -> WidgetState {
    let cfg = state.config_snapshot();
    let cache = state.cache_snapshot();

    let mut events: Vec<EventView> = cache
        .events
        .iter()
        .map(|e| {
            // 中性预期差：只看 actual vs forecast，不下方向结论
            let deviation =
                match caldata::mapping::deviation(e.actual.as_deref(), e.forecast.as_deref()) {
                    Some(caldata::mapping::Deviation::Beat) => Some("beat".to_string()),
                    Some(caldata::mapping::Deviation::Miss) => Some("miss".to_string()),
                    Some(caldata::mapping::Deviation::Inline) => Some("inline".to_string()),
                    None => None,
                };
            // 美元口径（可选项）：沿用方向映射表，默认关闭
            let usd_sentiment = if cfg.usd_mode {
                let dir = caldata::mapping::direction_for(&e.title);
                match caldata::mapping::sentiment(e.actual.as_deref(), e.forecast.as_deref(), dir) {
                    caldata::mapping::Sentiment::Bullish => Some("bull".to_string()),
                    caldata::mapping::Sentiment::Bearish => Some("bear".to_string()),
                    caldata::mapping::Sentiment::Neutral => None,
                }
            } else {
                None
            };
            EventView {
                id: e.id.clone(),
                release_at: e.release_at.to_rfc3339(),
                time: e.release_at.format("%H:%M").to_string(),
                country: e.country.code().to_string(),
                kind: match e.kind {
                    caldata::model::EventKind::Data => "data".to_string(),
                    caldata::model::EventKind::Event => "event".to_string(),
                    caldata::model::EventKind::Holiday => "holiday".to_string(),
                },
                title: e.title.clone(),
                importance: e.importance,
                forecast: e.forecast.clone(),
                previous: e.previous.clone(),
                actual: e.actual.clone(),
                deviation,
                status: e.status.clone(),
                usd_sentiment,
            }
        })
        .collect();
    events.sort_by(|a, b| a.release_at.cmp(&b.release_at));

    WidgetState {
        events,
        config: cfg,
        updated_at: cache.updated_at,
        last_error: cache.last_error,
    }
}
