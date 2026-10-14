//! geetest 验证码机制：createVerification / verifyVerification
//!
//! 触发链：`dailyNote` 返回 1034/5003 → createVerification 拿 gt+challenge
//! → 前端滑块（官方 gt.js）→ 用户通过 → verifyVerification 换 x-rpc-challenge token
//! → 后续 `get_note` 请求带 x-rpc-challenge 头（单次使用，用后清除）。
//!
//! 参考实现：paimon-webext（DS 计算、challenge_path 头）、GenshinPlayerQuery（流程）。
//!
//! ⚠️ 风控约束：本模块全部请求打**战绩域**（`Domain::Record`），同样经网关节流；
//!    且**只允许由用户显式点击「去验证」触发**，禁止任何自动路径调用。
//!
//! ⚠️ 历史结论（2026-08-30 定性，需在账号画像消退后重测）：
//!    滑块渲染/拖动/onSuccess 全通，键序/头集合/device_fp 全部对齐后双通道仍恒 -1
//!    → 当时判定为**账号/IP/设备级风控画像**（非代码问题）。
//!    2026-10-14 探针实测：zzz 便签 retcode=0、账号级画像已消退 →
//!    原神 verify 是否仍 -1 值得重测（**仅用户手动触发，失败即止，不重试**）。
//!
//! # 接线状态（2026-10-14 已接线）
//! 命令层已就位：`get_verification_challenge` / `open_captcha_window` /
//! `verify_captcha_fallback`（见 `commands.rs`），滑块窗口 + `public/captcha_inject.js` +
//! `capabilities/captcha.json` 均已恢复。四事件监听器在 `lib.rs`。
use crate::api::notes::record_headers;
use crate::api::sign::MihoyoApi;
use crate::crypto::ds::{ds_v2, DS_SALT_CN_RECORD};
use crate::gateway::{self, Domain};
use reqwest::header::{HeaderMap, HeaderValue};

const WAPI_BASE: &str = "https://api-takumi-record.mihoyo.com/game_record/app/card/wapi";

/// 验证码关联的目标接口完整 URL（米游社用于生成 challenge 上下文，实测需与触发接口一致）。
/// ⚠️ per-game：原神为 `/game_record/app/genshin/api/dailyNote`，绝区零为 event 域 `/note`
///    （曾硬编码 genshin 路径，若 zzz 也触发验证码则 challenge_path 与触发接口不匹配）。
pub fn challenge_path_for(game_id: &str) -> String {
    match crate::api::find_spec(game_id) {
        Some(spec) => {
            let endpoint = if game_id == "zzz" { "note" } else { "dailyNote" };
            format!("https://api-takumi-record.mihoyo.com{}/{endpoint}", spec.record_base)
        }
        // 未注册游戏：退回原神 dailyNote（保持与 challenge_path_for("genshin") 一致）
        None => CHALLENGE_PATH_DAILY_NOTE.to_string(),
    }
}

/// 兼容旧调用点（原神 dailyNote 路径）：仅诊断脚本 / python 探针对齐用，Rust 侧不再引用
#[allow(dead_code)]
pub const CHALLENGE_PATH_DAILY_NOTE: &str =
    "https://api-takumi-record.mihoyo.com/game_record/app/genshin/api/dailyNote";

