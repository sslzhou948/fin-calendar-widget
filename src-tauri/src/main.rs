//! FinCalendarWidget —— 桌面财经日历挂件入口。
//!
//! Windows release 构建时隐藏控制台窗口。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    fin_calendar_widget::run();
}
