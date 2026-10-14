//! x-rpc-challenge token 持久化（geetest 验证码换取的一次性凭证）
//!
//! 单次使用：`get_note` 请求前 `take`（读 + 删）并注入请求头；验证成功后由
//! `save_captcha_token` 写入。明文 JSON（非敏感凭证，仅为绕过米游社验证码保护），
//! 存于 config 目录 `challenge.json`。
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

fn challenge_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("challenge.json")
}

fn load_map(app: &AppHandle) -> HashMap<String, String> {
    let path = challenge_path(app);
    let Ok(data) = fs::read(&path) else {
        return HashMap::new();
    };
    let Ok(v) = serde_json::from_slice::<Value>(&data) else {
        return HashMap::new();
    };
    let mut m = HashMap::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                m.insert(k.clone(), s.to_string());
            }
        }
    }
    m
}

fn save_map(app: &AppHandle, m: &HashMap<String, String>) -> std::io::Result<()> {
    let path = challenge_path(app);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let obj: Value = m.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    fs::write(path, serde_json::to_vec_pretty(&obj).unwrap_or_default())
}

/// 读取并移除指定游戏的 challenge token（单次使用语义）
pub fn take(app: &AppHandle, game_id: &str) -> Option<String> {
    let mut m = load_map(app);
    let v = m.remove(game_id);
    if v.is_some() {
        let _ = save_map(app, &m);
    }
    v
}

/// 保存指定游戏的 challenge token
pub fn set(app: &AppHandle, game_id: &str, token: &str) -> std::io::Result<()> {
    let mut m = load_map(app);
    m.insert(game_id.to_string(), token.to_string());
    save_map(app, &m)
}

/// 清除指定游戏的 challenge token（验证失败/挑战作废时调用，避免残留脏 token）
#[allow(dead_code)]
pub fn clear(app: &AppHandle, game_id: &str) {
    let mut m = load_map(app);
    if m.remove(game_id).is_some() {
        let _ = save_map(app, &m);
    }
}
