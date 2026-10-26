//! Windows 桌面挂靠（ISSUES #17）：把主窗口挂到 Explorer 的桌面 WorkerW，
//! 使其位于壁纸之上、桌面图标之下（`HWND_BOTTOM` 做不到这一点：它只在普通
//! 顶层窗口 z-order 里垫底，而图标属于独立的桌面层）。
//!
//! 经典路线（Rainmeter 同款）：
//! 1. `FindWindow("Progman")` → 发 `0x052C` 生成 WorkerW；
//! 2. `EnumWindows` 找含 `SHELLDLL_DefView` 子窗口的 WorkerW（图标层宿主），
//!    取其后的下一个 WorkerW 兄弟窗口——即新生成的、位于图标层之下的那一层；
//! 3. `SetParent(挂件 HWND, 该 WorkerW)`，坐标改按 WorkerW 客户区相对坐标换算。
//!
//! 关键设计（模式相关挂靠，勿改成永久挂靠）：
//! - 图标层（DefView）是全屏覆盖窗口，位于挂靠层之上：永久挂靠会使挂件收不到
//!   任何鼠标输入，交互模式（泳道滚轮/悬停）会直接报废。
//! - 因此挂靠只在**被动穿透态**生效；进入交互模式时先 `SetParent(NULL)` 脱钩回
//!   顶层窗口（位置用脱钩前的屏幕矩形原地保留），退出交互时经重贴边自动回挂。
//!   交互期间的表现与 v0.2.0 一致（顶层窗口 + 旧压底兜底）。
//!
//! 容错：
//! - WorkerW 句柄缓存 + `IsWindow` 校验；Explorer 重启后旧句柄失效，自动重新发现；
//! - `0x052C` 只在缓存失效时发送，避免重复生成 WorkerW；
//! - 任何一步失败返回失败，调用方回落到旧的 `HWND_BOTTOM` 压底
//!   （行为不差于 v0.2.0）。
//!
//! 已知边界（取舍，见 ISSUES #17）：
//! - Explorer 进程重启会销毁其 WorkerW，随之销毁挂靠的子窗口（Win32 语义：
//!   父窗口销毁时子窗口一并销毁）。此时托盘仍在，重新双击应用图标可经
//!   单实例回调重建主窗口（lib.rs 单实例分支已处理）；不做常驻监听重建。

use serde::Serialize;
use std::sync::Mutex;
use tauri::WebviewWindow;

/// #17 诊断（2026-10-26 用户实测：新包仍压图标，挂靠静默失败）：
/// 记录挂靠每一步的结果，供设置页「桌面挂靠诊断」展示与日志文件留存。
/// 只记录、不改变挂靠行为；任何记录失败都静默忽略，永不影响贴边/交互。
#[derive(Serialize, Clone)]
pub struct PinEvent {
    pub time: String,
    pub kind: String,
    pub detail: String,
}

/// 挂靠诊断快照：当前父窗口（句柄 + 类名是定性关键——WorkerW 才算挂上，
/// 顶层窗口/其它类都说明没挂上或挂错层）+ 缓存 WorkerW 存活 + 最近事件。
#[derive(Serialize)]
pub struct PinDiag {
    pub supported: bool,
    pub window: Option<String>,
    pub current_parent: Option<String>,
    pub current_parent_class: String,
    pub cached_workerw: Option<String>,
    pub cached_alive: Option<bool>,
    pub log_path: Option<String>,
    pub events: Vec<PinEvent>,
}

static DIAG_EVENTS: Mutex<Vec<PinEvent>> = Mutex::new(Vec::new());
static DIAG_LOG_PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// 设置诊断日志文件路径（lib.rs setup 中调用一次；不调用则只保留内存事件）。
pub fn init_diag_log(path: std::path::PathBuf) {
    if let Ok(mut slot) = DIAG_LOG_PATH.lock() {
        *slot = Some(path);
    }
}

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub(crate) fn diag_log_path() -> Option<String> {
    DIAG_LOG_PATH
        .lock()
        .ok()
        .and_then(|g| (*g).clone())
        .map(|p| p.display().to_string())
}

pub(crate) fn diag_events() -> Vec<PinEvent> {
    DIAG_EVENTS.lock().map(|g| (*g).clone()).unwrap_or_default()
}

