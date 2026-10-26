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

use tauri::WebviewWindow;

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
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetParent, GetWindowRect, IsWindow,
        SendMessageTimeoutW, SetParent, SetWindowPos, SMTO_ABORTIFHUNG, SMTO_NORMAL,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    /// 已发现的桌面 WorkerW 缓存（Explorer 重启后失效，`IsWindow` 校验）。
    static CACHED_WORKERW: Mutex<Option<HWND>> = Mutex::new(None);

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
    }

    /// 枚举回调：找含 SHELLDLL_DefView 的 WorkerW，取其后的下一个 WorkerW。
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut EnumCtx);
        if ctx.target.is_some() {
            return BOOL(0);
        }
        if class_name(hwnd) == "WorkerW" {
            let defview = wide("SHELLDLL_DefView");
            let workerw = wide("WorkerW");
            let null_str = PCWSTR::from_raw(std::ptr::null());
            let found = FindWindowExW(hwnd, HWND(0), PCWSTR::from_raw(defview.as_ptr()), null_str);
            if let Ok(dv) = found {
                if dv != HWND(0) {
                    if let Ok(next) =
                        FindWindowExW(HWND(0), hwnd, PCWSTR::from_raw(workerw.as_ptr()), null_str)
                    {
                        if next != HWND(0) {
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
            let progman_cls = wide("Progman");
            if let Ok(progman) = FindWindowW(
                PCWSTR::from_raw(progman_cls.as_ptr()),
                PCWSTR::from_raw(std::ptr::null()),
            ) {
                if progman != HWND(0) {
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
                }
            }
            let mut ctx = EnumCtx { target: None };
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut EnumCtx as isize));
            ctx.target
        }
    }

    /// 取缓存的 WorkerW（校验存活，失效则重新发现；0x052C 只在此处触发）。
    fn cached_workerw() -> Option<HWND> {
        let mut guard = CACHED_WORKERW.lock().ok()?;
        if let Some(h) = *guard {
            if unsafe { IsWindow(h) }.0 != 0 {
                return Some(h);
            }
            *guard = None;
        }
        let found = discover_workerw()?;
        *guard = Some(found);
        Some(found)
    }

    pub fn attach_to_desktop(
        win: &WebviewWindow,
        screen_x: i32,
        screen_y: i32,
    ) -> Option<(i32, i32)> {
        let hwnd = hwnd_of(win)?;
        let workerw = cached_workerw()?;
        unsafe {
            // 已挂在同一 WorkerW 下：只换算坐标，不重复 SetParent。
            let already = GetParent(hwnd).ok().map(|p| p == workerw).unwrap_or(false);
            if !already {
                SetParent(hwnd, workerw).ok()?;
            }
            let mut rect = RECT::default();
            GetWindowRect(workerw, &mut rect).ok()?;
            let rel_x = screen_x - rect.left;
            let rel_y = screen_y - rect.top;
            // SetParent 改 WS_POPUP→WS_CHILD 后须 FRAMECHANGED 刷新样式帧，
            // 否则首次挂靠可能仍按顶层窗口绘制。
            let _ = SetWindowPos(
                hwnd,
                HWND(0),
                rel_x,
                rel_y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
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
                Ok(p) => p != HWND(0),
                Err(_) => false,
            };
            if !parented {
                return false;
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return false;
            }
            if SetParent(hwnd, HWND(0)).is_err() {
                return false;
            }
            let _ = SetWindowPos(
                hwnd,
                HWND(0),
                rect.left,
                rect.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            true
        }
    }

    pub fn ensure_attached(win: &WebviewWindow) -> bool {
        let Some(hwnd) = hwnd_of(win) else {
            return false;
        };
        let cached = CACHED_WORKERW.lock().ok().and_then(|g| *g);
        if let Some(w) = cached {
            let alive = unsafe { IsWindow(w) }.0 != 0;
            if alive {
                let parented = unsafe { GetParent(hwnd) }
                    .ok()
                    .map(|p| p == w)
                    .unwrap_or(false);
                if parented {
                    return true;
                }
            }
        }
        // 未挂靠或宿主已死：按窗口当前屏幕矩形重挂
        // （子窗口的 GetWindowRect 本就是屏幕坐标，可直接用）。
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return false;
        }
        attach_to_desktop(win, rect.left, rect.top).is_some()
    }
}
