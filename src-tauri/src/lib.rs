//! FinCalendarWidget —— Tauri 应用壳（TECH-DESIGN §5/§7/§8/§10）。
//!
//! 职责：透明穿透主窗口、托盘、设置窗口、commands、事件分发、后台调度装配。
//! 数据获取 / 解析 / 映射 / 调度纯函数全部在 `caldata` crate，本壳只做编排。
//!
//! 调用的 caldata 公开 API（`crates/caldata/src/lib.rs` 有再导出）：
//! - `caldata::jin10::Jin10::new()` / `caldata::faireconomy::Faireconomy::new()`
//! - `caldata::source::{DataSource::fetch_week, fetch_week_with_fallback,
//!   SourceMode, FetchOutcome, SourceError}`
//! - `caldata::schedule::{TZ, next_weekly_refresh, needs_refresh,
//!   build_wake_list, week_monday, weekday_cn}`
//! - `caldata::model::{CalendarEvent, Country::code(), Source}`

mod notifier;
mod scheduler;
mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use caldata::faireconomy::Faireconomy;
use caldata::jin10::Jin10;
use caldata::model::Source;
use caldata::schedule::{self, TZ};
use caldata::source::{DataSource, FetchOutcome, SourceError};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder,
};

use state::{AppState, Config};

/// 交互模式运行时状态（2026-10-23 混合式）：不落盘，重启必回穿透。
/// `on` 是唯一事实源——place_window 重贴边时据此决定是否穿透，
/// 防数据刷新触发的改宽把穿透偷偷开回、与前端泳道状态脱节。
struct InteractiveState {
    on: AtomicBool,
    item: Mutex<Option<CheckMenuItem>>,
}

/// 上海时区当前时间（内部统一时区，TECH-DESIGN §6）
pub(crate) fn now_sh() -> DateTime<Tz> {
    Utc::now().with_timezone(&TZ)
}

pub fn run() {
    tauri::Builder::default()
        .on_window_event(|window, event| {
            // 设置窗关闭回滚（清单 #13）：预览只改内存配置；窗口销毁后
            // 若内存配置 != 已落盘配置，Rust 侧恢复 + 重定位 + 广播。
            // 回滚必须走 Rust——旧做法在 JS 里拦截 CloseRequested、等
            // IPC 后再 destroy，WebView2 上会把窗口拆成黑框僵尸
            //（2026-10-23 真机实证：点 X 关不掉、取消后窗口内容变黑）。
            if window.label() == "settings"
                && matches!(event, tauri::WindowEvent::Destroyed)
            {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                if state.rollback_preview() {
                    // 应用退出时主窗可能已先销毁：不在场就只回滚内存，
                    // 不做窗口操作与广播（QA 防御）
                    if app.get_webview_window("main").is_some() {
                        reposition_preserving_width(&app);
                        let payload = build_widget_state(&state);
                        if let Err(e) = app.emit("calendar-updated", payload) {
                            eprintln!("[app] 设置关闭回滚广播失败: {e}");
                        }
                    }
                }
            }
        })
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
            app.manage(InteractiveState {
                on: AtomicBool::new(false),
                item: Mutex::new(None),
            });

            position_main_window(app.handle());

            // 主窗口获得焦点时重新压到底（防浮到应用窗口之上）
            if let Some(win) = app.get_webview_window("main") {
                let win_clone = win.clone();
                win.on_window_event(move |ev| {
                    if matches!(ev, tauri::WindowEvent::Focused(true)) {
                        pin_to_bottom(&win_clone);
                    }
                });
            }

            // 开机自启（清单 #9a）：按配置同步——关了就注销，不再像
            // v0.1 那样每次启动无条件强制注册（best-effort，失败只打日志）
            {
                use tauri_plugin_autostart::ManagerExt;
                let want = app.state::<AppState>().config_snapshot().autostart;
                let mgr = app.handle().autolaunch();
                let res = if want { mgr.enable() } else { mgr.disable() };
                if let Err(e) = res {
                    eprintln!("[app] 开机自启同步失败: {e}");
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
            preview_config,
            set_panel_width,
            set_interactive,
            get_interactive,
            test_sources,
            test_actuals,
            refresh_now
        ])
        .run(tauri::generate_context!())
        .expect("FinCalendarWidget 启动失败");
}

