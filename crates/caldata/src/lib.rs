//! 桌面财经日历挂件 — 数据层（`caldata`）
//!
//! 纯 Rust crate，**禁止依赖 Tauri**，可独立 `cargo test`。
//! 职责：从数据源拉取周经济日历 → 解析为统一内部模型
//! [`CalendarEvent`] → 国家/重要性/方向映射 → 刷新调度纯函数。
//!
//! 数据源（v0.2）：金十 WebSocket（主，`jin10`）+ Faireconomy 公开
//! JSON（备，`faireconomy`，命中后经 `forexfactory_actuals` 回填实际值）。
//! Investing 两路已于 v0.2 删除（Cloudflare 全拦、从未可用，清单 #8）。

pub mod faireconomy;
pub mod forexfactory_actuals;
pub mod jin10;
pub mod mapping;
pub mod model;
pub mod schedule;
pub mod source;

pub use mapping::{classify_kind, deviation, tentative_status, Deviation, Direction, Sentiment};
pub use model::{CalendarEvent, Country, EventKind, Source};
pub use schedule::{build_wake_list, needs_refresh, next_weekly_refresh, TZ};
pub use source::{fetch_week_with_fallback, DataSource, FetchOutcome, SourceError};
