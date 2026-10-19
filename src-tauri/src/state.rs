//! 应用状态：配置与缓存的内存态 + JSON 持久化 + 数据源实例管理。
//!
//! - 配置：`app_data_dir/config.json`（TECH-DESIGN §8）
//! - 缓存：`app_data_dir/cache.json`（covered_week、事件、已通知 id 集合，§4.2 对账依据）
//! - 数据源：`Faireconomy` / `InvestingBackup` 实例各持一份（复用 reqwest Client），
//!   配置变更（importance_map）时重建。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use caldata::faireconomy::Faireconomy;
use caldata::investing_backup::InvestingBackup;
use caldata::mapping::ImportanceMap;
use caldata::model::CalendarEvent;
use caldata::source::{SourceError, SourceMode};
use chrono::DateTime;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.json";
const CACHE_FILE: &str = "cache.json";
/// 已通知 id 集合上限（防无限增长）
const NOTIFIED_CAP: usize = 2000;

fn default_countries() -> Vec<String> {
    vec!["CN".to_string(), "US".to_string()]
}
fn default_min_stars() -> u8 {
    3
}
fn default_side() -> String {
    "right".to_string()
}
fn default_font_size() -> u8 {
    14
}
fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}
fn default_source() -> String {
    "auto".to_string()
}
/// Investing 3 档重要性 → 5 星映射（TECH-DESIGN §3.3），PRD 要求用一周真实数据校准。
/// JSON 中形如 {"1":1,"2":3,"3":5}。
fn default_importance_map() -> HashMap<u8, u8> {
    [(1u8, 1u8), (2u8, 3u8), (3u8, 5u8)].into_iter().collect()
}
fn default_panel_width_pct() -> u8 {
    42
}
fn default_frost_opacity() -> u8 {
    70
}
// 颜色主题默认值（与 styles.css :root 保持一致）
fn c_time() -> String { "#f0a832".to_string() }
fn c_stars() -> String { "#ffd166".to_string() }
fn c_beat() -> String { "#4ade80".to_string() }
fn c_miss() -> String { "#f87171".to_string() }
fn c_inline() -> String { "#9ca3af".to_string() }
fn c_text() -> String { "#eceff4".to_string() }
fn c_muted() -> String { "#9aa4b5".to_string() }

/// 颜色主题：挂件元素颜色自定义（设置页取色器，v1.1）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorTheme {
    #[serde(default = "c_time")]
    pub time: String,
    #[serde(default = "c_stars")]
    pub stars: String,
    #[serde(default = "c_beat")]
    pub beat: String,
    #[serde(default = "c_miss")]
    pub miss: String,
    #[serde(default = "c_inline")]
    pub inline: String,
    #[serde(default = "c_text")]
    pub text: String,
    #[serde(default = "c_muted")]
    pub muted: String,
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self {
            time: c_time(), stars: c_stars(), beat: c_beat(), miss: c_miss(),
            inline: c_inline(), text: c_text(), muted: c_muted(),
        }
    }
}

impl ColorTheme {
    /// 校验：必须为 #rrggbb 格式
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("time", &self.time), ("stars", &self.stars), ("beat", &self.beat),
            ("miss", &self.miss), ("inline", &self.inline),
            ("text", &self.text), ("muted", &self.muted),
        ] {
            let b = v.as_bytes();
            if b.len() != 7 || b[0] != b'#'
                || !b[1..].iter().all(|c| c.is_ascii_hexdigit())
            {
                return Err(format!("颜色 {name} 须为 #rrggbb 格式，当前为 {v}"));
            }
        }
        Ok(())
    }
}