// ---------------- 窗口与托盘 ----------------

/// 按列数计算窗口宽度（物理 px）。几何与 styles.css 对齐：面板横向
/// 内边距 2×1.1em + 边框 2px、列间隔 1.6em（`em_px` = font_size × DPI
/// 缩放）。**全程浮点、总量向上取整**：逐分量取整会在默认字号下产生
/// 负误差、静默吞掉最后一列（QA B1），禁止改回整数分量公式。
fn width_for_columns(em_px: f64, col_w: u32, cols: u32) -> u32 {
    let total = 2.2 * em_px + 2.0
        + cols as f64 * col_w as f64
        + cols.saturating_sub(1) as f64 * 1.6 * em_px;
    total.ceil() as u32
}

/// 把期望宽度夹进 [320, max_w]；超限时向下取整到整列（永不半列）。
fn snap_to_columns(em_px: f64, col_w: u32, want: u32, max_w: u32) -> u32 {
    let max_w = max_w.max(320);
    if want <= max_w {
        return want.max(320);
    }
    let pad_h = 2.2 * em_px + 2.0;
    let gap = 1.6 * em_px;
    let cols = (((max_w as f64 - pad_h + gap) / (col_w as f64 + gap)).floor() as u32).max(1);
    width_for_columns(em_px, col_w, cols).min(max_w)
}

/// 主窗口 DPI 缩放（前端按 CSS 像素计宽，窗口 API 用物理像素，须换算，QA B2）。
fn window_scale(app: &AppHandle) -> f64 {
    app.get_webview_window("main")
        .and_then(|w| w.scale_factor().ok())
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0)
}

/// 主显示器工作区（x, y, w, h）；取不到则回退 1920x1080。
fn work_area(app: &AppHandle) -> (i32, i32, u32, u32) {
    match app.primary_monitor().ok().flatten() {
        Some(m) => {
            let r = m.work_area();
            (r.position.x, r.position.y, r.size.width, r.size.height)
        }
        None => (0, 0, 1920, 1080),
    }
}

/// 把主窗口按给定宽度贴边放置（高 = 工作区全高），并开启点击穿透 + 压底。
fn place_window(app: &AppHandle, width: u32) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let cfg = app.state::<AppState>().config_snapshot();
    let (wx, wy, ww, wh) = work_area(app);
    let hi = ww.max(1);
    let w = width.clamp(320.min(hi), hi);
    let x = if cfg.side == "left" {
        wx
    } else {
        wx + ww as i32 - w as i32
    };
    let _ = win.set_size(tauri::PhysicalSize::new(w, wh));
    let _ = win.set_position(tauri::PhysicalPosition::new(x, wy));
    // 点击穿透：默认全穿透（挂件不可交互，操作走托盘 / 设置页）；
    // 交互模式中则保持可交互——重贴边（刷新改宽 / 配置变更）不得
    // 把穿透偷偷开回，否则前端泳道还在、鼠标已死（状态脱节）。
    let interactive = app
        .try_state::<InteractiveState>()
        .map(|s| s.on.load(Ordering::SeqCst))
        .unwrap_or(false);
    if let Err(e) = win.set_ignore_cursor_events(!interactive) {
        eprintln!("[window] 点击穿透设置失败: {e}");
    }
    // Windows：钉在 Z 序最底层（壁纸层），防止浮到应用窗口之上
    pin_to_bottom(&win);
}

/// 初始定位：手动模式用 manual_columns，自适应先给 3 列，
/// 首帧渲染后由前端探针实测列数、经 set_panel_width 修正。
/// 初始宽同样走整列取整，防手动大列数首帧出现半列闪烁（QA m2）。
fn position_main_window(app: &AppHandle) {
    let cfg = app.state::<AppState>().config_snapshot();
    let scale = window_scale(app);
    let em_px = cfg.font_size as f64 * scale;
    let col_w = (300.0 * scale).round() as u32;
    let cols = if cfg.width_mode == "manual" {
        cfg.manual_columns as u32
    } else {
        3
    };
    let (_, _, ww, _) = work_area(app);
    let max_w = (ww as f64 * 0.85) as u32;
    let w = snap_to_columns(em_px, col_w, width_for_columns(em_px, col_w, cols), max_w);
    place_window(app, w);
}

