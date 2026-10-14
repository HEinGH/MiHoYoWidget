//! 签到域 API：账号列表 / 签到信息 / 手动签到
//!
//! ⚠️ 2026-10-14 改造（A1 统一请求网关 + A2 短缓存）
//! 1. **所有请求经 `gateway::acquire(Domain::SignIn)`**：节流下沉到"请求本身"，
//!    不再依赖调用点。此前 `commands` 层与 `sign_in` 的内部复查询全部绕过
//!    `task::throttled()` → 实测每次启动 2 次请求间隔 113ms、单会话 2.36s 内 6 连发。
//! 2. **`get_sign_info` 同游戏 30s 短缓存**：消除"前端 refreshAll"与"调度器补签"
//!    在同一时刻并发请求同一账号的重复开销。
//!
//! ⚠️ 时序约定：**先 `gateway::acquire()`，再构造请求头，再 send**。
//!    `ds` 内含秒级时间戳，排在节流等待之后构造可避免等待期间时间戳变旧。
use crate::api::{find_by_biz, find_spec};
use crate::crypto::ds::{ds_v1, DS_SALT_CN_SIGNIN};
use crate::gateway::{self, Domain};
use crate::storage::cookies;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

pub const API_BASE: &str = "https://api-takumi.mihoyo.com";
pub const APP_VERSION: &str = "2.70.1";

/// 签到信息短缓存 TTL：同游戏在该窗口内的重复查询直接命中缓存（不发请求）
const SIGN_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub game_id: String,
    pub game_name: String,
    pub uid: String,
    pub nickname: String,
    pub server: String,
    pub level: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignInfo {
    pub signed: bool,
    pub total_days: i32,
    pub missed: i32,
}

#[derive(Debug, Clone)]
pub struct ApiError(pub String);

impl ApiError {
    /// 是否为网络/暂时性失败（DNS/连接/超时）—— 不等于登录失效。
    /// 前端据此区分"登录过期"与"暂时不可用"，避免误判导致主页面错显示"未登录"。
    pub fn is_network(&self) -> bool {
        self.0.starts_with("网络请求失败")
    }

    /// 是否为会话死亡（无 cookie 或服务端拒绝登录态）。
    /// 判据与前端 store.refresh() 的分类器保持一致（"未登录"精确 / "登录态不足"包含）。
    pub fn is_session_expired(&self) -> bool {
        self.0 == "未登录" || self.0.contains("登录态不足")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        ApiError(format!("网络请求失败: {e}"))
    }
}

#[derive(Debug, Clone)]
pub struct MihoyoApi {
    pub client: reqwest::Client,
    pub cookie_map: HashMap<String, String>,
    /// 固定设备标识：米游社 v2 会话绑定设备，随机 device_id 会被判登录失效(-100)
    pub device_id: String,
    /// 设备指纹（扫码时网页生成的 DEVICEFP）：event 活动域接口（zzz 便签）防设备风控 10041
    pub device_fp: String,
    /// 签到信息短缓存（key = `{game_id}|{uid}`）：同游戏 30s 内不重复请求。
    /// `Arc` 使 `Clone` 后的实例共享同一份缓存（`ApiState` 会克隆客户端）。
    sign_cache: Arc<Mutex<HashMap<String, (Instant, SignInfo)>>>,
}

