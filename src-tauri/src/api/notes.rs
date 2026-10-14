//! 每日便签（体力 / 委托 / 派遣 / 活跃度）：战绩域 `api-takumi-record.mihoyo.com`
//!
//! ⚠️ 2026-10-14 恢复（v1.2.0-lite 期间移除）。
//!    注：当年的备份目录 `_full_feature_reference/` 已于上传 GitHub 前的清理中删除
//!    （功能已全部恢复，备份属冗余），历史沿革见开发文档 `11_/13_` 号报告。
//!
//! # 风控约束（**硬铁律，改动本文件前必读**）
//! 1. **仅手动刷新**：本模块**只允许**由 `commands::get_daily_note` 调用，而该命令
//!    **只能被前端「刷新」按钮显式触发**。禁止任何自动路径调用——
//!    包括定时器、切页签、`init()`/`refreshAll`、打开/关闭设置页、登录成功回调。
//!    理由：战绩域是**滑动窗口风控**，每次请求都重新计时；只要存在自动请求，
//!    窗口就永不关闭（09-01 ~ 09-26 曾连续 44 次启动成功率 0%）。
//! 2. **所有请求经 `gateway::acquire(Domain::Record)`**（5s 间隔 + 0~3s 抖动）。
//!    历史上本模块的请求绕过调用点节流，是"高频打战绩域"的盲区。
//! 3. **不做自动重试 / 不做退避重试**：失败即把原因如实返回，由用户决定是否再点。
//!    （旧版的 30 分钟自动退避重试已按铁律移除——它本质仍是"自动请求"。）
//!
//! # 接口
//! - 原神：`/game_record/app/genshin/api/dailyNote`（DS v2 战绩域盐）
//! - 绝区零：`/event/game_record_zzz/api/zzz/note`（**独立盐** `DS_SALT_CN_ZZZ`）
//!   注：zzz 的 `/widget` 已废弃返回 -100，正确端点为 `/note`（实测）。
use crate::api::find_spec;
use crate::api::sign::{ApiError, MihoyoApi};
use crate::crypto::ds::{ds_v2, DS_SALT_CN_RECORD, DS_SALT_CN_ZZZ};
use crate::gateway::{self, Domain};
use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
use tauri::Manager;

/// game_record 域 app_version（对齐 genshin.py `get_ds_headers` CHINESE）
const RECORD_APP_VERSION: &str = "2.11.1";