/// 配置变更时重贴边（左右切换）：保持当前窗口宽度，避免自适应宽度
/// 被重置回 3 列造成闪烁；宽度调整交给前端 fitLayout 经 set_panel_width 完成。
fn reposition_preserving_width(app: &AppHandle) {
    let cur = app
        .get_webview_window("main")
        .and_then(|w| w.outer_size().ok())
        .map(|s| s.width);
    match cur {
        Some(w) if w >= 320 => place_window(app, w),
        _ => position_main_window(app),
    }
}

/// 交互模式切换的唯一入口（托盘勾选 / set_interactive 命令共用）：
/// 状态位 → 托盘勾选 → 窗口穿透 → 焦点 → 广播给前端切泳道。
/// 进入时给焦点，Esc 才有处可落；退出不抢焦点（用户多半已切走）。
/// 串行性（QA m2）：同步命令与菜单事件同在主线程事件循环串行执行，
/// 本函数不可重入；若日后把 set_interactive 改成 async 命令，须先
/// 给这里加整体串行化，否则读-改-写会交错出混合终态。
fn apply_interactive(app: &AppHandle, on: bool) {
    if let Some(st) = app.try_state::<InteractiveState>() {
        st.on.store(on, Ordering::SeqCst);
        // 先在锁内 clone 句柄、放锁再调平台 API（QA m1）：set_checked
        // 在个别平台会同步派发主线程，持锁调用有互等风险；锁中毒要
        // 留日志，不能静默跳过（否则勾选永久不同步且无迹可查）。
        let item = match st.item.lock() {
            Ok(g) => g.clone(),
            Err(_) => {
                eprintln!("[app] 交互菜单锁中毒，本次勾选同步跳过");
                None
            }
        };
        if let Some(item) = item {
            let _ = item.set_checked(on);
        }
    }
    if let Some(win) = app.get_webview_window("main") {
        if let Err(e) = win.set_ignore_cursor_events(!on) {
            eprintln!("[window] 交互模式穿透切换失败: {e}");
        }
        if on {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
    if let Err(e) = app.emit_to("main", "interactive-changed", on) {
        eprintln!("[app] interactive-changed 广播失败: {e}");
    }
}

/// 前端请求切换交互模式（Esc / 面板退出按钮调用）
#[tauri::command]
fn set_interactive(app: AppHandle, on: bool) {
    apply_interactive(&app, on);
}

/// 前端查询交互状态：interactive-changed 是即发即忘事件，监听注册前
/// 发生的切换（冷启动时托盘抢先点、WebView 崩溃重载）会永久丢失，
/// 前端启动后靠本命令向事实源对账收敛（QA M1）。
#[tauri::command]
fn get_interactive(app: AppHandle) -> bool {
    app.try_state::<InteractiveState>()
        .map(|s| s.on.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// 前端布局引擎请求窗口宽度（整列宽，入参为 CSS 像素）：内部换算为
/// 物理像素后夹取到 [320, 工作区 85%]；被夹取时向下取整到整列，
/// 保证永不出现半列（问题清单 #3/#4）。返回实际应用的宽度（CSS px）。
#[tauri::command]
fn set_panel_width(app: AppHandle, width: u32, col_w: u32) -> Result<u32, String> {
    let cfg = app.state::<AppState>().config_snapshot();
    let scale = window_scale(&app);
    let em_px = cfg.font_size as f64 * scale;
    let col_w_phys = ((col_w.max(1) as f64) * scale).round() as u32;
    let want_phys = (width as f64 * scale).round() as u32;
    let (_, _, ww, _) = work_area(&app);
    let max_w = (ww as f64 * 0.85) as u32;
    let w = snap_to_columns(em_px, col_w_phys, want_phys, max_w);
    place_window(&app, w);
    Ok((w as f64 / scale).round() as u32)
}

/// Windows：`SetWindowPos(HWND_BOTTOM)` 把窗口压到 Z 序最底。
/// 获得焦点时由 `on_window_event(Focused)` 重新压底（见 setup 中的监听）。
#[cfg(target_os = "windows")]
fn pin_to_bottom(win: &tauri::WebviewWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    // 经 raw-window-handle 取 isize（版本无关），再构本 crate 的 HWND，
    // 避开 Tauri 自带 windows crate 与本 crate 的 HWND 类型不一致问题。
    let hwnd: Option<HWND> = (|| {
        let handle = win.window_handle().ok()?;
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            Some(HWND(h.hwnd.get() as *mut std::ffi::c_void))
        } else {
            None
        }
    })();
    match hwnd {
        Some(h) => unsafe {
            let _ = SetWindowPos(
                h,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        },
        None => eprintln!("[window] 获取 HWND 失败"),
    }
}

/// 非 Windows：空实现，保持跨平台编译。
#[cfg(not(target_os = "windows"))]
fn pin_to_bottom(_win: &tauri::WebviewWindow) {}

/// 托盘：打开设置 / 交互浏览 / 立即刷新 / 退出
fn build_tray(app: &AppHandle) {
    let open_item = MenuItemBuilder::with_id("tray_open_settings", "打开设置")
        .build(app)
        .expect("托盘菜单创建失败");
    let interactive_item = CheckMenuItemBuilder::with_id("tray_interactive", "交互浏览（滚轮）")
        .checked(false)
        .build(app)
        .expect("托盘菜单创建失败");
    // 勾选状态由 apply_interactive 统一维护（Esc/退出按钮退出时也要同步）
    if let Some(st) = app.try_state::<InteractiveState>() {
        if let Ok(mut g) = st.item.lock() {
            *g = Some(interactive_item.clone());
        }
    }
    let refresh_item = MenuItemBuilder::with_id("tray_refresh", "立即刷新")
        .build(app)
        .expect("托盘菜单创建失败");
    let quit_item = MenuItemBuilder::with_id("tray_quit", "退出")
        .build(app)
        .expect("托盘菜单创建失败");
    let menu = MenuBuilder::new(app)
        .items(&[&open_item, &interactive_item, &refresh_item, &quit_item])
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
            "tray_interactive" => {
                let cur = app
                    .try_state::<InteractiveState>()
                    .map(|s| s.on.load(Ordering::SeqCst))
                    .unwrap_or(false);
                apply_interactive(app, !cur);
            }
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
    // 兜底（QA M2）：极窄竞态下（关窗瞬间在途的 preview 在 Destroyed
    // 回滚之后落地）内存可能残留未应用预览；新建窗口前再对账一次，
    // 保证设置页打开时读到的就是落盘配置
    {
        let state = app.state::<AppState>();
        if state.rollback_preview() {
            reposition_preserving_width(app);
            let payload = build_widget_state(&state);
            if let Err(e) = app.emit("calendar-updated", payload) {
                eprintln!("[app] 打开设置前回滚广播失败: {e}");
            }
        }
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

/// 设置页预览（清单 #7）：仅内存生效——换配置/重建数据源/主窗口即时
/// 重渲染，但不落盘、不触发全量刷新。点「应用」走 set_config 落盘；
/// 未应用的预览在设置窗销毁时由 rollback_preview 回滚（清单 #13）。
#[tauri::command]
async fn preview_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
) -> Result<(), String> {
    config.validate()?;
    state.apply_config(config)?;
    reposition_preserving_width(&app);
    let payload = build_widget_state(&state);
    if let Err(e) = app.emit("calendar-updated", payload) {
        eprintln!("[preview_config] 发送 calendar-updated 失败: {e}");
    }
    Ok(())
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
    let want_autostart = config.autostart;
    state.save_config(config)?;
    // 开机自启只在「应用」落盘时同步系统状态（预览不碰系统，清单 #7/#9a）
    {
        use tauri_plugin_autostart::ManagerExt;
        let mgr = app.autolaunch();
        let res = if want_autostart {
            mgr.enable()
        } else {
            mgr.disable()
        };
        if let Err(e) = res {
            eprintln!("[set_config] 开机自启同步失败: {e}");
        }
    }
    reposition_preserving_width(&app);
    // 通知主窗口立即重渲染（配置变更即时生效）
    let payload = build_widget_state(&state);
    if let Err(e) = app.emit("calendar-updated", payload) {
        eprintln!("[set_config] 发送 calendar-updated 失败: {e}");
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        scheduler::full_refresh(&app2).await;
    });
    Ok(())
}

/// 接口测试（TECH-DESIGN §8；v0.2 轻量化，清单 #10）：返回连通性、
/// 延迟、解析行数、错误原文（不受配置的 source 模式影响）。
/// - 金十：**探活**——完整握手/登录，只拉今日一天（2 个请求），
///   不再每次测试都全量拉整周，降低平台风控风险；
/// - Faireconomy：其接口本身即整周一个文件，1 次 GET 已是最小粒度。
#[tauri::command]
async fn test_sources() -> Result<Vec<SourceTestResult>, String> {
    let mut out = Vec::with_capacity(2);

    // 金十探活（仅今日）
    {
        let name = "金十（探活 · 仅今日）".to_string();
        let t0 = std::time::Instant::now();
        match Jin10::new() {
            Ok(src) => {
                let res = tokio::time::timeout(
                    Duration::from_secs(30),
                    src.probe(now_sh().date_naive()),
                )
                .await;
                let latency_ms = t0.elapsed().as_millis();
                match res {
                    Ok(Ok(rows)) => out.push(SourceTestResult {
                        name,
                        ok: true,
                        latency_ms,
                        rows,
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
            Err(e) => out.push(SourceTestResult {
                name,
                ok: false,
                latency_ms: 0,
                rows: 0,
                error: Some(format!("数据源构造失败: {e}")),
            }),
        }
    }

    // Faireconomy：整周 1 次 GET（该接口的最小粒度）
    {
        let week_start = schedule::week_monday(now_sh().date_naive());
        match Faireconomy::new() {
            Ok(src) => {
                let name = format!("{}（整周 · 1 次请求）", src.name());
                let t0 = std::time::Instant::now();
                let res =
                    tokio::time::timeout(Duration::from_secs(30), src.fetch_week(week_start))
                        .await;
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
            Err(e) => out.push(SourceTestResult {
                name: "faireconomy".to_string(),
                ok: false,
                latency_ms: 0,
                rows: 0,
                error: Some(format!("数据源构造失败: {e}")),
            }),
        }
    }

    Ok(out)
}

/// 实际值覆盖测试（设置页"实际值回填测试"按钮调用，2026-10-23 改版）
/// 走生产同款降级链（金十 → Faireconomy+FF）抓本周，
/// 统计已公布数据行中带实际值的比例，并给出样本。
/// 返回：{ source, total, published, with_actual, samples[≤5] }
#[tauri::command]
async fn test_actuals() -> Result<serde_json::Value, String> {
    let jin10 = Jin10::new().map_err(|e| e.to_string())?;
    let fe = Faireconomy::new().map_err(|e| e.to_string())?;
    let week_start = schedule::week_monday(now_sh().date_naive());
    let outcome = caldata::source::fetch_week_with_fallback(
        &jin10,
        &fe,
        caldata::source::SourceMode::Auto,
        week_start,
        None,
    )
    .await;
    if outcome.from_cache {
        return Err(format!(
            "全链数据源均失败：{}",
            scheduler::format_errors(&outcome)
        ));
    }

    let now = now_sh();
    let events = &outcome.events;
    let published: Vec<_> = events
        .iter()
        .filter(|e| e.kind == caldata::model::EventKind::Data && e.release_at <= now)
        .collect();
    let with_actual = published.iter().filter(|e| e.actual.is_some()).count();
    let samples: Vec<serde_json::Value> = published
        .iter()
        .filter(|e| e.actual.is_some())
        .take(5)
        .map(|e| {
            serde_json::json!({
                "title": e.title,
                "country": format!("{:?}", e.country),
                "forecast": e.forecast,
                "previous": e.previous,
                "actual": e.actual,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "source": scheduler::describe_used(outcome.used),
        "total": events.len(),
        "published": published.len(),
        "with_actual": with_actual,
        "samples": samples,
    }))
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
            Source::Jin10 => "jin10".to_string(),
            Source::Faireconomy => "faireconomy".to_string(),
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