/// 挂件配置（设置页可调，TECH-DESIGN §8）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 关注国家代码，如 ["CN","US"]（与 `caldata::model::Country::code()` 对照）
    #[serde(default = "default_countries")]
    pub countries: Vec<String>,
    /// 显示星级阈值 1–5，默认 3
    #[serde(default = "default_min_stars")]
    pub min_stars: u8,
    /// 挂件位于左/右半屏："left" | "right"
    #[serde(default = "default_side")]
    pub side: String,
    /// 字号 px，12–18
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    /// 毛玻璃效果（仅面板区域），默认开
    #[serde(default = "default_true")]
    pub frosted: bool,
    /// 数据公布系统通知，默认开
    #[serde(default = "default_true")]
    pub notifications: bool,
    /// 数据源："auto" | "primary" | "backup"
    #[serde(default = "default_source")]
    pub source: String,
    /// 重要性 3 档 → 5 星映射表，可配置；本壳在构造数据源时转换为
    /// `caldata::mapping::ImportanceMap` 经 `with_importance()` 注入。
    #[serde(default = "default_importance_map")]
    pub importance_map: HashMap<u8, u8>,
    /// 美元口径利多/利空着色（设置可选项），默认关闭（默认用中性预期差着色）
    #[serde(default = "default_false")]
    pub usd_mode: bool,
    /// 事件类（讲话/纪要等）通知，默认关闭（数据行通知不受影响）
    #[serde(default = "default_false")]
    pub notify_events: bool,
    /// 隐藏假期行，默认关闭
    #[serde(default = "default_false")]
    pub hide_holidays: bool,
    /// 面板宽度占屏幕宽度的百分比，25–75，默认 42（设置页滑杆可调）
    #[serde(default = "default_panel_width_pct")]
    pub panel_width_pct: u8,
    /// 毛玻璃背景不透明度 0–100，默认 70（设置页滑杆可调）
    #[serde(default = "default_frost_opacity")]
    pub frost_opacity: u8,
    /// 颜色主题（元素颜色自定义）
    #[serde(default)]
    pub colors: ColorTheme,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            countries: default_countries(),
            min_stars: default_min_stars(),
            side: default_side(),
            font_size: default_font_size(),
            frosted: default_true(),
            notifications: default_true(),
            source: default_source(),
            importance_map: default_importance_map(),
            usd_mode: default_false(),
            notify_events: default_false(),
            hide_holidays: default_false(),
            panel_width_pct: default_panel_width_pct(),
            frost_opacity: default_frost_opacity(),
            colors: ColorTheme::default(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.min_stars == 0 || self.min_stars > 5 {
            return Err("min_stars 必须在 1–5 之间".to_string());
        }
        if self.side != "left" && self.side != "right" {
            return Err("side 只能是 left 或 right".to_string());
        }
        if !(12..=18).contains(&self.font_size) {
            return Err("font_size 必须在 12–18 之间".to_string());
        }
        if !["auto", "primary", "backup"].contains(&self.source.as_str()) {
            return Err("source 只能是 auto / primary / backup".to_string());
        }
        if self.countries.is_empty() {
            return Err("countries 不能为空".to_string());
        }
        if !(25..=75).contains(&self.panel_width_pct) {
            return Err("panel_width_pct 必须在 25–75 之间".to_string());
        }
        if self.frost_opacity > 100 {
            return Err("frost_opacity 必须在 0–100 之间".to_string());
        }
        self.colors.validate()?;
        Ok(())
    }

    /// 配置字符串 → caldata 的数据源模式
    pub fn to_source_mode(&self) -> SourceMode {
        match self.source.as_str() {
            "primary" => SourceMode::PrimaryOnly,
            "backup" => SourceMode::BackupOnly,
            _ => SourceMode::Auto,
        }
    }

    /// 配置映射表 → caldata::mapping::ImportanceMap（缺键回退默认值）
    pub fn to_importance_map(&self) -> ImportanceMap {
        ImportanceMap {
            low: self.importance_map.get(&1).copied().unwrap_or(1),
            mid: self.importance_map.get(&2).copied().unwrap_or(3),
            high: self.importance_map.get(&3).copied().unwrap_or(5),
        }
    }
}

/// 本地缓存（TECH-DESIGN §4.2 对账依据）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cache {
    /// 已覆盖的周：(iso_year, iso_week)
    #[serde(default)]
    pub covered_week: Option<(i32, u32)>,
    #[serde(default)]
    pub events: Vec<CalendarEvent>,
    /// 已发送通知的事件 id（去重，重启不重发）
    #[serde(default)]
    pub notified_ids: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// 两路数据源实例（复用内部 reqwest Client）
/// primary = Faireconomy（公开 JSON，免反爬）；backup = Investing 社区接口。
pub struct Sources {
    pub primary: Faireconomy,
    pub backup: InvestingBackup,
}

impl Sources {
    pub fn build(importance: &ImportanceMap) -> Result<Self, SourceError> {
        Ok(Self {
            primary: Faireconomy::new()?,
            backup: InvestingBackup::new()?.with_importance(*importance),
        })
    }
}

/// Tauri State：内存态 + 持久化目录 + 数据源实例 + 唤醒表重建信号
pub struct AppState {
    dir: PathBuf,
    config: RwLock<Config>,
    cache: RwLock<Cache>,
    sources: RwLock<Arc<Sources>>,
    rebuild_tx: tokio::sync::mpsc::Sender<()>,
    /// 上次 actuals 扫尾尝试时间（用于失败退避，避免 FF 故障时 hammer）
    last_actuals_sweep: RwLock<Option<DateTime<Tz>>>,
}

impl AppState {
    /// 从 app_data_dir 加载（文件缺失/损坏 → 默认值）
    pub fn load(dir: PathBuf) -> Result<(Self, tokio::sync::mpsc::Receiver<()>), SourceError> {
        let config = read_json::<Config>(&dir.join(CONFIG_FILE));
        let cache = read_json::<Cache>(&dir.join(CACHE_FILE));
        let sources = Arc::new(Sources::build(&config.to_importance_map())?);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        Ok((
            Self {
                dir,
                config: RwLock::new(config),
                cache: RwLock::new(cache),
                sources: RwLock::new(sources),
                rebuild_tx: tx,
                last_actuals_sweep: RwLock::new(None),
            },
            rx,
        ))
    }