/// 追加 x-rpc-challenge_path / x-rpc-challenge_trace 头（geetest 挑战上下文）
fn add_challenge_headers(
    mut headers: HeaderMap,
    challenge_path: &str,
    trace: Option<&str>,
) -> HeaderMap {
    headers.insert(
        "x-rpc-challenge_path",
        HeaderValue::from_str(challenge_path).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    if let Some(t) = trace {
        if !t.is_empty() {
            headers.insert(
                "x-rpc-challenge_trace",
                HeaderValue::from_str(t).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
        }
    }
    headers
}

/// 1) 获取 geetest 挑战参数：返回 (gt, challenge)
///
/// DS 计算：b=''，q=is_high=true（对齐 paimon-webext getDS 参数排序拼接）
pub async fn create_verification(
    api: &MihoyoApi,
    sign_game: &str,
    challenge_path: &str,
    trace: Option<&str>,
) -> Result<(String, String), String> {
    let ds = ds_v2(DS_SALT_CN_RECORD, "", "is_high=true");
    // 战绩域请求 → 经网关节流（先 acquire 再构造头，保持 ds 时间戳新鲜）
    gateway::acquire(Domain::Record).await;
    let mut headers = record_headers(api, sign_game, &ds).map_err(|e| e.to_string())?;
    headers = add_challenge_headers(headers, challenge_path, trace);

    let url = format!("{WAPI_BASE}/createVerification?is_high=true");
    let resp = api
        .client
        .get(&url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("createVerification 请求失败: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        gateway::note_response(Domain::Record, "createVerification", None);
        return Err(format!("HTTP {status}: {}", &body[..body.len().min(200)]));
    }
    let v: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            gateway::note_response(Domain::Record, "createVerification", None);
            return Err(format!("createVerification 响应非 JSON: {e} | {body}"));
        }
    };
    let rc = v.get("retcode").and_then(|x| x.as_i64()).unwrap_or(-1);
    gateway::note_response(Domain::Record, "createVerification", Some(rc));
    if rc != 0 {
        return Err(format!(
            "createVerification 失败({rc}): {}",
            v.get("message").and_then(|x| x.as_str()).unwrap_or("")
        ));
    }
    let data = &v["data"];
    let gt = data.get("gt").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let challenge = data
        .get("challenge")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if gt.is_empty() || challenge.is_empty() {
        return Err(format!("createVerification 响应缺 gt/challenge: {body}"));
    }
    crate::debug_log::log(&format!(
        "createVerification 成功: gt={} challenge={}",
        &gt[..gt.len().min(16)],
        &challenge[..challenge.len().min(16)]
    ));
    Ok((gt, challenge))
}

/// 2) 提交滑块结果，换取 x-rpc-challenge token
///
/// body 字段名带 `geetest_` 前缀（paimon-webext ICaptchaRequest 实测确认）。
///
/// ⚠️ 头集合 = create 完全同款（`record_headers`：app_version 2.11.1 + device_fp + signgame），
///    **不要**用 paimon 模板头（2.75.1 / tool_version / device_name / Origin / Referer 等）——
///    2026-08-30 实测：paimon 2024 版模板头 verify 恒 -1；create（record_headers 2.11.1）
///    稳定成功；genshin.py（仍在维护）CN 域也用 2.11.1。以本环境成功锚点为准。
///
/// DS 计算：b=body（**严格与请求体字节一致**，固定字段顺序 challenge→seccode→validate）
pub async fn verify_verification(
    api: &MihoyoApi,
    geetest_challenge: &str,
    geetest_validate: &str,
    geetest_seccode: &str,
    sign_game: &str,
    challenge_path: &str,
    trace: Option<&str>,
) -> Result<String, String> {
    // 固定字段顺序构造 body（challenge→seccode→validate，对齐 ICaptchaRequest；
    // DS 的 b 必须与请求体字节一致）
    let body = format!(
        r#"{{"geetest_challenge":"{}","geetest_seccode":"{}","geetest_validate":"{}"}}"#,
        geetest_challenge, geetest_seccode, geetest_validate
    );
    let ds = ds_v2(DS_SALT_CN_RECORD, &body, "");
    // 与 create 完全同款头（record_headers + challenge 上下文），不带 paimon 模板头
    gateway::acquire(Domain::Record).await;
    let mut headers = record_headers(api, sign_game, &ds).map_err(|e| e.to_string())?;
    headers = add_challenge_headers(headers, challenge_path, trace);

    let url = format!("{WAPI_BASE}/verifyVerification");
    let resp = api
        .client
        .post(&url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("verifyVerification 请求失败: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        gateway::note_response(Domain::Record, "verifyVerification", None);
        return Err(format!("HTTP {status}: {}", &body[..body.len().min(200)]));
    }
    let v: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            gateway::note_response(Domain::Record, "verifyVerification", None);
            return Err(format!("verifyVerification 响应非 JSON: {e} | {body}"));
        }
    };
    let rc = v.get("retcode").and_then(|x| x.as_i64()).unwrap_or(-1);
    gateway::note_response(Domain::Record, "verifyVerification", Some(rc));
    if rc != 0 {
        return Err(format!(
            "verifyVerification 失败({rc}): {} | raw={}",
            v.get("message").and_then(|x| x.as_str()).unwrap_or(""),
            &body[..body.len().min(300)]
        ));
    }
    let token = v["data"]
        .get("challenge")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return Err(format!("verifyVerification 响应缺 challenge: {body}"));
    }
    crate::debug_log::log("verifyVerification 成功，已换取 x-rpc-challenge token");
    Ok(token)
}
