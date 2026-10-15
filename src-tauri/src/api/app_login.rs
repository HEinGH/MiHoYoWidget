//! 米游社「App 身份会话」扫码登录（2026-10-15 新增 · Stage 1）
//!
//! ## 为什么需要它
//!
//! 原神 `dailyNote` 对**客户端身份**门槛更高：本应用原有的"网页扫码（WebView 加载
//! `user.mihoyo.com`）"身份只能拿到 `ltoken_v2`，**拿不到米游社站点的 `cookie_token`**，
//! 且**不会在通行证侧登记设备**——请求恒被 `1034`/`5003` 挡下。
//!
//! 本模块走官方的「**米游社扫码登录**」flow（UIGF `hoyolab/login/qrcode_hoyolab`），
//! 实测（报告 `15_路线A实战记录_1015.md`）确认它有两项关键差异：
//! 1. 确认登录后**从响应头 `Set-Cookie` 一次性下发完整凭证**
//!    （`ltoken_v2` + `cookie_token_v2` + `ltuid_v2` + `ltmid_v2`）；
//! 2. **会在米哈游通行证侧登记设备**——用户会收到官方「新设备登录」邮件，
//!    且设备出现在手机 App 的「登录设备管理」中（设备名 = 本模块的 `DEVICE_NAME`）。
//!
//! ## 与 `hk4e-sdk` GameToken 路线的关系（**不要再走那条**）
//!
//! 2026-10-15 已穷举 15 个变体（含用户实测的真实 App 版本 `2.116.0`、双 host、双 DS 盐、
//! 极简头/去 aigis/去 game_biz/加 app_key/verify_key、以及绕行 `getCookieAccountInfoByGameToken`），
//! GameToken 换取环节**恒 `-5300`**（"请升级应用版本或删除账号后重新登录"）
//! ⇒ 该路线在本环境**不可用**，已放弃。
//!
//! ## 🔴 三条实测纪律（改动本模块前必读）
//!
//! 1. **`x-rpc-device_id` 必须持久化并与会话成套**：v2 会话与设备绑定，换 id 即失效。
//!    本模块登录成功后会把新 `device_id` 写进 `config`（沿用 `login_with_cookies` 的纪律）。
//! 2. **`APP_VERSION` 必须与用户手机上实际的米游社 App 版本一致**：
//!    社区文档/代码快照里的版本号（2.41/2.75/2.80）早已过时。当前值由用户 2026-10-15 实测提供；
//!    若日后接口报版本类错误（如 `-5300`），**第一件事是问用户当前版本**。
//! 3. **`DEVICE_NAME` 是用户可见值**：它会原样出现在用户的「登录设备管理」里，
//!    并出现在官方的「新设备登录」提醒邮件中。**不要随意填一个假手机型号。**
//!
//! ## 风控面
//!
//! 本模块只打 `passport-api.miyoushe.com` 与 `public-data-api.mihoyo.com`
//! —— **均不属于战绩域**（`api-takumi-record`），不触碰"仅手动刷新"铁律。
use crate::api::sign::ApiError;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, SET_COOKIE, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 创建扫码会话（`retcode=-3001` = 请求头缺少部分参数）
const CREATE_URL: &str =
    "https://passport-api.miyoushe.com/account/ma-cn-passport/web/createQRLogin";
/// 查询扫码状态（`-3501` 二维码已过期 / `-3505` 用户取消扫码）
const QUERY_URL: &str =
    "https://passport-api.miyoushe.com/account/ma-cn-passport/web/queryQRLoginStatus";
/// 设备指纹签发（platform=2 安卓）
const GETFP_URL: &str = "https://public-data-api.mihoyo.com/device-fp/api/getFp";

/// 米游社 App 的 app_id（UIGF 文档 + 实测均为此值）
const APP_ID: &str = "bll8iq97cem8";
/// 米游社 App 版本号。
/// ⚠️ **用户 2026-10-15 从手机实测**：安卓端为 `2.116.0`。
///    本值同时用于 `x-rpc-app_version` 与 UA 中的 `miHoYoBBS/<ver>`（两处需同步修改）。
const APP_VERSION: &str = "2.116.0";
/// App 会话 UA（与 `APP_VERSION` 同步）
const APP_UA: &str = "Mozilla/5.0 (Linux; Android 13; 2304FPN6DC Build/TKQ1.221114.001; wv) \
     AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/108.0.5359.128 \
     Mobile Safari/537.36 miHoYoBBS/2.116.0";

/// 设备名/型号。
/// ⚠️ **会原样显示在用户的「登录设备管理」与官方「新设备登录」邮件里**（实测）——
///    刻意取一个可识别且诚实的中性名，不要伪装成某款真机型号。
pub const DEVICE_NAME: &str = "MiHoYoWidget";

