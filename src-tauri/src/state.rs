//! 应用状态：配置与缓存的内存态 + JSON 持久化 + 数据源实例管理。
//!
//! - 配置：`app_data_dir/config.json`（TECH-DESIGN §8）
//! - 缓存：`app_data_dir/cache.json`（covered_week、事件、已通知 id 集合，§4.2 对账依据）
//! - 数据源：`Jin10`（主）/ `Faireconomy`（备）两路实例（v0.2 起），
//!   配置变更时重建。

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use caldata::faireconomy::Faireconomy;
use caldata::jin10::Jin10;
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
fn default_panel_width_pct() -> u8 {
    42
}
fn default_width_mode() -> String {
    "auto".to_string()
}
fn default_manual_columns() -> u8 {
    3
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
    /// 开机自启动，默认开（清单 #9a；v0.2 前为无条件强制自启、无开关）
    #[serde(default = "default_true")]
    pub autostart: bool,
    /// 数据源："auto" | "primary"（v0.2 起；旧值 "backup" 已随 Investing
    /// 删除而失效，to_source_mode 会把它归一到 Auto，validate 仍放行以
    /// 兼容老配置，设置页保存时写回新值）
    #[serde(default = "default_source")]
    pub source: String,
    /// 美元口径利多/利空着色（设置可选项），默认关闭（默认用中性预期差着色）
    #[serde(default = "default_false")]
    pub usd_mode: bool,
    /// 事件类（讲话/纪要等）通知，默认关闭（数据行通知不受影响）
    #[serde(default = "default_false")]
    pub notify_events: bool,
    /// 隐藏假期行，默认关闭
    #[serde(default = "default_false")]
    pub hide_holidays: bool,
    /// 面板宽度占屏幕宽度的百分比，25–75，默认 42（v0.2 起退役：
    /// 宽度改由 width_mode/manual_columns 决定，本字段仅保留反序列化兼容）
    #[serde(default = "default_panel_width_pct")]
    pub panel_width_pct: u8,
    /// 宽度模式："auto"（按内容列数自适应，默认）| "manual"（固定列数）
    #[serde(default = "default_width_mode")]
    pub width_mode: String,
    /// 手动模式列数 1–6，默认 3
    #[serde(default = "default_manual_columns")]
    pub manual_columns: u8,
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
            autostart: default_true(),
            source: default_source(),
            usd_mode: default_false(),
            notify_events: default_false(),
            hide_holidays: default_false(),
            panel_width_pct: default_panel_width_pct(),
            width_mode: default_width_mode(),
            manual_columns: default_manual_columns(),
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
        if !["auto", "manual"].contains(&self.width_mode.as_str()) {
            return Err("width_mode 只能是 auto / manual".to_string());
        }
        if !(1..=6).contains(&self.manual_columns) {
            return Err("manual_columns 必须在 1–6 之间".to_string());
        }
        if self.frost_opacity > 100 {
            return Err("frost_opacity 必须在 0–100 之间".to_string());
        }
        self.colors.validate()?;
        Ok(())
    }

    /// 配置字符串 → caldata 的数据源模式。
    /// 旧值 "backup"（Investing 已删）归一到 Auto（清单 #8 兼容迁移）。
    pub fn to_source_mode(&self) -> SourceMode {
        match self.source.as_str() {
            "primary" => SourceMode::PrimaryOnly,
            _ => SourceMode::Auto,
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

/// 两路数据源实例（v0.2）：primary = 金十 WS（主），secondary = Faireconomy（备）。
pub struct Sources {
    pub primary: Jin10,
    pub secondary: Faireconomy,
}

impl Sources {
    pub fn build() -> Result<Self, SourceError> {
        Ok(Self {
            primary: Jin10::new()?,
            secondary: Faireconomy::new()?,
        })
    }
}

/// Tauri State：内存态 + 持久化目录 + 数据源实例 + 唤醒表重建信号
pub struct AppState {
    dir: PathBuf,
    config: RwLock<Config>,
    /// 最后落盘的配置（清单 #13）：预览只改 `config`（内存），设置窗
    /// 关闭时与它对比决定是否回滚
    saved_config: RwLock<Config>,
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
        let saved_config = config.clone();
        let cache = read_json::<Cache>(&dir.join(CACHE_FILE));
        let sources = Arc::new(Sources::build()?);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        Ok((
            Self {
                dir,
                config: RwLock::new(config),
                saved_config: RwLock::new(saved_config),
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

    /// 仅内存生效（设置页预览，清单 #7）：换配置 + 重建数据源，**不落盘**。
    /// 「应用」走 save_config；未应用的预览由窗口销毁时的
    /// rollback_preview 回滚（清单 #13，JS 快照回滚已删除）。
    pub fn apply_config(&self, config: Config) -> Result<(), String> {
        let sources = Arc::new(
            Sources::build()
                .map_err(|e| format!("数据源初始化失败: {e}"))?,
        );
        *self.config.write().unwrap() = config;
        *self.sources.write().unwrap() = sources;
        Ok(())
    }

    /// 落盘保存：写 config.json + 内存生效（顺序与旧实现一致：
    /// 先构建数据源、先写盘、再换内存，避免失败时盘/内存不一致扩大）。
    pub fn save_config(&self, config: Config) -> Result<(), String> {
        let sources = Arc::new(
            Sources::build()
                .map_err(|e| format!("数据源初始化失败: {e}"))?,
        );
        write_json(&self.dir.join(CONFIG_FILE), &config)?;
        *self.saved_config.write().unwrap() = config.clone();
        *self.config.write().unwrap() = config;
        *self.sources.write().unwrap() = sources;
        Ok(())
    }

    /// 设置窗关闭回滚（清单 #13）：预览只改内存配置；窗口销毁后若内存
    /// 配置与最后落盘配置不一致，恢复落盘配置并返回 true（调用方据此
    /// 重定位主窗 + 广播刷新）；一致返回 false，关窗零副作用。
    /// 比较走序列化（Config 未派生 PartialEq；字段顺序稳定可比）。
    pub fn rollback_preview(&self) -> bool {
        let saved = self.saved_config.read().unwrap().clone();
        let live = self.config_snapshot();
        let same = serde_json::to_string(&saved).ok() == serde_json::to_string(&live).ok();
        if same {
            return false;
        }
        match self.apply_config(saved) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[state] 设置关闭回滚预览失败: {e}");
                false
            }
        }
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
                        // 回填后重算 kind：首拉时无任何数值的指标会被判为 Event，
                        // actual 到位后应升为 Data，否则前端按事件行渲染、实际值不可见
                        // （FF 路径同款修复，2026-10-23 金十路径补齐）
                        cached.kind = caldata::mapping::classify_kind(
                            &cached.title,
                            cached.forecast.as_deref(),
                            cached.previous.as_deref(),
                            cached.actual.as_deref(),
                        );
                        newly.push(cached.clone());
                    }
                }
            }
            if !newly.is_empty() {
                // 回填也是数据更新："数据截至"（清单 #12）须跟着走，
                // 否则刚公布完页脚还显示旧时间
                cache.updated_at = Some(crate::now_sh().format("%Y-%m-%d %H:%M:%S").to_string());
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

    /// 对外落盘入口（scheduler 的 FF 回填分支用，QA M5）
    pub fn persist_cache_now(&self) {
        self.persist_cache();
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