impl MihoyoApi {
    pub fn new(app: &AppHandle, cookies: &cookies::Cookies) -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Linux; Android 12; SM-G9980) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36 miHoYoBBS/2.70.1")
                .build()
                .unwrap_or_default(),
            cookie_map: cookies.to_map(),
            device_id: crate::storage::config::device_id(app),
            device_fp: cookies.device_fp.clone(),
            sign_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 基础请求头（国服签到域，对齐 genshin.py daily.py）
    fn base_headers(&self, sign_game: &str) -> Result<HeaderMap, ApiError> {
        let mut h = HeaderMap::new();
        let cookie_str = self
            .cookie_map
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        h.insert(COOKIE, HeaderValue::from_str(&cookie_str).map_err(|e| ApiError(e.to_string()))?);
        h.insert("x-rpc-app_version", HeaderValue::from_static(APP_VERSION));
        h.insert("x-rpc-client_type", HeaderValue::from_static("5"));
        h.insert(
            "x-rpc-device_id",
            HeaderValue::from_str(&self.device_id).unwrap_or_else(|_| HeaderValue::from_static("00000000-0000-0000-0000-000000000000")),
        );
        h.insert("x-rpc-sys_version", HeaderValue::from_static("12"));
        h.insert("x-rpc-platform", HeaderValue::from_static("android"));
        h.insert("x-rpc-channel", HeaderValue::from_static("miyousheluodi"));
        // sign_game 来自内置注册表；from_str 失败会 panic，兜底为空串（不携带该头）
        h.insert(
            "x-rpc-signgame",
            HeaderValue::from_str(sign_game).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        // 设备指纹：event 域接口（zzz 便签）不带会触发设备风控 10041（实测）
        if !self.device_fp.is_empty() {
            h.insert(
                "x-rpc-device_fp",
                HeaderValue::from_str(&self.device_fp)
                    .unwrap_or_else(|_| HeaderValue::from_static("")),
            );
        }
        h.insert(
            "ds",
            HeaderValue::from_str(&ds_v1(DS_SALT_CN_SIGNIN))
                .unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        Ok(h)
    }

    /// 绑定游戏账号列表（getUserGameRolesByCookie）
    pub async fn get_game_accounts(&self) -> Result<Vec<Account>, ApiError> {
        gateway::acquire(Domain::SignIn).await;
        let headers = self.base_headers("hk4e")?;
        let resp = self
            .client
            .get(format!("{API_BASE}/binding/api/getUserGameRolesByCookie"))
            .headers(headers)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let retcode = resp.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        gateway::note_response(Domain::SignIn, "getUserGameRolesByCookie", Some(retcode));
        if retcode != 0 {
            let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("unknown");
            // ⚠️ 登录态失效（-100/-101 等）必须使用标准错误串"登录态不足，请重新扫码登录"：
            //    前端登录失效分类器（store.refresh）与 get_games/get_login_status 的
            //    is_session_expired() 都以此关键词判定会话死亡；其余错误（风控/参数等）
            //    不得包含该关键词，避免"单游戏/暂时性错误"误判为整个会话过期。
            if retcode == -100 || retcode == -101 {
                return Err(ApiError(format!("登录态不足，请重新扫码登录({retcode}): {msg}")));
            }
            return Err(ApiError(format!(
                "获取账号失败({retcode}): {msg}"
            )));
        }

        let mut accounts = Vec::new();
        if let Some(list) = resp.pointer("/data/list").and_then(|v| v.as_array()) {
            for item in list {
                let biz = item.get("game_biz").and_then(|v| v.as_str()).unwrap_or("");
                let Some(spec) = find_by_biz(biz) else { continue };
                accounts.push(Account {
                    game_id: spec.id.to_string(),
                    game_name: spec.name.to_string(),
                    uid: item.get("game_uid").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    nickname: item.get("nickname").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    server: item.get("region").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    level: item.get("level").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                });
            }
        }
        Ok(accounts)
    }

    fn cache_key(game_id: &str, uid: &str) -> String {
        format!("{game_id}|{uid}")
    }

    /// 清掉某游戏的签到缓存（签到成功后必须调用，否则会返回改动前的旧值）
    fn invalidate_sign_cache(&self, game_id: &str, uid: &str) {
        if let Ok(mut c) = self.sign_cache.lock() {
            c.remove(&Self::cache_key(game_id, uid));
        }
    }

    /// 签到信息（**缓存版**，供前端与调度器调用）：同游戏 30s 内直接命中缓存、不发请求。
    /// 服务端 `total_sign_day` 每月自动归零，为唯一权威；失败时保留旧值不覆盖。
    pub async fn get_sign_info(&self, game_id: &str, uid: &str, server: &str) -> Result<SignInfo, ApiError> {
        let key = Self::cache_key(game_id, uid);
        if let Ok(c) = self.sign_cache.lock() {
            if let Some((ts, info)) = c.get(&key) {
                if ts.elapsed() < SIGN_CACHE_TTL {
                    return Ok(info.clone());
                }
            }
        }
        let info = self.fetch_sign_info(game_id, uid, server).await?;
        if let Ok(mut c) = self.sign_cache.lock() {
            c.insert(key, (Instant::now(), info.clone()));
        }
        Ok(info)
    }

    /// 签到信息（**原始请求，不走缓存**）：`sign_in` 成功后用它拿改动后的最新值
    async fn fetch_sign_info(&self, game_id: &str, uid: &str, server: &str) -> Result<SignInfo, ApiError> {
        let spec = find_spec(game_id).ok_or_else(|| ApiError(format!("未注册游戏: {game_id}")))?;
        gateway::acquire(Domain::SignIn).await;
        let headers = self.base_headers(spec.sign_game_header)?;
        let resp = self
            .client
            .get(format!("{API_BASE}/event/luna/info"))
            .headers(headers)
            .query(&[
                ("act_id", spec.act_id),
                ("region", server),
                ("uid", uid),
            ])
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let retcode = resp.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        gateway::note_response(Domain::SignIn, "luna/info", Some(retcode));
        if retcode != 0 {
            let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("unknown");
            // 与 get_game_accounts 一致：登录态失效用标准关键词错误串
            if retcode == -100 || retcode == -101 {
                return Err(ApiError(format!("登录态不足，请重新扫码登录({retcode}): {msg}")));
            }
            return Err(ApiError(format!(
                "签到信息获取失败({retcode}): {msg}"
            )));
        }

        let data = resp.get("data").unwrap_or(&serde_json::Value::Null);
        Ok(SignInfo {
            signed: data.get("is_sign").and_then(|v| v.as_bool()).unwrap_or(false),
            total_days: data.get("total_sign_day").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            missed: data.get("sign_cnt_missed").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        })
    }

    /// 手动签到（sign 接口，幂等：已签到返回 0 成功）
    ///
    /// ⚠️ 成功后的收尾复查询（`fetch_sign_info`）**同样经网关节流**——
    ///    它是一次真实请求，历史上正因绕过调用点节流而成为盲区（实测 723ms 内连发）。
    pub async fn sign_in(&self, game_id: &str, uid: &str, server: &str) -> Result<SignInfo, ApiError> {
        let spec = find_spec(game_id).ok_or_else(|| ApiError(format!("未注册游戏: {game_id}")))?;
        gateway::acquire(Domain::SignIn).await;
        let headers = self.base_headers(spec.sign_game_header)?;
        let body = serde_json::json!({
            "act_id": spec.act_id,
            "region": server,
            "uid": uid,
        });
        let resp = self
            .client
            .post(format!("{API_BASE}/event/luna/sign"))
            .headers(headers)
            .json(&body)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let retcode = resp.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        gateway::note_response(Domain::SignIn, "luna/sign", Some(retcode));
        if retcode != 0 {
            return Err(ApiError(format!(
                "签到失败: {}",
                resp.get("message").and_then(|v| v.as_str()).unwrap_or("unknown")
            )));
        }
        // 签到成功后重新查询（避免 claim 后立即读的计数延迟问题：等 300ms）
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.invalidate_sign_cache(game_id, uid);
        self.fetch_sign_info(game_id, uid, server).await
    }

    /// 验证 cookie 有效性（真实调一次账号列表接口）。
    /// 返回 Result 以区分三种状态（前端登录判定必需）：
    /// - Ok(true)  = cookie 有效
    /// - Err(is_session_expired) = 服务端拒绝登录态（真过期）
    /// - Err(is_network) = 网络/暂时性失败（**不算失效**，不能据此判定"登录已过期"）
    pub async fn test_cookies(&self) -> Result<bool, ApiError> {
        self.get_game_accounts().await.map(|_| true)
    }

    /// 用 login_ticket 换取 stoken（getMultiTokenByLoginTicket）。
    /// 背景：event 活动域接口（绝区零便签）鉴权需 stoken，而网页扫码 cookie 不含 stoken；
    /// 旧接口 getSTokenByCookieToken 已下线(404)，此为社区现行方案。
    /// 注意：该接口无 DS 签名、无额外鉴权 header，仅凭 login_ticket + uid 即可。
    ///
    /// ⚠️ 实测 **v2 网页扫码不种 login_ticket**（login_ticket 是 v1 遗留凭证），
    ///    故主流程已不再调用此方法。仅当用户手动粘贴含 login_ticket 的 v1 cookie 时才有意义。
    #[allow(dead_code)]
    pub async fn fetch_stoken(&self, login_ticket: &str) -> Result<String, ApiError> {
        let uid = self
            .cookie_map
            .get("ltuid_v2")
            .or_else(|| self.cookie_map.get("ltuid"))
            .or_else(|| self.cookie_map.get("account_id_v2"))
            .or_else(|| self.cookie_map.get("account_id"))
            .cloned()
            .unwrap_or_default();
        if uid.is_empty() {
            return Err(ApiError("无 uid，无法换取 stoken".into()));
        }
        gateway::acquire(Domain::SignIn).await;
        let resp = self
            .client
            .get(format!("{API_BASE}/auth/api/getMultiTokenByLoginTicket"))
            .query(&[
                ("login_ticket", login_ticket),
                ("token_types", "3"),
                ("uid", &uid),
            ])
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let retcode = resp.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        gateway::note_response(Domain::SignIn, "getMultiTokenByLoginTicket", Some(retcode));
        if retcode != 0 {
            return Err(ApiError(format!(
                "换取 stoken 失败({retcode}): {}",
                resp.get("message").and_then(|v| v.as_str()).unwrap_or("unknown")
            )));
        }
        if let Some(list) = resp.pointer("/data/list").and_then(|v| v.as_array()) {
            for item in list {
                if item.get("name").and_then(|v| v.as_str()) == Some("stoken") {
                    if let Some(t) = item.get("token").and_then(|v| v.as_str()) {
                        return Ok(t.to_string());
                    }
                }
            }
        }
        Err(ApiError("换取 stoken 响应中未找到 stoken 字段".into()))
    }
}

/// 供 commands 层使用：从磁盘加载 cookie 并构造客户端
pub fn api_from_app(app: &AppHandle) -> Option<MihoyoApi> {
    let c = cookies::load_with_migration(app)?;
    if c.is_empty() {
        return None;
    }
    Some(MihoyoApi::new(app, &c))
}

#[cfg(test)]
mod tests {
    use super::ApiError;

    #[test]
    fn api_error_classification() {
        // 网络/暂时性失败：不算登录失效
        let net = ApiError("网络请求失败: error sending request for url ...".into());
        assert!(net.is_network());
        assert!(!net.is_session_expired());
        // 会话死亡：无 cookie / 服务端拒绝
        assert!(ApiError("未登录".into()).is_session_expired());
        assert!(ApiError("登录态不足，请重新扫码登录(-100): 登录失效".into()).is_session_expired());
        // 单游戏/其他错误：既非网络也非会话死亡（前端据此不误判）
        let dev = ApiError("设备风控(10041)，该游戏便签暂不可用（设备指纹校验未通过），不影响签到功能".into());
        assert!(!dev.is_network());
        assert!(!dev.is_session_expired());
        let captcha = ApiError("CAPTCHA_REQUIRED|触发了米游社验证码保护，请完成滑块验证".into());
        assert!(!captcha.is_session_expired());
        let biz = ApiError("获取账号失败(10102): 参数错误".into());
        assert!(!biz.is_session_expired());
    }
}
