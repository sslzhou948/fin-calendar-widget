//! 桌面财经日历挂件 — 数据层（`caldata`）
//!
//! 纯 Rust crate，**禁止依赖 Tauri**，可独立 `cargo test`。
//! 职责：从 Investing 接口拉取周经济日历 → 解析为统一内部模型
//! [`CalendarEvent`] → 国家/重要性/方向映射 → 刷新调度纯函数。
//!
//! ## 诚实边界（必读）
//! - `src/tests/fixtures/` 下的 HTML/JSON 样本均为**手写仿真样本**（文件头有标注），
//!   结构忠于社区文档与 POC 抓到的页面结构，但**未经 live 验证**。
//! - 国家数字 ID 映射仅 `5=美国` 经社区文档确认，其余标注 UNVERIFIED，
//!   须在开发环境从日历页面核对一次（见 [`mapping::DEFAULT_COUNTRY_IDS`]）。
//! - 重要性 3 档 → 5 星的默认映射 `{1:1, 2:3, 3:5}` 为初值，
//!   须用一周真实数据校准（见 [`mapping::ImportanceMap`] 注释）。

pub mod faireconomy;
pub mod forexfactory_actuals;
pub mod investing_backup;
pub mod investing_primary;
pub mod jin10;
pub mod mapping;
pub mod model;
pub mod schedule;
pub mod source;

pub use mapping::{
    classify_kind, deviation, tentative_status, Deviation, Direction, ImportanceMap, Sentiment,
};
pub use model::{CalendarEvent, Country, EventKind, Source};
pub use schedule::{build_wake_list, needs_refresh, next_sunday_midnight, TZ};
pub use source::{fetch_week_with_fallback, DataSource, FetchOutcome, SourceError};