/// 记录一条诊断事件（内存保留最近 40 条；文件追加，超 256KB 截断重写）。
pub(crate) fn record(kind: &str, detail: String) {
    let line = format!("[{}] {}: {}", now_str(), kind, detail);
    if let Ok(mut events) = DIAG_EVENTS.lock() {
        events.push(PinEvent {
            time: now_str(),
            kind: kind.to_string(),
            detail,
        });
        if events.len() > 40 {
            let excess = events.len() - 40;
            events.drain(..excess);
        }
    }
    let path = DIAG_LOG_PATH.lock().ok().and_then(|g| (*g).clone());
    if let Some(path) = path {
        let too_big = std::fs::metadata(&path)
            .map(|m| m.len() > 256 * 1024)
            .unwrap_or(false);
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true);
        if too_big {
            opts.write(true).truncate(true);
        } else {
            opts.append(true);
        }
        if let Ok(mut f) = opts.open(&path) {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
            let _ = f.write_all(b"\n");
        }
    }
}

/// 当前挂靠诊断快照（设置页调用；非 Windows 返回 supported=false）。
pub fn diag_snapshot(win: &WebviewWindow) -> PinDiag {
    #[cfg(target_os = "windows")]
    {
        return imp::diag_snapshot(win);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = win;
        PinDiag {
            supported: false,
            window: None,
            current_parent: None,
            current_parent_class: String::new(),
            cached_workerw: None,
            cached_alive: None,
            log_path: diag_log_path(),
            events: diag_events(),
        }
    }
}

/// 尝试把窗口挂到桌面 WorkerW；成功返回按 WorkerW 客户区换算的相对坐标
/// （调用方用该坐标 `set_position`）；失败返回 `None`（调用方回落压底）。
pub fn attach_to_desktop(win: &WebviewWindow, screen_x: i32, screen_y: i32) -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    {
        return imp::attach_to_desktop(win, screen_x, screen_y);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (win, screen_x, screen_y);
        None
    }
}

/// 把窗口从桌面层脱钩回顶层窗口（交互模式进入时调用）；已是顶层时为空操作。
/// 脱钩后位置用脱钩前的屏幕矩形原地保留，返回是否实际执行了脱钩。
pub fn detach_to_top(win: &WebviewWindow) -> bool {
    #[cfg(target_os = "windows")]
    {
        return imp::detach_to_top(win);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = win;
        false
    }
}

