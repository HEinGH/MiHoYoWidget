//! Cookie 存取（DPAPI 加密 JSON）+ 老版 cookies.enc 无感迁移
use crate::crypto::dpapi;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// 登录所需全部 cookie 字段（与老版 TargetCookieNames 对齐，含 v2）
/// 当前由 Cookies 结构体字段承载，此常量保留作字段清单文档
#[allow(dead_code)]
pub const COOKIE_FIELDS: &[&str] = &[
    "ltoken",
    "ltoken_v2",
    "ltuid",
    "ltuid_v2",
    "cookie_token",
    "cookie_token_v2",
    "stoken",
    "stoken_v2",
    "account_id",
    "account_id_v2",
    "mid",
    "login_ticket",
    "device_fp",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cookies {
    pub ltoken: String,
    pub ltoken_v2: String,
    pub ltuid: String,
    pub ltuid_v2: String,
    pub cookie_token: String,
    pub cookie_token_v2: String,
    pub stoken: String,
    pub stoken_v2: String,
    pub account_id: String,
    pub account_id_v2: String,
    pub mid: String,
    /// 网页登录时种下的 HttpOnly cookie，用于换取 stoken（getMultiTokenByLoginTicket）
    pub login_ticket: String,
    /// 设备指纹（DEVICEFP cookie，扫码时网页生成）：请求头 x-rpc-device_fp 用。
    /// 实测：绝区零便签(event 域)不带此头会触发设备风控 retcode=10041。
    pub device_fp: String,
}

impl Cookies {
    pub fn is_empty(&self) -> bool {
        self.ltoken.is_empty() && self.ltoken_v2.is_empty() && self.cookie_token.is_empty()
    }

    pub fn from_map(map: &HashMap<String, String>) -> Self {
        Self {
            ltoken: map.get("ltoken").cloned().unwrap_or_default(),
            ltoken_v2: map.get("ltoken_v2").cloned().unwrap_or_default(),
            ltuid: map.get("ltuid").cloned().unwrap_or_default(),
            ltuid_v2: map.get("ltuid_v2").cloned().unwrap_or_default(),
            cookie_token: map.get("cookie_token").cloned().unwrap_or_default(),
            cookie_token_v2: map.get("cookie_token_v2").cloned().unwrap_or_default(),
            stoken: map.get("stoken").cloned().unwrap_or_default(),
            stoken_v2: map.get("stoken_v2").cloned().unwrap_or_default(),
            account_id: map.get("account_id").cloned().unwrap_or_default(),
            account_id_v2: map.get("account_id_v2").cloned().unwrap_or_default(),
            mid: map
                .get("account_mid_v2")
                .or_else(|| map.get("ltmid_v2"))
                .or_else(|| map.get("mid"))
                .cloned()
                .unwrap_or_default(),
            login_ticket: map.get("login_ticket").cloned().unwrap_or_default(),
            device_fp: map.get("DEVICEFP").cloned().unwrap_or_default(),
        }
    }

    /// 转请求 Cookie 映射。
    /// ⚠️ 米游社鉴权陷阱（实测）：
    /// - v2 会话（ltoken_v2 非空）时若混带 v1 字段（ltuid/account_id），服务器按 v1 校验、
    ///   而 v1 凭证为空 → 必然 -100 登录失效，故 v2 分支**只输出 v2 字段**；
    /// - v2 会话的 mid 字段名必须是 account_mid_v2 / ltmid_v2（服务器不认 `mid`）；
    /// - device_id 必须固定且与扫码设备(_MHYUUID)一致，否则 v2 会话被判失效。
    pub fn to_map(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        if !self.ltoken_v2.is_empty() {
            // —— v2 会话：只输出 v2 字段 ——
            let v2_entries = [
                ("ltoken_v2", &self.ltoken_v2),
                ("ltuid_v2", &self.ltuid_v2),
                ("cookie_token_v2", &self.cookie_token_v2),
                ("stoken_v2", &self.stoken_v2),
                ("account_id_v2", &self.account_id_v2),
            ];
            for (k, v) in v2_entries {
                if !v.is_empty() {
                    m.insert(k.to_string(), v.clone());
                }
            }
            if !self.mid.is_empty() {
                m.insert("account_mid_v2".to_string(), self.mid.clone());
                m.insert("ltmid_v2".to_string(), self.mid.clone());
            }
        } else {
            // —— v1 会话（手动粘贴老 cookie）：输出 v1 字段 ——
            let v1_entries = [
                ("ltoken", &self.ltoken),
                ("ltuid", &self.ltuid),
                ("cookie_token", &self.cookie_token),
                ("stoken", &self.stoken),
                ("account_id", &self.account_id),
                ("mid", &self.mid),
            ];
            for (k, v) in v1_entries {
                if !v.is_empty() {
                    m.insert(k.to_string(), v.clone());
                }
            }
        }
        m
    }
}

pub fn cookie_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("cookies.enc")
}

/// 老版数据目录（%APPDATA%\MiHoYoWidget\）
fn legacy_cookie_path() -> PathBuf {
    dirs_data() // %APPDATA%
        .join("MiHoYoWidget")
        .join("cookies.enc")
}

fn dirs_data() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn encrypt_save(path: &Path, cookies: &Cookies) -> std::io::Result<()> {
    let json = serde_json::to_vec(cookies).map_err(std::io::Error::other)?;
    let enc = dpapi::encrypt(&json).map_err(std::io::Error::other)?;
    fs::write(path, enc)
}

fn decrypt_load(path: &Path) -> Option<Cookies> {
    let data = fs::read(path).ok()?;
    let plain = dpapi::decrypt(&data).ok()?;
    serde_json::from_slice(&plain).ok()
}

/// 加载 cookie；无新文件时尝试迁移老版 cookies.enc（用户无感）
pub fn load_with_migration(app: &AppHandle) -> Option<Cookies> {
    let new_path = cookie_path(app);
    if new_path.exists() {
        return decrypt_load(&new_path);
    }
    let legacy = legacy_cookie_path();
    if legacy.exists() {
        if let Some(c) = decrypt_load(&legacy) {
            // 迁移成功：写入新文件（保留老文件不删，避免误删用户数据）
            if let Some(dir) = new_path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = encrypt_save(&new_path, &c);
            return Some(c);
        }
    }
    None
}

pub fn save(app: &AppHandle, cookies: &Cookies) -> std::io::Result<()> {
    let path = cookie_path(app);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    encrypt_save(&path, cookies)
}

pub fn delete(app: &AppHandle) {
    let path = cookie_path(app);
    let _ = fs::remove_file(path);
    // ⚠️ 2026-08-30 修复：退出登录必须同时删除老版迁移源 cookies.enc。
    //    has() 判定是"新文件 ∨ 老文件"，若老迁移源残留 → 退出后 has_cookies 恒 true
    //    （前端误判已登录），且 api_from_app 会从遗留老文件"复活"旧登录
    //    （登录失效→重新登录的死循环）。用户主动退出=清除登录数据，含迁移源。
    let _ = fs::remove_file(legacy_cookie_path());
}

pub fn has(app: &AppHandle) -> bool {
    cookie_path(app).exists() || legacy_cookie_path().exists()
}