    /// 记录 actuals 扫尾尝试时间
    pub fn set_last_actuals_sweep(&self, t: DateTime<Tz>) {
        *self.last_actuals_sweep.write().unwrap() = Some(t);
    }

    /// 上次 actuals 扫尾尝试时间
    pub fn last_actuals_sweep(&self) -> Option<DateTime<Tz>> {
        *self.last_actuals_sweep.read().unwrap()
    }

    pub fn config_snapshot(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn cache_snapshot(&self) -> Cache {
        self.cache.read().unwrap().clone()
    }

    /// 可变访问缓存（事件驱动回填用）
    pub fn cache_write(&self) -> std::sync::RwLockWriteGuard<'_, Cache> {
        self.cache.write().unwrap()
    }

    /// 数据源实例（Arc 共享，调用方按需取引用）
    pub fn sources(&self) -> Arc<Sources> {
        self.sources.read().unwrap().clone()
    }

    /// 保存配置：先重建数据源（importance_map 可能变），再落盘 + 更新内存。
    /// 调用方须先完成 `Config::validate()`。
    pub fn save_config(&self, config: Config) -> Result<(), String> {
        let sources = Arc::new(
            Sources::build(&config.to_importance_map())
                .map_err(|e| format!("数据源初始化失败: {e}"))?,
        );
        write_json(&self.dir.join(CONFIG_FILE), &config)?;
        *self.config.write().unwrap() = config;
        *self.sources.write().unwrap() = sources;
        Ok(())
    }

    /// 全量刷新成功后替换事件集并记录 covered_week。
    /// 返回"相对旧缓存 newly 拿到 actual"的事件（供 notifier 发送通知）；
    /// 纯新增 id（首次拉取）不通知，避免首次启动刷屏。
    pub fn replace_events(
        &self,
        events: Vec<CalendarEvent>,
        covered_week: (i32, u32),
    ) -> Vec<CalendarEvent> {
        let mut newly = Vec::new();
        {
            let cache = self.cache.read().unwrap();
            for f in events.iter().filter(|e| e.actual.is_some()) {
                if let Some(old) = cache.events.iter().find(|e| e.id == f.id) {
                    if old.actual.is_none() {
                        newly.push(f.clone());
                    }
                }
            }
        }
        {
            let mut cache = self.cache.write().unwrap();
            cache.events = events;
            cache.covered_week = Some(covered_week);
            cache.updated_at = Some(crate::now_sh().format("%Y-%m-%d %H:%M:%S").to_string());
        }
        self.persist_cache();
        newly
    }

    /// 用新拉取的数据补 actual：仅当缓存中该事件 actual 为空时更新；
    /// 返回本次 newly 拿到 actual 的事件（供 notifier）。
    pub fn apply_actuals(&self, fresh: &[CalendarEvent]) -> Vec<CalendarEvent> {
        let mut newly = Vec::new();
        {
            let mut cache = self.cache.write().unwrap();
            for f in fresh {
                let Some(actual) = f.actual.as_ref() else {
                    continue;
                };
                if let Some(cached) = cache.events.iter_mut().find(|e| e.id == f.id) {
                    if cached.actual.is_none() {
                        cached.actual = Some(actual.clone());
                        newly.push(cached.clone());
                    }
                }
            }
        }
        if !newly.is_empty() {
            self.persist_cache();
        }
        newly
    }

    pub fn mark_notified(&self, ids: &[String]) {
        {
            let mut cache = self.cache.write().unwrap();
            for id in ids {
                if !cache.notified_ids.contains(id) {
                    cache.notified_ids.push(id.clone());
                }
            }
            // 防无限增长：只保留最近的
            if cache.notified_ids.len() > NOTIFIED_CAP {
                let excess = cache.notified_ids.len() - NOTIFIED_CAP;
                cache.notified_ids.drain(..excess);
            }
        }
        self.persist_cache();
    }

    pub fn set_last_error(&self, err: Option<String>) {
        {
            self.cache.write().unwrap().last_error = err;
        }
        self.persist_cache();
    }

    /// 通知 scheduler 重建唤醒时间表（channel 容量 1，信号可合并）
    pub fn request_rebuild(&self) {
        let _ = self.rebuild_tx.try_send(());
    }

    fn persist_cache(&self) {
        let cache = self.cache.read().unwrap();
        if let Err(e) = write_json(&self.dir.join(CACHE_FILE), &*cache) {
            eprintln!("[state] cache.json 写入失败: {e}");
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let s = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| e.to_string())
}