/// 单次扫码会话的最长轮询时长（秒）。
/// ⚠️ 服务端对 `createQRLogin` 的二维码只给 **约 6 分钟**有效期；
///    本值同时是 `app_login_busy` 标记的 **TTL 兜底**（纪律：任何 pending 标记必须有出口）。
pub const POLL_TIMEOUT_SECS: u64 = 300;

/// 随机 hex 串（长度 n）
fn rand_hex(n: usize) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    (0..n)
        .map(|_| HEX[(rand::random::<u64>() % 16) as usize] as char)
        .collect()
}

/// 本 flow 的公共请求头（UIGF 只要求 `x-rpc-app_id` + `x-rpc-device_id`，
/// 其余按"安卓 App"画像补齐，与探针实测通过的组合保持一致）
fn headers(device_id: &str, device_fp: &str) -> Result<HeaderMap, ApiError> {
    let mut h = HeaderMap::new();
    // ⚠️ `HeaderValue::from_str` 遇非法字节会失败；一律走 map_err，**绝不 unwrap**
    let mut put = |k: &'static str, v: &str| -> Result<(), ApiError> {
        h.insert(k, HeaderValue::from_str(v).map_err(|e| ApiError(e.to_string()))?);
        Ok(())
    };
    put("x-rpc-app_id", APP_ID)?;
    put("x-rpc-device_id", device_id)?;
    put("x-rpc-client_type", "2")?; // 2 = 安卓 App（实测服务端回显 client_type=2）
    put("x-rpc-app_version", APP_VERSION)?;
    put("x-rpc-sys_version", "13")?;
    put("x-rpc-platform", "android")?;
    put("x-rpc-channel", "miyousheluodi")?;
    put("x-rpc-device_name", DEVICE_NAME)?;
    put("x-rpc-device_model", DEVICE_NAME)?;
    if !device_fp.is_empty() {
        put("x-rpc-device_fp", device_fp)?;
    }
    put("Referer", "https://app.mihoyo.com")?;
    put("Origin", "https://app.mihoyo.com")?;
    put("X-Requested-With", "com.mihoyo.hyperion")?;
    if let Ok(v) = HeaderValue::from_str(APP_UA) {
        h.insert(USER_AGENT, v);
    }
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(h)
}

/// 专用客户端（**不复用 `MihoyoApi` 的网页头客户端**：两套身份必须严格分离）
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(APP_UA)
        .build()
        .unwrap_or_default()
}

