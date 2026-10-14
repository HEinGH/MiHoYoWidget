//! 配置读写（JSON，原子写）
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub games: Vec<String>,
    pub auto_checkin: bool,
    /// 签到域自动刷新间隔（秒）。**只作用于签到域**（api-takumi，无限流风控）。
    /// 下限 `MIN_REFRESH_INTERVAL_SECS`（15 分钟），`load`/`update` 内强制夹取。
    /// ⚠️ 战绩域（每日便签）自 2026-10-14 起为「**仅手动刷新**」，不受本配置影响。
    pub refresh_interval: u64,
    pub auto_start: bool,
    pub widget_position: Option<WidgetPosition>,
    pub transparency: f64,
    /// 字体大小档位（倍数，1.0=最小档；可选档位受窗口宽度限制，见前端 FONT_LEVELS）
    pub font_scale: f64,
    /// 设置页"手动粘贴 Cookie"开关（默认关闭，优先扫码）
    pub allow_manual_cookie: bool,
    /// 米游社 v2 会话绑定的设备标识（x-rpc-device_id）。
    /// 扫码登录时 WebView 产生 _MHYUUID，需持久化复用，否则 v2 cookie 被判登录失效(-100)
    pub device_id: String,
    /// 「原神便签」开关（v1.3.0 起默认 **false = 冻结**）。
    ///
    /// 冻结原因（2026-10-14 实测坐实）：米游社现已要求**「App 身份」会话**才能读取原神
    /// `dailyNote`——本应用的登录走米哈游通行证（网页身份），请求恒被 `1034`/`5003` 挡下，
    /// 且 `verifyVerification` 恒 `-1`（设备/会话上下文不足，双通道实测）。
    /// 继续请求只是白白暴露风控面，故默认冻结。
    ///
    /// 开启后：便签卡才对原神发起战绩域请求（**仍遵循"仅手动刷新"铁律**）。
    /// 建议仅在已导入「App 形式会话」（成套搬运 cookie + 移动端 device_id + device_fp）后再开启。
    pub genshin_note_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetPosition {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            games: vec!["genshin".into(), "zzz".into()],
            auto_checkin: true,
            refresh_interval: DEFAULT_REFRESH_INTERVAL_SECS,
            auto_start: true,
            widget_position: None,
            transparency: 0.9,
            font_scale: 1.0,
            allow_manual_cookie: false,
            device_id: String::new(),
            // v1.3.0：默认冻结原神便签（米游社要求 App 身份会话，网页身份必被挡）
            genshin_note_enabled: false,
        }
    }
}

pub fn config_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("config.json")
}

/// 签到域自动刷新间隔下限（秒）= 15 分钟。
/// ⚠️ **不要调低**：签到域虽无滑动窗口风控，但有 `-500004 请求过于频繁` 限流；
///    且历史上"5 分钟刷新"正是 1034 风控的直接成因（当时刷新还包含战绩域便签）。
///    下限值即纪律值——此常量是"假配置"治理的落点。
pub const MIN_REFRESH_INTERVAL_SECS: u64 = 900;
pub const DEFAULT_REFRESH_INTERVAL_SECS: u64 = 900;

pub fn load(app: &AppHandle) -> Config {
    let path = config_path(app);
    let mut cfg = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    };
    // 兜底夹取：历史 config.json 里存的是被淘汰的 300s（5 分钟）默认值
    if cfg.refresh_interval < MIN_REFRESH_INTERVAL_SECS {
        cfg.refresh_interval = DEFAULT_REFRESH_INTERVAL_SECS;
    }
    cfg
}

/// 原子写：临时文件 + rename，避免写一半损坏
pub fn save(app: &AppHandle, cfg: &Config) -> std::io::Result<()> {
    let path = config_path(app);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn update(app: &AppHandle, key: &str, value: serde_json::Value) -> Config {    let mut cfg = load(app);
    // 仅支持白名单键（防御未知键污染）
    match key {
        "games" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.games = v;
            }
        }
        "auto_checkin" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.auto_checkin = v;
            }
        }
        "auto_start" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.auto_start = v;
            }
        }
        "transparency" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.transparency = v;
            }
        }
        "font_scale" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.font_scale = v;
            }
        }
        "refresh_interval" => {
            if let Ok(v) = serde_json::from_value::<u64>(value) {
                // 强制下限：不允许把签到域刷新设得比纪律值更频繁
                cfg.refresh_interval = v.max(MIN_REFRESH_INTERVAL_SECS);
            }
        }
        "widget_position" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.widget_position = v;
            }
        }
        "allow_manual_cookie" => {
            if let Ok(v) = serde_json::from_value(value) {
                cfg.allow_manual_cookie = v;
            }
        }
        "genshin_note_enabled" => {
            if let Ok(v) = serde_json::from_value::<bool>(value) {
                if v != cfg.genshin_note_enabled {
                    crate::debug_log::log(&format!(
                        "config: 原神便签开关 {} → {}（开启后便签卡可对原神发起战绩域请求）",
                        cfg.genshin_note_enabled, v
                    ));
                }
                cfg.genshin_note_enabled = v;
            }
        }
        _ => {}
    }
    let _ = save(app, &cfg);
    cfg
}

/// 获取持久化的设备标识（x-rpc-device_id）。
/// 首次调用时生成固定 uuid 并保存；扫码登录成功后会由 _MHYUUID 覆盖，
/// 保证与米游社 v2 会话绑定的设备一致。
pub fn device_id(app: &AppHandle) -> String {
    let cfg = load(app);
    if !cfg.device_id.is_empty() {
        return cfg.device_id;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut cfg = load(app);
    cfg.device_id = id.clone();
    let _ = save(app, &cfg);
    id
}

/// 用扫码登录窗口的设备 uuid（_MHYUUID）覆盖 device_id，
/// 使后续 API 请求与 v2 会话绑定同一设备。
pub fn set_device_id(app: &AppHandle, id: &str) {
    let mut cfg = load(app);
    if cfg.device_id != id {
        cfg.device_id = id.to_string();
        let _ = save(app, &cfg);
    }
}