/// 构造 game_record 域请求头（DS v2 + client_type=5）
///
/// ⚠️ 与 `create_verification` / `verify_verification` 共用同一套头集合：
///    create 稳定成功即以此头集合为锚点，**不要**换成 paimon 的 2024 版模板头
///    （2.75.1 / tool_version / device_name / Origin / Referer）——实测 verify 恒 -1。
pub(crate) fn record_headers(api: &MihoyoApi, sign_game: &str, ds: &str) -> Result<HeaderMap, ApiError> {
    let mut h = HeaderMap::new();
    let cookie_str = api
        .cookie_map
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    h.insert(COOKIE, HeaderValue::from_str(&cookie_str).map_err(|e| ApiError(e.to_string()))?);
    h.insert("x-rpc-app_version", HeaderValue::from_static(RECORD_APP_VERSION));
    h.insert("x-rpc-client_type", HeaderValue::from_static("5"));
    h.insert("x-rpc-device_id", HeaderValue::from_str(&api.device_id).unwrap_or_else(|_| HeaderValue::from_static("00000000-0000-0000-0000-000000000000")));
    h.insert("x-rpc-sys_version", HeaderValue::from_static("12"));
    h.insert("x-rpc-platform", HeaderValue::from_static("android"));
    h.insert("x-rpc-channel", HeaderValue::from_static("miyousheluodi"));
    // ⚠️ sign_game 来自内置注册表（非用户输入），但 HeaderValue::from_str 失败会 panic，
    //    此处兜底（空串表示不携带该头，服务端自行判定）
    h.insert(
        "x-rpc-signgame",
        HeaderValue::from_str(sign_game).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    // 设备指纹：不带则 event 活动域接口（绝区零便签）触发设备风控 10041（实测 10041 已随
    //        账号画像消退解除，但该头仍是 event 域的必要条件）
    if !api.device_fp.is_empty() {
        h.insert(
            "x-rpc-device_fp",
            HeaderValue::from_str(&api.device_fp).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
    }
    // ⚠️ 时序约定：调用方必须**先 acquire，再构造头**（ds 内含秒级时间戳）
    h.insert(
        "ds",
        HeaderValue::from_str(ds).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    Ok(h)
}

/// 获取指定游戏账号的每日便签（accounts 由调用方传入，避免重复请求账号接口）
///
/// `app` 用于读取/清除 `x-rpc-challenge` 验证码 token（单次使用语义）。
///
/// 返回值：成功为便签 JSON；失败为**可直接展示给用户的中文原因**（含风控语义分流）。
pub async fn get_note(
    app: &tauri::AppHandle,
    api: &MihoyoApi,
    accounts: &[crate::api::sign::Account],
    game_id: &str,
) -> Result<serde_json::Value, String> {
    let spec = find_spec(game_id).ok_or_else(|| format!("未注册游戏: {game_id}"))?;

    let acc = accounts
        .iter()
        .find(|a| a.game_id == game_id)
        .ok_or_else(|| format!("未找到 {game_id} 账号"))?;

    // DS v2 签名约定（对齐 genshin.py）：role_id/server 作为 b=body(compact JSON)，q 为空
    // b 的 key 顺序固定 role_id→server（与 genshin.py dict 插入顺序一致）
    let body_json = format!(
        r#"{{"role_id":"{}","server":"{}"}}"#,
        acc.uid, acc.server
    );
    // 盐值：zzz 走 event 活动域接口，必须用 zzz 专属盐；其余走战绩域盐
    let salt = if game_id == "zzz" {
        DS_SALT_CN_ZZZ
    } else {
        DS_SALT_CN_RECORD
    };
    let ds = ds_v2(salt, &body_json, "");

    // ⚠️ 铁律：战绩域请求必须经网关（Domain::Record）
    gateway::acquire(Domain::Record).await;

    let mut headers = record_headers(api, spec.sign_game_header, &ds).map_err(|e| e.to_string())?;
    // 验证码 token（单次使用：take 即读取并清除；验证成功后由 captcha 链路写入）
    //
    // ⚠️⚠️ 头名必须**同时发两个拼写**（2026-10-14 从 paimon-webext 源码取证）：
    //    paimon 的生产代码用的是 `x-rpc-chellange`（**少一个 a**，其源码 `appendChallenge`
    //    里就是这么写的，且该扩展长期可用）；而本项目历史实现一直用 `x-rpc-challenge`，
    //    因为当年 verify 恒 -1、从未真正走到"用 token"这一步，所以头名从未被验证过。
    //    为避免猜错，两个都带（多发一个头无副作用，服务端只读它认识的那个）。
    if let Some(token) = crate::storage::challenge::take(app, game_id) {
        let hv = HeaderValue::from_str(&token).unwrap_or_else(|_| HeaderValue::from_static(""));
        headers.insert("x-rpc-challenge", hv.clone());
        headers.insert("x-rpc-chellange", hv);
        crate::debug_log::log(&format!(
            "get_note({game_id}): 已注入 x-rpc-challenge / x-rpc-chellange 头"
        ));
    }

    // zzz 正确端点为 /note（/widget 已废弃返回 -100，实测），其余游戏为 /dailyNote
    let endpoint = if game_id == "zzz" { "note" } else { "dailyNote" };
    let url = format!(
        "https://api-takumi-record.mihoyo.com{}/{endpoint}",
        spec.record_base
    );
    crate::debug_log::log(&format!("get_note({game_id}): 手动刷新请求 {url}"));

    let resp = api
        .client
        .get(&url)
        .headers(headers)
        .query(&[("role_id", &acc.uid), ("server", &acc.server)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    // 捕获 x-trace-id 响应头（geetest 验证码上下文：create/verifyVerification 需带上
    // x-rpc-challenge_trace 与服务端绑定请求，缺失会被 verifyVerification 拒绝）
    let trace_id = resp
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        gateway::note_response(Domain::Record, endpoint, None);
        return Err(format!("HTTP {status}: {}", &body[..body.len().min(200)]));
    }
    let resp: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            gateway::note_response(Domain::Record, endpoint, None);
            return Err(format!("响应非 JSON: {e} | body={}", &body[..body.len().min(200)]));
        }
    };

    let retcode = resp.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
    gateway::note_response(Domain::Record, endpoint, Some(retcode));

    // ⚠️ trace 记录时机：**先判断 retcode**。只有需要验证码（1034/5003/10035）时才存 trace，
    //    供随后 create_verification 使用；正常返回不产生 trace。
    if !trace_id.is_empty() && matches!(retcode, 5003 | 10035 | 1034) {
        if let Some(state) = app.try_state::<crate::commands::ApiState>() {
            // 验证码挑战期间（captcha_pending）不覆盖 trace：
            //   窗口内已创建挑战绑定旧 trace，刷新会使其脱钩 → verify 恒失败
            let pending = state
                .captcha_pending
                .lock()
                .map(|p| p.contains(game_id))
                .unwrap_or(false);
            if !pending {
                if let Ok(mut m) = state.trace_map.lock() {
                    m.insert(game_id.to_string(), trace_id.clone());
                }
                crate::debug_log::log(&format!("get_note({game_id}): 捕获 x-trace-id={trace_id}"));
            } else {
                crate::debug_log::log(&format!(
                    "get_note({game_id}): 验证码挑战进行中，跳过 trace 更新（保留 {trace_id}）"
                ));
            }
        }
    }

    if retcode != 0 {
        let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("");
        // 错误串纪律：单游戏级/暂时性错误**严禁**含"重新登录/登录态不足"关键词
        // （前端 isSessionExpiredMsg 以此判定会话死亡，误判会让主页面显示"未登录"）
        let hint = match retcode {
            5003 | 10035 | 1034 => {
                "CAPTCHA_REQUIRED|触发了米游社验证码保护，需完成滑块验证".to_string()
            }
            10041 => "设备风控(10041)，该游戏便签暂不可用（服务端设备风控拦截），不影响签到功能".to_string(),
            -100 => "登录态不足，请在设置中重新扫码登录获取完整 Cookie".to_string(),
            _ => format!("便签获取失败({retcode}): {msg}"),
        };
        return Err(hint);
    }

    let data = resp.get("data").unwrap_or(&serde_json::Value::Null);

    // 统一输出结构（前端 DailyNote / NOTE_CONFIG 对齐）
    let mut out = serde_json::json!({
        "game_id": game_id,
        "game_name": spec.name,
        "game_icon": spec.icon,
        "uid": acc.uid,
        "nickname": acc.nickname,
    });

    match game_id {
        "genshin" => {
            out["resin"] = serde_json::json!({
                "current": data.get("current_resin").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.get("max_resin").and_then(|v| v.as_i64()).unwrap_or(0),
                "recovery_time_seconds": data.get("remaining_resin_recovery_time").and_then(|v| v.as_i64()).unwrap_or(0),
            });
            out["commissions"] = serde_json::json!({
                "completed": data.get("completed_commissions").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.get("max_commissions").and_then(|v| v.as_i64()).unwrap_or(0),
                "finished": data.get("completed_commissions").and_then(|v| v.as_i64()).unwrap_or(0)
                    >= data.get("max_commissions").and_then(|v| v.as_i64()).unwrap_or(0),
            });
            let total_exp = data.get("total_expedition_num").and_then(|v| v.as_i64()).unwrap_or(0);
            let done_exp = data.get("finished_expedition_num").and_then(|v| v.as_i64()).unwrap_or(0);
            out["expeditions"] = serde_json::json!({
                "completed": done_exp,
                "max": total_exp,
                "ongoing": total_exp - done_exp,
            });
            out["home_coin"] = serde_json::json!({
                "current": data.get("current_home_coin").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.get("max_home_coin").and_then(|v| v.as_i64()).unwrap_or(2400),
                "is_full": data.get("current_home_coin").and_then(|v| v.as_i64()).unwrap_or(0)
                    >= data.get("max_home_coin").and_then(|v| v.as_i64()).unwrap_or(2400),
            });
        }
        "zzz" => {
            // energy 为每日体力，current/max 在 progress 子对象里（实测 /energy/current 恒 0）
            out["resin"] = serde_json::json!({
                "current": data.pointer("/energy/progress/current").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.pointer("/energy/progress/max").and_then(|v| v.as_i64()).unwrap_or(0),
                "recovery_time_seconds": data.pointer("/energy/restore").and_then(|v| v.as_i64()).unwrap_or(0),
            });
            // vitality 为今日活跃度，独立资源（0 也是有效状态，无条件输出）
            out["vitality"] = serde_json::json!({
                "current": data.pointer("/vitality/current").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.pointer("/vitality/max").and_then(|v| v.as_i64()).unwrap_or(0),
            });
            // —— 展开页字段 ——
            // 录像店经营（SaleStateDoing=营业中）
            if let Some(s) = data.pointer("/vhs_sale/sale_state").and_then(|v| v.as_str()) {
                out["vhs_sale"] = serde_json::json!({ "state": s });
            }
            // 每日盲盒/刮刮卡（CardSignDone 等枚举）
            if let Some(s) = data.get("card_sign").and_then(|v| v.as_str()) {
                out["card_sign"] = serde_json::json!({ "state": s });
            }
            // 悬赏委托进度（bounty_commission）：2026-10-14 探针实测含 refresh_time
            out["bounty_commission"] = serde_json::json!({
                "current": data.pointer("/bounty_commission/num").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.pointer("/bounty_commission/total").and_then(|v| v.as_i64()).unwrap_or(0),
            });
            // 周常刷新时间（abyss_refresh，秒）
            if let Some(s) = data.get("abyss_refresh").and_then(|v| v.as_i64()) {
                out["abyss_refresh"] = serde_json::json!({ "seconds": s });
            }
            // 丽都周纪获取积分（weekly_task）
            out["weekly_task"] = serde_json::json!({
                "current": data.pointer("/weekly_task/cur_point").and_then(|v| v.as_i64()).unwrap_or(0),
                "max": data.pointer("/weekly_task/max_point").and_then(|v| v.as_i64()).unwrap_or(0),
            });
            // 绳网会员状态（member_card）
            if let Some(b) = data.pointer("/member_card/is_open").and_then(|v| v.as_bool()) {
                out["member_card"] = serde_json::json!({ "is_open": b });
            }
            // 咖啡店状态（CafeStateDone=已领取）
            if let Some(s) = data.get("cafe_state").and_then(|v| v.as_str()) {
                out["cafe_state"] = serde_json::json!({ "state": s });
            }
            // 录像店等级（temple_running.level，2026-10-14 探针实测字段）
            if let Some(l) = data.pointer("/temple_running/level").and_then(|v| v.as_i64()) {
                out["temple_running"] = serde_json::json!({ "level": l });
            }
        }
        _ => {}
    }

    Ok(out)
}