/// 申请服务端签发的设备指纹（`platform=2` 安卓画像，字段清单照 UIGF 官方示例）。
/// **best-effort**：失败时回退为随机 13 位 hex（社区同款兜底），不阻断登录流程。
pub async fn get_fp(client: &reqwest::Client, device_id: &str) -> String {
    let ext = serde_json::json!({
        "cpuType": "arm64-v8a", "romCapacity": "512", "productName": "ishtar",
        "romRemain": "459", "manufacturer": "Xiaomi", "appMemory": "512",
        "hostname": "xiaomi.eu", "screenSize": "1440x3022", "osVersion": "13",
        "aaid": rand_hex(16), "vendor": "中国电信",
        "accelerometer": "0.061016977x0.8362915x9.826724", "buildTags": "release-keys",
        "model": "2304FPN6DC", "brand": "Xiaomi", "oaid": rand_hex(16), "hardware": "qcom",
        "deviceType": "ishtar", "devId": "REL", "serialNumber": "unknown",
        "buildTime": "1690889245000", "buildUser": "builder", "ramCapacity": "229481",
        "magnetometer": "80.64375x-14.1x77.90625",
        "display": "TKQ1.221114.001 release-keys", "ramRemain": "110308",
        "deviceInfo": "Xiaomi/ishtar/ishtar:13/TKQ1.221114.001/V14.0.17.0.TMACNXM:user/release-keys",
        "gyroscope": "7.9894776E-4x-1.3315796E-4x6.6578976E-4", "vaid": rand_hex(16),
        "buildType": "user", "sdkVersion": "33", "board": "kalama"
    });
    let body = serde_json::json!({
        "device_id": rand_hex(16),
        "seed_id": uuid::Uuid::new_v4().to_string(),
        "seed_time": (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)).to_string(),
        "platform": "2",
        "device_fp": rand_hex(13),
        "app_name": "bbs_cn",
        "ext_fields": ext.to_string(),
        "bbs_device_id": device_id,
    });
    let Ok(h) = headers(device_id, "") else {
        return rand_hex(13);
    };
    let resp = client
        .post(GETFP_URL)
        .headers(h)
        .body(body.to_string())
        .send()
        .await;
    let Ok(resp) = resp else {
        crate::debug_log::log("app_login::get_fp: 请求失败，回退随机 fp");
        return rand_hex(13);
    };
    match resp.json::<serde_json::Value>().await {
        Ok(v) => {
            let rc = v.get("retcode").and_then(|x| x.as_i64()).unwrap_or(-1);
            let fp = v
                .pointer("/data/device_fp")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if rc == 0 && !fp.is_empty() {
                crate::debug_log::log("app_login::get_fp: 已取得服务端设备指纹");
                fp.to_string()
            } else {
                crate::debug_log::log("app_login::get_fp: 响应无 device_fp，回退随机 fp");
                rand_hex(13)
            }
        }
        Err(_) => rand_hex(13),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppQr {
    /// 二维码内容（由**前端**渲染成二维码图片；⚠️ 该页面本身不渲染二维码，实测）
    pub url: String,
    /// 查询扫码状态用的 ticket
    pub ticket: String,
}

/// ① 创建扫码会话
pub async fn create_qr(
    client: &reqwest::Client,
    device_id: &str,
    device_fp: &str,
) -> Result<AppQr, ApiError> {
    let h = headers(device_id, device_fp)?;
    let resp = client
        .post(CREATE_URL)
        .headers(h)
        .body("{}")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let rc = resp.get("retcode").and_then(|x| x.as_i64()).unwrap_or(-1);
    if rc != 0 {
        let msg = resp.get("message").and_then(|x| x.as_str()).unwrap_or("unknown");
        return Err(ApiError(format!("创建扫码会话失败({rc}): {msg}")));
    }
    let url = resp.pointer("/data/url").and_then(|x| x.as_str()).unwrap_or("");
    let ticket = resp.pointer("/data/ticket").and_then(|x| x.as_str()).unwrap_or("");
    if url.is_empty() || ticket.is_empty() {
        return Err(ApiError("创建扫码会话失败：响应缺少 url/ticket".into()));
    }
    Ok(AppQr { url: url.to_string(), ticket: ticket.to_string() })
}

#[derive(Debug, Clone, Default)]
pub struct QrPoll {
    /// `Created`（未扫描）/ `Scanned`（已扫描）/ `Confirmed`（已确认）
    pub status: String,
    /// 本次响应 `Set-Cookie` 解析出的 kv（**确认登录后**才含凭证）
    pub cookies: HashMap<String, String>,
}

/// ② 查询扫码状态。
/// ⚠️ 凭证在**响应头 `Set-Cookie`** 里（`ltoken_v2`/`cookie_token_v2`/`ltuid_v2`/`ltmid_v2`…），
///    必须用 `get_all(SET_COOKIE)` 逐个收集——多个同名头会被合并，用 `get()` 会漏。
pub async fn query_qr(
    client: &reqwest::Client,
    device_id: &str,
    device_fp: &str,
    ticket: &str,
) -> Result<QrPoll, ApiError> {
    let h = headers(device_id, device_fp)?;
    let body = serde_json::json!({ "ticket": ticket }).to_string();
    let resp = client.post(QUERY_URL).headers(h).body(body).send().await?;

    // ① 先收 Set-Cookie（借用 headers，必须在 json() 消费 resp 之前完成）
    let mut cookies = HashMap::new();
    for v in resp.headers().get_all(SET_COOKIE).iter() {
        let Ok(s) = v.to_str() else { continue };
        let Some(first) = s.split(';').next() else { continue };
        let Some((k, val)) = first.split_once('=') else { continue };
        let (k, val) = (k.trim(), val.trim());
        if !k.is_empty() && !val.is_empty() {
            cookies.insert(k.to_string(), val.to_string());
        }
    }

    let v = resp.json::<serde_json::Value>().await?;
    let rc = v.get("retcode").and_then(|x| x.as_i64()).unwrap_or(-1);
    if rc != 0 {
        let msg = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
        return Err(ApiError(format!("查询扫码状态失败({rc}): {msg}")));
    }
    let status = v
        .pointer("/data/status")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Ok(QrPoll { status, cookies })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rand_hex_len_and_charset() {
        let s = rand_hex(13);
        assert_eq!(s.len(), 13);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn headers_fill_required_fields() {
        let h = headers("dev", "fp").expect("headers");
        assert_eq!(h.get("x-rpc-app_id").unwrap(), APP_ID);
        assert_eq!(h.get("x-rpc-device_id").unwrap(), "dev");
        assert_eq!(h.get("x-rpc-device_fp").unwrap(), "fp");
        assert_eq!(h.get("x-rpc-client_type").unwrap(), "2");
        // device_fp 为空时不应带该头（空 HeaderValue 会被服务端判参数错误）
        assert!(headers("dev", "").unwrap().get("x-rpc-device_fp").is_none());
    }

    /// 回归：凭证只从 `Set-Cookie` 取，且 `ltmid_v2` 需能被 `Cookies::from_map` 识别为 mid
    #[test]
    fn set_cookie_first_pair_parsing() {
        let raw = "ltoken_v2=abc; Path=/; HttpOnly";
        let first = raw.split(';').next().unwrap();
        let (k, v) = first.split_once('=').unwrap();
        assert_eq!((k.trim(), v.trim()), ("ltoken_v2", "abc"));
    }
}