/// 确保窗口挂在桌面层：已挂在有效 WorkerW 下则不动；否则按窗口当前屏幕
/// 矩形重挂。返回最终是否处于挂靠态（false 时调用方回落压底）。
pub fn ensure_attached(win: &WebviewWindow) -> bool {
    #[cfg(target_os = "windows")]
    {
        return imp::ensure_attached(win);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = win;
        false
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::Mutex;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use tauri::WebviewWindow;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, BOOL, HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetParent, GetWindowRect, IsWindow,
        SendMessageTimeoutW, SetParent, SetWindowPos, SMTO_ABORTIFHUNG, SMTO_NORMAL,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    /// windows 0.58 的 HWND 包的是 *mut c_void（非整数），空句柄用此常量。
    const NULL_HWND: HWND = HWND(std::ptr::null_mut());

    /// 句柄诊断显示（十六进制地址；仅用于日志/设置页比对，不含隐私）。
    fn addr(h: HWND) -> String {
        format!("0x{:x}", h.0 as usize)
    }

    /// 已发现的桌面 WorkerW 缓存（Explorer 重启后失效，`IsWindow` 校验）。
    /// 存整数地址而非 HWND：裸指针不是 Send，不能进 static Mutex。
    static CACHED_WORKERW: Mutex<Option<isize>> = Mutex::new(None);

    fn hwnd_of(win: &WebviewWindow) -> Option<HWND> {
        let handle = win.window_handle().ok()?;
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            Some(HWND(h.hwnd.get() as *mut std::ffi::c_void))
        } else {
            None
        }
    }

    /// UTF-16 空结尾串（不用 `w!` 宏，避开宏的版本差异）。
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetClassNameW(hwnd, &mut buf) };
        if n <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize])
    }

    struct EnumCtx {
        target: Option<HWND>,
        seen_workerw: usize,
        defview_host: bool,
        next_found: bool,
    }

    /// 枚举回调：找含 SHELLDLL_DefView 的 WorkerW，取其后的下一个 WorkerW。
    /// 计数与命中标志供诊断记录（失败时能区分“没枚举到 / 没 DefView /
    /// 有 DefView 但无下层兄弟”三种死法）。
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut EnumCtx);
        if ctx.target.is_some() {
            return BOOL(0);
        }
        if class_name(hwnd) == "WorkerW" {
            ctx.seen_workerw += 1;
            let defview = wide("SHELLDLL_DefView");
            let workerw = wide("WorkerW");
            let null_str = PCWSTR::from_raw(std::ptr::null());
            let found = FindWindowExW(
                hwnd,
                NULL_HWND,
                PCWSTR::from_raw(defview.as_ptr()),
                null_str,
            );
            if let Ok(dv) = found {
                if dv != NULL_HWND {
                    ctx.defview_host = true;
                    if let Ok(next) = FindWindowExW(
                        NULL_HWND,
                        hwnd,
                        PCWSTR::from_raw(workerw.as_ptr()),
                        null_str,
                    ) {
                        if next != NULL_HWND {
                            ctx.next_found = true;
                            ctx.target = Some(next);
                            return BOOL(0);
                        }
                    }
                }
            }
        }
        BOOL(1)
    }

    /// 发现桌面 WorkerW：Progman 发 0x052C 生成 + 枚举定位目标层。
    fn discover_workerw() -> Option<HWND> {
        unsafe {
            let mut progman_s = "无".to_string();
            let mut sent = false;
            let progman_cls = wide("Progman");
            if let Ok(progman) = FindWindowW(
                PCWSTR::from_raw(progman_cls.as_ptr()),
                PCWSTR::from_raw(std::ptr::null()),
            ) {
                if progman != NULL_HWND {
                    progman_s = addr(progman);
                    // Explorer 忙时 1s 超时即放弃等待，仍继续枚举
                    // （WorkerW 可能已由上次生成）。
                    let mut _result: usize = 0;
                    let _ = SendMessageTimeoutW(
                        progman,
                        0x052C,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_NORMAL | SMTO_ABORTIFHUNG,
                        1000,
                        Some(&mut _result as *mut usize),
                    );
                    sent = true;
                }
            }
            let mut ctx = EnumCtx {
                target: None,
                seen_workerw: 0,
                defview_host: false,
                next_found: false,
            };
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut EnumCtx as isize));
            let chosen = ctx.target.map(addr).unwrap_or_else(|| "无".to_string());
            super::record(
                "discover",
                format!(
                    "progman={} 已发0x052C={} 枚举WorkerW={} 含DefView={} 找到下层={} 选中={}",
                    progman_s, sent, ctx.seen_workerw, ctx.defview_host, ctx.next_found, chosen
                ),
            );
            ctx.target
        }
    }

    /// 取缓存的 WorkerW（校验存活，失效则重新发现；0x052C 只在此处触发）。
    fn cached_workerw() -> Option<HWND> {
        let mut guard = CACHED_WORKERW.lock().ok()?;
        if let Some(cached_addr) = *guard {
            let h = HWND(cached_addr as *mut std::ffi::c_void);
            if unsafe { IsWindow(h) }.0 != 0 {
                return Some(h);
            }
            *guard = None;
            super::record(
                "cache",
                format!("缓存 WorkerW {} 已失效，重新发现", addr(h)),
            );
        }
        let found = discover_workerw()?;
        *guard = Some(found.0 as isize);
        Some(found)
    }

    pub fn attach_to_desktop(
        win: &WebviewWindow,
        screen_x: i32,
        screen_y: i32,
    ) -> Option<(i32, i32)> {
        let Some(hwnd) = hwnd_of(win) else {
            super::record("attach", "失败：取窗口句柄失败".to_string());
            return None;
        };
        let Some(workerw) = cached_workerw() else {
            super::record(
                "attach",
                "失败：WorkerW 发现失败（见 discover 记录）".to_string(),
            );
            return None;
        };
        unsafe {
            // 已挂在同一 WorkerW 下：只换算坐标，不重复 SetParent。
            let already = GetParent(hwnd).ok().map(|p| p == workerw).unwrap_or(false);
            if !already {
                if let Err(e) = SetParent(hwnd, workerw) {
                    let code = GetLastError();
                    super::record(
                        "attach",
                        format!(
                            "失败：SetParent 到 {} 出错 {:?}，GetLastError={}",
                            addr(workerw),
                            e,
                            code.0
                        ),
                    );
                    return None;
                }
            }
            let mut rect = RECT::default();
            if GetWindowRect(workerw, &mut rect).is_err() {
                super::record(
                    "attach",
                    format!("失败：取 WorkerW {} 区域失败", addr(workerw)),
                );
                return None;
            }
            let rel_x = screen_x - rect.left;
            let rel_y = screen_y - rect.top;
            // SetParent 改 WS_POPUP→WS_CHILD 后须 FRAMECHANGED 刷新样式帧，
            // 否则首次挂靠可能仍按顶层窗口绘制。
            let _ = SetWindowPos(
                hwnd,
                NULL_HWND,
                rel_x,
                rel_y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            super::record(
                "attach",
                format!(
                    "成功：workerw={} 已挂靠={} 相对坐标=({}, {})",
                    addr(workerw),
                    already,
                    rel_x,
                    rel_y
                ),
            );
            Some((rel_x, rel_y))
        }
    }

    pub fn detach_to_top(win: &WebviewWindow) -> bool {
        let Some(hwnd) = hwnd_of(win) else {
            return false;
        };
        unsafe {
            let parented = match GetParent(hwnd) {
                Ok(p) => p != NULL_HWND,
                Err(_) => false,
            };
            if !parented {
                return false;
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return false;
            }
            if let Err(e) = SetParent(hwnd, NULL_HWND) {
                let code = GetLastError();
                super::record(
                    "detach",
                    format!(
                        "失败：SetParent 回顶层出错 {:?}，GetLastError={}",
                        e, code.0
                    ),
                );
                return false;
            }
            let _ = SetWindowPos(
                hwnd,
                NULL_HWND,
                rect.left,
                rect.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            super::record(
                "detach",
                format!(
                    "成功：已脱钩回顶层，原屏幕矩形=({}, {}, {}, {})",
                    rect.left, rect.top, rect.right, rect.bottom
                ),
            );
            true
        }
    }

    pub fn ensure_attached(win: &WebviewWindow) -> bool {
        let Some(hwnd) = hwnd_of(win) else {
            return false;
        };
        let cached = CACHED_WORKERW.lock().ok().and_then(|g| *g);
        if let Some(cached_addr) = cached {
            let w = HWND(cached_addr as *mut std::ffi::c_void);
            let alive = unsafe { IsWindow(w) }.0 != 0;
            if alive {
                let parented = unsafe { GetParent(hwnd) }
                    .ok()
                    .map(|p| p == w)
                    .unwrap_or(false);
                if parented {
                    super::record("ensure", format!("仍挂靠在 {}，不动", addr(w)));
                    return true;
                }
            }
        }
        // 未挂靠或宿主已死：按窗口当前屏幕矩形重挂
        // （子窗口的 GetWindowRect 本就是屏幕坐标，可直接用）。
        // 重挂结果由 attach_to_desktop 自行记录，此处不再重复。
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return false;
        }
        attach_to_desktop(win, rect.left, rect.top).is_some()
    }

    /// 诊断快照：现时父窗口句柄 + 类名（定性关键）+ 缓存 WorkerW 存活。
    pub fn diag_snapshot(win: &WebviewWindow) -> super::PinDiag {
        let (window, parent, parent_class) = match hwnd_of(win) {
            Some(hwnd) => {
                let (p, cls) = unsafe {
                    match GetParent(hwnd) {
                        Ok(ph) if ph != NULL_HWND => (Some(addr(ph)), class_name(ph)),
                        _ => (None, String::new()),
                    }
                };
                (Some(addr(hwnd)), p, cls)
            }
            None => (None, None, String::new()),
        };
        let (cached, alive) = match CACHED_WORKERW.lock().ok().and_then(|g| *g) {
            Some(a) => {
                let w = HWND(a as *mut std::ffi::c_void);
                let al = unsafe { IsWindow(w) }.0 != 0;
                (Some(addr(w)), Some(al))
            }
            None => (None, None),
        };
        super::PinDiag {
            supported: true,
            window,
            current_parent: parent,
            current_parent_class: parent_class,
            cached_workerw: cached,
            cached_alive: alive,
            log_path: super::diag_log_path(),
            events: super::diag_events(),
        }
    }
}
