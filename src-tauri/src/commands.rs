//! Tauri command 层：前端调用面
//!
//! ⚠️ 版本沿革
//! - v1.2.0-lite（2026-09-26 ~ 2026-10-14）：**零战绩域**保命版本，仅登录 + 签到。
//!   原因：账号自 09-01 起持续风控（435× 10041 + 200× 1034），而旧版每次启动/刷新/点页签
//!   都请求便签 → 持续续期风控滑动窗口 → 窗口永不关闭。
//! - **v1.2.1（2026-10-14 起）**：探针实测风控已消退（zzz `retcode=0`），恢复便签功能。
//!   **新增硬铁律：战绩域一律「仅手动刷新」**——便签请求只允许由前端「刷新」按钮
//!   显式触发，任何自动路径（定时器 / 切页签 / init / 设置页 / 登录回调）都不得调用。
//!   见 `api/notes.rs` 顶部注释。
use crate::api::sign::{api_from_app, Account, MihoyoApi};
use crate::storage::{config, cookies};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

/// 全局共享状态：API 客户端缓存 + 账号列表缓存（60s 有效，减少重复请求）
pub struct ApiState {
    pub client: Mutex<Option<MihoyoApi>>,
    pub accounts: Mutex<Option<(Instant, Vec<Account>)>>,
    /// 最近一次便签请求的 `x-trace-id`（per game）：geetest 验证码上下文，
    /// create / verifyVerification 均需带 `x-rpc-challenge_trace`，缺失会被拒绝。
    pub trace_map: Mutex<std::collections::HashMap<String, String>>,
    /// 验证码挑战进行中的游戏集合：期间 **不得覆盖 trace_map**
    /// （窗口内已创建的挑战绑定旧 trace，刷新会使其脱钩 → verify 恒失败，实测踩坑）。
    ///
    /// ⚠️ 状态机纪律：任何"冻结/pending 标记"**必须有出口 + TTL 兜底**。
    ///    本标记由 `captcha-success` 事件或看门狗超时解除；无出口的标记会永久卡死流程。
    pub captcha_pending: Mutex<HashSet<String>>,
    /// 「App 会话登录」进行中标记（Stage 1，2026-10-15）。
    ///
    /// ⚠️ 纪律同上：**必须有出口 + TTL 兜底**。本标记由三处之一释放：
    /// ① 轮询走到 `Confirmed` 并完成落盘；② 用户点「取消」；③ 超时
    /// （`app_login::POLL_TIMEOUT_SECS`，与服务端二维码有效期同量级）。
    pub app_login_busy: Mutex<bool>,
    /// 「App 会话登录」取消信号（轮询循环每 2s 检查一次）
    pub app_login_abort: Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
}

impl ApiState {
    pub fn new() -> Self {
        Self {
            client: Mutex::new(None),
            accounts: Mutex::new(None),
            trace_map: Mutex::new(std::collections::HashMap::new()),
            captcha_pending: Mutex::new(HashSet::new()),
            app_login_busy: Mutex::new(false),
            app_login_abort: Mutex::new(None),
        }
    }
}

impl Default for ApiState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize)]
pub struct AppState {
    pub has_cookies: bool,
    pub games: Vec<String>,
    pub auto_checkin: bool,
    pub auto_start: bool,
    pub transparency: f64,
    pub font_scale: f64,
    pub allow_manual_cookie: bool,
    /// 签到域自动刷新间隔（秒，已由后端夹取 ≥900）。
    /// ⚠️ 只作用于签到域；**战绩域（便签）不受此配置影响——它永远是"仅手动刷新"**。
    pub refresh_interval: u64,
    /// 「原神便签」开关（v1.3.0 起默认 false = **冻结**）。
    /// 冻结时前端不渲染原神便签的刷新入口，后端 `get_daily_note` 也会直接拒绝（不发网络请求）。
    pub genshin_note_enabled: bool,
    /// 二进制版本号（编译期取自 `Cargo.toml`）。
    /// ⚠️ 前端底部版本栏**必须**用它渲染，**禁止再硬编码**——历史上硬编码导致 v1.3.0 仍显示 v1.2.1。
    pub version: String,
}

fn load_api(app: &AppHandle, state: &State<'_, ApiState>) -> Option<MihoyoApi> {
    let mut guard = state.client.lock().unwrap();
    if guard.is_none() {
        crate::debug_log::log("load_api: 尝试加载 cookie 并构造客户端");
        *guard = api_from_app(app);
        if guard.is_some() {
            crate::debug_log::log("load_api: 客户端构造成功（cookie 已加载/迁移）");
        } else {
            crate::debug_log::log("load_api: 无可用 cookie（构造失败）");
        }
    }
    guard.clone()
}

/// 获取账号列表（60s 缓存，避免每个 command 重复请求账号接口）
async fn load_accounts(
    app: &AppHandle,
    state: &State<'_, ApiState>,
) -> Result<Vec<Account>, String> {
    {
        let guard = state.accounts.lock().unwrap();
        if let Some((ts, accounts)) = guard.as_ref() {
            if ts.elapsed() < Duration::from_secs(60) && !accounts.is_empty() {
                return Ok(accounts.clone());
            }
        }
    }
    let api = load_api(app, state).ok_or_else(|| "未登录".to_string())?;
    let accounts = api.get_game_accounts().await.map_err(|e| e.to_string())?;
    *state.accounts.lock().unwrap() = Some((Instant::now(), accounts.clone()));
    Ok(accounts)
}

#[tauri::command]
pub fn get_app_state(app: AppHandle) -> Result<AppState, String> {
    crate::debug_log::log("get_app_state 被调用");
    let cfg = config::load(&app);
    let has = cookies::has(&app);
    crate::debug_log::log(&format!("get_app_state: has_cookies={has}"));
    // cookie 有效性由前端调用 get_login_status 异步判断（避免同步阻塞网络），
    // 故此处不再返回曾存在但前端从未使用的 cookie_valid 字段（恒 false，已删除）
    Ok(AppState {
        has_cookies: has,
        games: cfg.games.clone(),
        auto_checkin: cfg.auto_checkin,
        auto_start: cfg.auto_start,
        transparency: cfg.transparency,
        font_scale: cfg.font_scale,
        allow_manual_cookie: cfg.allow_manual_cookie,
        refresh_interval: cfg.refresh_interval,
        genshin_note_enabled: cfg.genshin_note_enabled,
        // 编译期常量：与 Cargo.toml 的 version 天然一致（前端底部版本栏用它渲染）
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// 手动粘贴 Cookie 登录（设置页开关开启时可用）
///
/// ⚠️⚠️ 2026-10-14 扩展为「成套导入」——**只搬 cookie 是不够的**（实测依据）：
///   1. `x-rpc-device_id` 与 v2 会话**绑定**：随机 uuid 会被判登录失效（`-100`，
///      2026-08-30 实测）；
///   2. `device_fp` 的注册以 `bbs_device_id` 为锚：换 device_id 后旧 fp 立即失效
///      （→ `5003`，2026-10-14 探针实测）；
///   3. 反过来说，**设备身份正是能否通过的关键**：服务端按"设备"区分对待
///      （我们的 WebView2 设备 → `1034/5003`；手机 App / QQbot 的设备 → 正常）。
///   ⇒ 因此从 App 抓包导入时，必须把 **cookie + x-rpc-device_id（+ x-rpc-device_fp）**
///      三者来自**同一次抓包**，成套导入。
///
/// 参数：`device_id` / `device_fp` 可留空——留空时分别回退为「沿用当前设备标识」
/// 与「从粘贴内容里识别 / 保留既有 DEVICEFP」（保证既有扫码会话行为不变）。
/// 失败会**回滚设备标识**，不会破坏当前正在用的登录。
#[tauri::command]
pub async fn login_with_cookies(
    app: AppHandle,
    state: State<'_, ApiState>,
    cookies_map: std::collections::HashMap<String, String>,
    device_id: Option<String>,
    device_fp: Option<String>,
) -> Result<serde_json::Value, String> {
    // 后端防御：开关未开启时直接拒绝（防止绕过前端 UI 直接调用命令）
    if !config::load(&app).allow_manual_cookie {
        crate::debug_log::log("login_with_cookies 被拒绝：未开启 allow_manual_cookie");
        return Err("未开启手动粘贴 Cookie 权限，请在设置中开启".into());
    }
    let mut map = cookies_map;

    // ① 设备标识：显式参数优先；否则尝试从粘贴内容里识别
    //    （抓包工具常把请求头也一并复制，此时键名形如 x-rpc-device_id）
    let dev_arg = device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            ["x-rpc-device_id", "device_id", "_MHYUUID", "deviceId"]
                .iter()
                .find_map(|k| {
                    map.get(*k)
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                })
        });
    // 这些不是 cookie，剔除以免污染 Cookie 串
    for k in [
        "x-rpc-device_id",
        "device_id",
        "_MHYUUID",
        "deviceId",
        "x-rpc-device_fp",
    ] {
        map.remove(k);
    }

    // ② device_fp：显式参数 > 粘贴内容里的 DEVICEFP
    let fp_arg = device_fp
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| map.get("DEVICEFP").cloned().filter(|s| !s.trim().is_empty()));
    if let Some(f) = &fp_arg {
        map.insert("DEVICEFP".into(), f.clone());
    }

    let mut c = cookies::Cookies::from_map(&map);
    if c.is_empty() {
        return Err("未提供有效 cookie 字段（至少需要 ltoken_v2 / ltoken / cookie_token 之一）".into());
    }
    // device_fp 留空时**保留既有值**，否则会丢掉扫码时种下的 DEVICEFP → zzz 便签 10041
    if c.device_fp.is_empty() {
        if let Some(old) = cookies::load_with_migration(&app) {
            c.device_fp = old.device_fp;
        }
    }

    // ③ 设备标识切换（备份 + 失败回滚）
    let old_dev = config::device_id(&app);
    let dev = match dev_arg {
        Some(d) => d,
        None => {
            crate::debug_log::log(
                "login_with_cookies: 未提供 device_id → 沿用当前设备标识（仅适用于 WebView2 扫码会话）",
            );
            old_dev.clone()
        }
    };
    if dev != old_dev {
        config::set_device_id(&app, &dev);
    }
    let fp_desc = if c.device_fp.is_empty() {
        "<空>".to_string()
    } else {
        format!("…{}", &c.device_fp[..c.device_fp.len().min(6)])
    };
    crate::debug_log::log(&format!(
        "login_with_cookies: 尝试导入会话 device_id={dev}（原 {old_dev}）· device_fp={fp_desc} · cookie 字段={:?}",
        {
            let mut ks: Vec<String> = c.to_map().keys().cloned().collect();
            ks.sort();
            ks
        }
    ));

    // ④ 先验证再落盘（网络失败 / 服务端拒绝一律拒绝写入，防止污染既有会话）
    let api = MihoyoApi::new(&app, &c);
    let test = api.test_cookies().await;
    if !matches!(test, Ok(true)) {
        let reason = match &test {
            Ok(_) => "服务端判定会话无效".to_string(),
            Err(e) => e.to_string(),
        };
        if dev != old_dev {
            config::set_device_id(&app, &old_dev);
        }
        crate::debug_log::log(&format!(
            "login_with_cookies: 导入失败，已回滚 device_id → {old_dev}：{reason}"
        ));
        return Err(format!(
            "导入失败（已回滚设备标识，当前登录未受影响）：{reason}\n\
             若只搬了 cookie、未一并提供抓包里的 x-rpc-device_id，会被判登录失效(-100)。"
        ));
    }
    cookies::save(&app, &c).map_err(|e| e.to_string())?;
    *state.client.lock().unwrap() = Some(api.clone());
    *state.accounts.lock().unwrap() = None;
    crate::debug_log::log("login_with_cookies: 会话验证通过，cookie 已落盘");

    // ⑤ 一次性自检：**仅在原神便签未冻结时**才请求（v1.3.0 冻结语义一致——绝不偷偷发战绩域请求；
    //    属用户显式点击触发的一次性检查，不属于"自动刷新"）
    let note_check = if config::load(&app).genshin_note_enabled {
        match load_accounts(&app, &state).await {
            Ok(accounts) => {
                match crate::api::notes::get_note(&app, &api, &accounts, "genshin").await {
                    Ok(_) => "原神便签已可读取".to_string(),
                    Err(e) => format!("原神便签仍不可用：{e}"),
                }
            }
            Err(e) => format!("账号列表获取失败：{e}"),
        }
    } else {
        "会话验证通过。原神便签当前处于冻结状态，未发起任何战绩域请求；\
         如需自检原神便签，请先在设置中开启「原神便签」"
            .to_string()
    };
    crate::debug_log::log(&format!("login_with_cookies: 导入后自检 → {note_check}"));
    Ok(serde_json::json!({ "ok": true, "note_check": note_check }))
}

// ---------------------------------------------------------------------------
// App 身份会话登录（官方「米游社扫码登录」flow，2026-10-15 Stage 1）
//
// 背景与方法依据见 `api/app_login.rs` 模块头 + 报告 `15_路线A实战记录_1015.md`。
// ⚠️ 风控面：只打 passport-api.miyoushe.com / public-data-api.mihoyo.com，
//    **不属于战绩域**，不触碰"仅手动刷新"铁律。
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct AppLoginStart {
    /// 二维码内容 —— 由**前端**渲染成二维码图片
    /// （⚠️ 实测：该 URL 对应的官方页面**不会**渲染二维码，只回一张静态提示图）
    pub url: String,
    /// 查询扫码状态用（前端不需要它，仅便于诊断）
    pub ticket: String,
}

/// ① 开始 App 会话登录：生成新设备身份 → 创建扫码会话 → 后台轮询 → 返回二维码内容。
///
/// 为什么"新设备身份"：`x-rpc-device_id` 与 v2 会话绑定，必须与本会话成套；
/// 且新设备会在通行证侧**登记**（用户会收到官方「新设备登录」邮件，设备名 = `DEVICE_NAME`）。
#[tauri::command]
pub async fn start_app_login(
    app: AppHandle,
    state: State<'_, ApiState>,
) -> Result<AppLoginStart, String> {
    // 若已有进行中的流程（例如用户关了弹窗没点取消），**先发取消信号再重开**，
    // 避免"卡住 5 分钟不能重试"的死角（宽进：不报错，只记日志）。
    {
        let prev = state.app_login_abort.lock().unwrap().clone();
        if let Some(a) = prev {
            a.store(true, std::sync::atomic::Ordering::Relaxed);
            crate::debug_log::log("start_app_login: 检测到上一次流程仍在进行，已发取消信号后重开");
        }
    }
    match start_app_login_inner(&app, &state).await {
        Ok(v) => Ok(v),
        Err(e) => {
            *state.app_login_busy.lock().unwrap() = false;
            *state.app_login_abort.lock().unwrap() = None;
            Err(e)
        }
    }
}

async fn start_app_login_inner(
    app: &AppHandle,
    state: &State<'_, ApiState>,
) -> Result<AppLoginStart, String> {
    let device_id = uuid::Uuid::new_v4().to_string();
    let client = crate::api::app_login::client();
    let device_fp = crate::api::app_login::get_fp(&client, &device_id).await;
    crate::debug_log::log(&format!(
        "start_app_login: 新设备身份 device_id={device_id} device_name={}",
        crate::api::app_login::DEVICE_NAME
    ));

    let qr = crate::api::app_login::create_qr(&client, &device_id, &device_fp)
        .await
        .map_err(|e| e.to_string())?;
    crate::debug_log::log(&format!(
        "start_app_login: 扫码会话已创建 ticket={} url={}",
        qr.ticket, qr.url
    ));

    let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    *state.app_login_busy.lock().unwrap() = true;
    *state.app_login_abort.lock().unwrap() = Some(abort.clone());

    let app2 = app.clone();
    // ⚠️ ticket 既要给后台轮询、又要返回前端 → 必须 clone（曾漏 clone 导致 E0382 move 错误）
    let ticket_for_poll = qr.ticket.clone();
    tauri::async_runtime::spawn(async move {
        poll_app_login(app2, client, device_id, device_fp, ticket_for_poll, abort).await;
    });

    Ok(AppLoginStart { url: qr.url, ticket: qr.ticket })
}

/// ② 取消 App 会话登录（轮询循环每 2s 检查一次取消信号）
#[tauri::command]
pub fn cancel_app_login(state: State<'_, ApiState>) -> Result<(), String> {
    if let Some(a) = state.app_login_abort.lock().unwrap().as_ref() {
        a.store(true, std::sync::atomic::Ordering::Relaxed);
        crate::debug_log::log("cancel_app_login: 已发出取消信号");
    }
    Ok(())
}

/// 轮询扫码状态直至 `Confirmed`；无论成败都会释放 `app_login_busy` 并向前端发事件。
async fn poll_app_login(
    app: AppHandle,
    client: reqwest::Client,
    device_id: String,
    device_fp: String,
    ticket: String,
    abort: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let deadline = std::time::Instant::now()
        + Duration::from_secs(crate::api::app_login::POLL_TIMEOUT_SECS);
    let mut last_status = String::new();
    let mut cookies: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut consecutive_err = 0u32;
    let mut result: Result<(), String> = Err("等待超时：未在时限内完成扫码确认".into());

    while std::time::Instant::now() < deadline {
        if abort.load(Ordering::Relaxed) {
            result = Err("已取消".into());
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        match crate::api::app_login::query_qr(&client, &device_id, &device_fp, &ticket).await {
            Ok(p) => {
                consecutive_err = 0;
                if !p.cookies.is_empty() {
                    cookies.extend(p.cookies);
                }
                if p.status != last_status {
                    crate::debug_log::log(&format!(
                        "app_login: 状态 {} → {}",
                        if last_status.is_empty() { "(空)" } else { &last_status },
                        p.status
                    ));
                    last_status = p.status.clone();
                }
                if p.status == "Confirmed" {
                    // 再补一次请求：凭证可能分散在多次响应的 Set-Cookie（实测行为）
                    if let Ok(p2) =
                        crate::api::app_login::query_qr(&client, &device_id, &device_fp, &ticket).await
                    {
                        if !p2.cookies.is_empty() {
                            cookies.extend(p2.cookies);
                        }
                    }
                    result = finish_app_login(&app, &device_id, &device_fp, &cookies).await;
                    break;
                }
            }
            Err(e) => {
                consecutive_err += 1;
                crate::debug_log::log(&format!("app_login: 轮询失败({consecutive_err}) {e}"));
                // 连续失败视为流程已死（如二维码失效 -3501 / 取消 -3505），避免空转 5 分钟
                if consecutive_err >= 3 {
                    result = Err(e.to_string());
                    break;
                }
            }
        }
    }

    // ⚠️ 出口 + TTL 兜底：无论何种路径都释放标记（纪律：pending 标记不得永久残留）
    if let Some(st) = app.try_state::<ApiState>() {
        *st.app_login_busy.lock().unwrap() = false;
        *st.app_login_abort.lock().unwrap() = None;
    }

    match result {
        Ok(()) => {
            crate::debug_log::log("app_login: 成功，已通知前端");
            let _ = app.emit("app-login-success", serde_json::json!({}));
        }
        Err(msg) => {
            crate::debug_log::log(&format!("app_login: 失败 → {msg}"));
            let _ = app.emit("app-login-failed", serde_json::json!({ "message": msg }));
        }
    }
}

/// 确认成功后收尾：换设备标识 → **先验证再落盘** → 刷新客户端缓存。
///
/// ⚠️ 与 `login_with_cookies` 同一纪律：验证失败**回滚 device_id**，不破坏当前登录。
///    （v2 会话与设备绑定，只换 cookie 不换设备会被判 `-100`；反之亦然。）
async fn finish_app_login(
    app: &AppHandle,
    device_id: &str,
    device_fp: &str,
    raw: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let mut c = cookies::Cookies::from_map(raw);
    if c.is_empty() {
        return Err("确认成功但响应未带任何凭证 cookie（Set-Cookie 解析为空）".into());
    }
    // 设备指纹以**本次登录所用**的 fp 为准（它的注册锚就是本 device_id）
    if !device_fp.is_empty() {
        c.device_fp = device_fp.to_string();
    }
    let old_dev = config::device_id(app);
    config::set_device_id(app, device_id);
    let api = MihoyoApi::new(app, &c);
    match api.test_cookies().await {
        Ok(true) => {}
        other => {
            config::set_device_id(app, &old_dev);
            let reason = match other {
                Ok(_) => "服务端判定会话无效".to_string(),
                Err(e) => e.to_string(),
            };
            return Err(format!("App 会话验证失败（已回滚设备标识，当前登录未受影响）：{reason}"));
        }
    }
    cookies::save(app, &c).map_err(|e| e.to_string())?;
    let mut keys: Vec<String> = c.to_map().keys().cloned().collect();
    keys.sort();
    crate::debug_log::log(&format!(
        "app_login: App 会话已落盘（device_id={device_id} · cookie 字段={keys:?}）"
    ));
    if let Some(st) = app.try_state::<ApiState>() {
        *st.client.lock().unwrap() = Some(api);
        *st.accounts.lock().unwrap() = None;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_login_status(
    app: AppHandle,
    state: State<'_, ApiState>,
) -> Result<serde_json::Value, String> {
    let has = cookies::has(&app);
    // ⚠️ 三态判定（2026-08-31 修复）：此前 test_cookies() 返回 bool，网络/暂时性失败
    //    与"服务端拒绝登录态"混为一谈 → 10 分钟周期探测在网络抖动时误判"登录已过期"，
    //    clearSession 后主页面错显示"未登录"（而设置页真实校验通过显示"已登录"）。
    //    现区分：valid=true 仅表示"文件存在且服务端接受"；网络失败不算失效。
    let Some(api) = load_api(&app, &state) else {
        return Ok(serde_json::json!({ "has_cookies": has, "valid": false }));
    };
    match api.test_cookies().await {
        Ok(_) => Ok(serde_json::json!({ "has_cookies": has, "valid": true })),
        Err(e) if e.is_session_expired() => {
            crate::debug_log::log(&format!("get_login_status: 登录态已失效（{e}）"));
            // cookie 失效（服务端拒绝）：清缓存，让前端引导重新登录
            *state.client.lock().unwrap() = None;
            *state.accounts.lock().unwrap() = None;
            Ok(serde_json::json!({ "has_cookies": has, "valid": false }))
        }
        Err(e) if e.is_network() => {
            // 网络/暂时性失败：不算失效，保持"已登录"（valid=has_cookies），
            // 附带 unreachable 标记供前端诊断（如需要）
            crate::debug_log::log(&format!("get_login_status: 网络/暂时性失败，不判失效（{e}）"));
            Ok(serde_json::json!({ "has_cookies": has, "valid": has, "unreachable": true }))
        }
        Err(e) => {
            // 其他非会话错误（参数/风控等）：保守不判失效
            crate::debug_log::log(&format!("get_login_status: 非会话错误，不判失效（{e}）"));
            Ok(serde_json::json!({ "has_cookies": has, "valid": has, "unreachable": true }))
        }
    }
}

/// 解析 "k=v; k2=v2" 形式的 cookie 字符串
pub fn parse_cookie_str(s: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for part in s.split(';') {
        let idx = match part.find('=') {
            Some(i) => i,
            None => continue,
        };
        let k = part[..idx].trim();
        let v = part[idx + 1..].trim();
        if !k.is_empty() && !v.is_empty() {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

/// 扫码登录成功后统一收尾：保存 cookie → 刷新 API 客户端 → 通知前端 → 关闭登录窗口
/// 由两个入口调用：① start_qr_login 的 WebView CookieManager 轮询（主路径，可读 HttpOnly）；
/// ② lib.rs handle_qr_state 注入脚本回传（兜底路径）。
pub async fn apply_login_cookie(app: &AppHandle, map: std::collections::HashMap<String, String>) {
    let mut c = cookies::Cookies::from_map(&map);
    if c.is_empty() {
        crate::debug_log::log("扫码登录: cookie 为空，跳过");
        return;
    }
    // 补全 mid（v2 体系字段为 account_mid_v2 / ltmid_v2）
    if c.mid.is_empty() {
        if let Some(m) = map.get("account_mid_v2").or_else(|| map.get("ltmid_v2")) {
            c.mid = m.clone();
        }
    }
    // 用扫码窗口的设备 uuid 覆盖 device_id，保证 v2 会话与请求设备一致
    if let Some(d) = map.get("_MHYUUID").or_else(|| map.get("DEVICEFP_SEED_ID")) {
        crate::storage::config::set_device_id(app, d);
        crate::debug_log::log(&format!("扫码登录: device_id 已绑定 {d}"));
    }
    // 设备指纹（DEVICEFP cookie）：绝区零便签走 event 活动域接口，不带此值触发
    // 设备风控 10041（实测验证：带 fp=DEVICEFP 后 retcode=0 全字段返回）。
    // 本版本不请求战绩域，但仍保留该字段（后续功能恢复时直接可用）。
    if let Some(fp) = map.get("DEVICEFP") {
        c.device_fp = fp.clone();
        crate::debug_log::log("扫码登录: 已保存设备指纹 DEVICEFP");
    }
    if cookies::save(app, &c).is_err() {
        crate::debug_log::log("扫码登录: cookie 保存失败");
        return;
    }
    // 刷新 API 客户端缓存（作用域结束即释放借用）
    {
        if let Some(state) = app.try_state::<ApiState>() {
            *state.client.lock().unwrap() = Some(MihoyoApi::new(app, &c));
            *state.accounts.lock().unwrap() = None; // 账号缓存失效，下次拉取
        }
    }
    let _ = app.emit("login-success", ());
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.destroy();
    }
    let uid = if !c.ltuid_v2.is_empty() { &c.ltuid_v2 } else { &c.ltuid };
    crate::debug_log::log(&format!("扫码登录成功，cookie 已保存（ltuid={uid}）"));
}

/// 退出登录：删除本地 cookie 并清空客户端缓存
#[tauri::command]
pub fn logout(app: AppHandle, state: State<'_, ApiState>) -> Result<(), String> {
    cookies::delete(&app);
    *state.client.lock().unwrap() = None;
    *state.accounts.lock().unwrap() = None;
    crate::debug_log::log("已退出登录（cookie 已删除）");
    Ok(())
}

/// 开机自启：联动系统注册（autostart 插件）并同步 config 标志
/// ⚠️ 2026-08-30 修复：此前前端开关只写 config.auto_start 标志，
///    从未调用 autostart 插件 → 系统层面开机自启从未生效。
/// ⚠️ 2026-08-30 二次修复：debug 构建是 console 子系统（弹命令行窗口）且
///    编译时嵌入 devUrl（依赖 `npm run dev`，重启后必白屏）→
///    绝不允许 debug 构建写入自启动注册表（此前已因此污染 Run 键一次）。
#[tauri::command]
pub fn set_auto_start(app: AppHandle, enabled: bool) -> Result<bool, String> {
    if enabled && cfg!(debug_assertions) {
        return Err(
            "当前为开发版（debug），无法用于开机自启（无独立前端资源）。请使用正式版开启此功能。"
                .into(),
        );
    }
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| format!("启用开机自启失败: {e}"))?;
    } else {
        manager.disable().map_err(|e| format!("禁用开机自启失败: {e}"))?;
    }
    // 同步 config 标志（保持与系统状态一致）
    let mut cfg = config::load(&app);
    cfg.auto_start = enabled;
    let _ = config::save(&app, &cfg);
    crate::debug_log::log(&format!("开机自启已{}", if enabled { "启用" } else { "禁用" }));
    Ok(enabled)
}

/// 查询系统开机自启真实状态（回写 config 保证一致）
#[tauri::command]
pub fn get_auto_start(app: AppHandle) -> Result<bool, String> {
    let enabled = app
        .autolaunch()
        .is_enabled()
        .map_err(|e| format!("查询开机自启状态失败: {e}"))?;
    // ⚠️ 2026-08-30 二次修复：自校验注册表 Run 键指向的是否当前 exe。
    //    此前 debug 构建残留指向 debug exe（console 子系统 + devUrl 依赖）
    //    → 重启弹命令行窗口 + 白屏。绿色版移动后同样会路径漂移，此处兜底纠正。
    repair_autostart_registry_path();
    let mut cfg = config::load(&app);
    cfg.auto_start = enabled;
    let _ = config::save(&app, &cfg);
    Ok(enabled)
}

/// 自校验自启动注册表路径：HKCU\...\Run\MiHoYoWidget 若指向的不是当前进程 exe
/// （debug 残留 / 绿色版移动），则静默纠正为当前 exe。任何失败静默忽略，不阻塞调用。
fn repair_autostart_registry_path() {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_SET_VALUE, REG_NONE, REG_SZ, REG_VALUE_TYPE,
    };
    // debug 构建无法独立运行（依赖 devUrl），开启设置时绝不允许它改写注册表，
    // 否则会把正式版的自启动项覆盖成 debug 路径（弹窗+白屏）。修复动作只由正式版执行。
    if cfg!(debug_assertions) {
        return;
    }

    let Ok(cur) = std::env::current_exe() else { return };
    let cur_display = cur.display().to_string();

    unsafe {
        let mut hkey: HKEY = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            KEY_READ | KEY_SET_VALUE,
            &mut hkey,
        )
        .ok()
        .is_err()
        {
            return;
        }
        // 读当前注册表值
        let mut buf = [0u16; 4096];
        let mut cb = (buf.len() * 2) as u32;
        let mut ty = REG_NONE;
        let mut reg_value: Option<String> = None;
        let query_err = RegQueryValueExW(
            hkey,
            w!("MiHoYoWidget"),
            None,
            Some(&mut ty as *mut REG_VALUE_TYPE),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut cb as *mut u32),
        );
        if query_err.ok().is_ok() && ty == REG_SZ && cb >= 2 {
            let chars = (cb as usize / 2).saturating_sub(1); // 去结尾 NUL
            reg_value = Some(String::from_utf16_lossy(&buf[..chars]));
        }
        let needs_fix = match &reg_value {
            // 剥掉首尾空格与引号后与当前 exe 对比
            Some(v) => v.trim().trim_matches('"') != cur_display.as_str(),
            None => false, // 键不存在：从未启用，无需处理
        };
        if needs_fix {
            let fixed = format!("\"{}\"", cur_display);
            let data: Vec<u16> = fixed.encode_utf16().chain([0]).collect();
            let slice = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
            let _ = RegSetValueExW(hkey, w!("MiHoYoWidget"), None, REG_SZ, Some(slice));
        }
        let _ = RegCloseKey(hkey);
    }
}

/// 启动扫码登录：打开登录窗口（WebView 加载米游社登录页）
/// 登录成功后由 Rust 事件处理器自动提取 cookie、保存并通知前端
#[tauri::command]
pub fn start_qr_login(app: AppHandle) -> Result<(), String> {
    // 已存在则聚焦
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }

    // 注入脚本：每 1.5s 读取 URL + document.cookie 并通过 Tauri 事件回传
    let script = r#"
    (function(){
      function send(){
        try{
          var url = window.location.href || '';
          var cookie = document.cookie || '';
          if(window.__TAURI__ && window.__TAURI__.event){
            window.__TAURI__.event.emit('qr-login-state', {url: url, cookie: cookie});
          }
        }catch(e){}
      }
      setInterval(send, 1500);
      send();
    })();
    "#;

    let url = "https://user.mihoyo.com/#/login"
        .parse::<tauri::Url>()
        .map_err(|e| e.to_string())?;

    // 异步创建窗口：WebView2 初始化可能较慢，同步执行会阻塞主线程导致整个应用卡死
    tauri::async_runtime::spawn(async move {
        crate::debug_log::log("start_qr_login: 开始创建登录窗口");
        match tauri::WebviewWindowBuilder::new(&app, "login", tauri::WebviewUrl::External(url))
            .title("扫码登录米游社")
            .inner_size(440.0, 620.0)
            .center()
            .resizable(false)
            .initialization_script(script)
            // 导航日志：定位空白页问题（成功/失败）
            .on_navigation(|url| {
                crate::debug_log::log(&format!("login 导航: {url}"));
                true
            })
            .build()
        {
            Ok(w) => {
                crate::debug_log::log("start_qr_login: 登录窗口创建成功");
                // × 按钮：销毁窗口（防止残留导致下次无法重新打开）
                let w2 = w.clone();
                w.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        crate::debug_log::log("login 窗口关闭请求");
                        api.prevent_close();
                        let _ = w2.destroy();
                    }
                });
                // 登录成功检测（主路径）：轮询 URL + WebView2 CookieManager。
                // 关键点：WebView2 cookie 库持久化，残留上次登录的旧 cookie，
                // 仅凭 ltoken 存在会误判（窗口一开就"登录成功"）；必须同时满足
                // URL 已离开 /login（扫码成功会跳转）才算成功。
                let w_poll = w.clone();
                let app_poll = app.clone();
                tauri::async_runtime::spawn(async move {
                    // 轮询直到窗口销毁（用户取消 × 或登录成功已关窗）。
                    // ⚠️ 不用固定次数：扫码可能超过 3.7 分钟，固定轮询会提前停止，
                    //    之后扫码成功也无法自动提取 cookie（实测体验问题）。
                    loop {
                        // 每 1.5s 一次
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        let url = match w_poll.url() {
                            Ok(u) => u.to_string(),
                            Err(_) => {
                                crate::debug_log::log("扫码登录: 轮询停止（login 窗口不可用）");
                                break; // 窗口已销毁：用户取消或登录成功已关窗
                            }
                        };
                        if url.contains("/login") {
                            continue; // 仍在登录页：忽略残留 cookie，继续等扫码跳转
                        }
                        let cookies = match w_poll.cookies() {
                            Ok(c) => c,
                            Err(_) => break,
                        };
                        let mut map = std::collections::HashMap::new();
                        for c in &cookies {
                            let domain = c.domain().unwrap_or("");
                            if domain.contains("mihoyo.com") || domain.contains("hoyoverse.com") {
                                map.insert(c.name().to_string(), c.value().to_string());
                            }
                        }
                        if map.contains_key("ltoken") || map.contains_key("ltoken_v2") {
                            crate::debug_log::log(&format!(
                                "扫码登录: 检测到登录成功（URL={url}，WebView {} 个 cookie，提取 {} 个）",
                                cookies.len(),
                                map.len()
                            ));
                            apply_login_cookie(&app_poll, map).await;
                            break;
                        }
                    }
                });
            }
            Err(e) => crate::debug_log::log(&format!("start_qr_login: 登录窗口创建失败: {e}")),
        }
    });
    Ok(())
}

/// 启用/禁用游戏、透明度的统一配置读写
#[tauri::command]
pub fn get_config(app: AppHandle) -> Result<serde_json::Value, String> {
    Ok(serde_json::to_value(config::load(&app)).map_err(|e| e.to_string())?)
}

#[tauri::command]
pub fn update_config(
    app: AppHandle,
    key: String,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let cfg = config::update(&app, &key, value);
    Ok(serde_json::to_value(cfg).map_err(|e| e.to_string())?)
}

/// 绑定账号列表（按配置启用 + 注册表过滤）
/// ⚠️ 登录状态三态约定（2026-08-31 修复）——前端 init() 据此区分"真未登录"与"暂时性失败"：
/// - 无 cookie（load_api=None）          → `{games:[], logged_in:false}`（真未登录/文件损坏）
/// - 服务端拒绝登录态（is_session_expired） → `{games:[], logged_in:false}`（登录过期，引导重新登录）
/// - 网络/暂时性失败                      → Err（前端**保留登录态**、提示并自动重试，
///                                            绝不误判"未登录"——此前任何错误都返回 logged_in:false，
///                                            开机自启网络未就绪时主页面错显示"未登录"）
#[tauri::command]
pub async fn get_games(
    app: AppHandle,
    state: State<'_, ApiState>,
) -> Result<serde_json::Value, String> {
    // ① 无 cookie（或解密失败）：明确"未登录"，非错误
    if load_api(&app, &state).is_none() {
        crate::debug_log::log("get_games: 无 cookie（未登录）");
        return Ok(serde_json::json!({ "games": [], "logged_in": false }));
    }
    // ② 有 cookie：拉账号列表；失败按"会话失效 / 暂时性失败"分流
    let accounts = match load_accounts(&app, &state).await {
        Ok(a) => a,
        Err(e) => {
            crate::debug_log::log(&format!("get_games 失败: {e}"));
            // 账号缓存失效（下次调用重新拉取，避免 60s 内继续返回旧账号）
            *state.accounts.lock().unwrap() = None;
            let msg = e.to_string();
            if msg == "未登录" || msg.contains("登录态不足") {
                // 服务端拒绝登录态 / cookie 文件损坏 → 登录过期
                return Ok(serde_json::json!({ "games": [], "logged_in": false }));
            }
            // 网络/暂时性失败 → 抛错，前端保留登录态并提示（不误判"未登录"）
            return Err(msg);
        }
    };
    crate::debug_log::log(&format!("get_games 成功，账号数: {}", accounts.len()));
    let cfg = config::load(&app);
    let games: Vec<serde_json::Value> = accounts
        .iter()
        .filter(|a| cfg.games.contains(&a.game_id))
        .map(|a| serde_json::json!({
            "game_id": a.game_id,
            "game_name": a.game_name,
            "game_icon": crate::api::find_spec(&a.game_id).map(|s| s.icon).unwrap_or(""),
            "uid": a.uid,
            "nickname": a.nickname,
            "server": a.server,
            "level": a.level,
        }))
        .collect();
    Ok(serde_json::json!({ "games": games, "logged_in": true }))
}

/// 签到信息（服务端权威，失败返回错误而非假 0）
#[tauri::command]
pub async fn get_sign_info(
    app: AppHandle,
    state: State<'_, ApiState>,
    game_id: String,
) -> Result<serde_json::Value, String> {
    let api = load_api(&app, &state).ok_or_else(|| "未登录".to_string())?;
    let accounts = load_accounts(&app, &state).await?;
    let acc = accounts
        .iter()
        .find(|a| a.game_id == game_id)
        .ok_or_else(|| format!("未找到 {game_id} 账号"))?;
    let info = api.get_sign_info(&game_id, &acc.uid, &acc.server).await.map_err(|e| {
        // 签到状态获取失败必须留痕（定位"显示未签到但实际已签到"类问题）
        crate::debug_log::log(&format!("get_sign_info({game_id}) 失败: {e}"));
        e.to_string()
    })?;
    crate::debug_log::log(&format!(
        "get_sign_info({game_id}) 成功: signed={} total_days={} missed={}",
        info.signed, info.total_days, info.missed
    ));
    Ok(serde_json::json!({
        "game_id": game_id,
        "signed": info.signed,
        "total_days": info.total_days,
        "missed": info.missed,
    }))
}

#[tauri::command]
pub async fn manual_sign_in(
    app: AppHandle,
    state: State<'_, ApiState>,
    game_id: String,
) -> Result<serde_json::Value, String> {
    let api = load_api(&app, &state).ok_or_else(|| "未登录".to_string())?;
    let accounts = load_accounts(&app, &state).await?;
    let acc = accounts
        .iter()
        .find(|a| a.game_id == game_id)
        .ok_or_else(|| format!("未找到 {game_id} 账号"))?;
    let info = api
        .sign_in(&game_id, &acc.uid, &acc.server)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "game_id": game_id,
        "signed": info.signed,
        "total_days": info.total_days,
    }))
}

/// 获取每日便签（**战绩域 · 仅手动刷新**）
///
/// # 铁律（改动前必读）
/// 本命令是**唯一**的战绩域读取入口，且**只允许**由前端「刷新」按钮显式调用。
/// 禁止在以下任何路径中调用：
/// - 定时器 / 周期任务（`App.tsx` 的 15 分钟签到刷新、Rust 调度器）
/// - `init()` / `refreshAll()` / 切页签 / 打开关闭设置页 / 登录成功回调
///
/// 原因：战绩域是**滑动窗口风控**，每次请求都重新计时。历史两次事故（08-30「5 分钟刷新
/// 导致 1034 永续」、09-26「切页签 force 击穿退避」）都是"自动/交互路径打战绩域"造成的。
/// 2026-09-26 ~ 10-14 停止全部战绩域请求 18 天后，风控窗口才自然关闭（zzz 实测 retcode=0）。
#[tauri::command]
pub async fn get_daily_note(
    app: AppHandle,
    state: State<'_, ApiState>,
    game_id: String,
) -> Result<serde_json::Value, String> {
    // ⚠️ v1.3.0「原神便签冻结」：后端硬门禁——冻结时**直接拒绝且不发任何网络请求**。
    //    前端已有两道门（不渲染入口 + store 拦截），此处是第三道（防绕过前端直接调用）。
    //    错误串纪律：不得含"未登录/登录态不足"等关键词（否则前端会误判会话失效）。
    if game_id == "genshin" && !config::load(&app).genshin_note_enabled {
        crate::debug_log::log("get_daily_note(genshin): 已冻结，拒绝请求（未发出任何战绩域请求）");
        return Err(
            "原神便签已冻结（v1.3.0）：米游社现已要求 App 身份会话才能读取，本应用为网页身份，\
             请求必被挡下且验证无法通过。可在设置中开启「原神便签」以解除冻结。"
                .into(),
        );
    }
    let api = load_api(&app, &state).ok_or_else(|| "未登录".to_string())?;
    let accounts = load_accounts(&app, &state).await?;
    crate::api::notes::get_note(&app, &api, &accounts, &game_id).await
}

// ============================================================================
// geetest 验证码链路（2026-10-14 接线）
//
// 触发前提：便签请求返回 1034（CAPTCHA_REQUIRED）——此时 `api/notes.rs` 已把该次
//           响应的 `x-trace-id` 存进 `ApiState.trace_map[game_id]`。
// 全链路（**只允许用户显式点击「去验证」触发，禁止任何自动路径**）：
//   ① 前端「去验证」→ get_verification_challenge（createVerification，战绩域 1 次）
//   ② frontend → open_captcha_window（加载官方域宿主页，注入 __WGT__ + captcha_inject.js）
//   ③ 用户在滑块窗口完成验证 → 浏览器 fetch verifyVerification
//        失败则 fallback → verify_captcha_fallback（Rust 完整请求）
//   ④ 换取 x-rpc-challenge token → emit captcha-result → 落盘 challenge.json（单次使用）
//   ⑤ 前端提示"验证已完成，请点「刷新」"——⚠️ 由**用户再次点击**才发便签请求（铁律）
//
// ⚠️ 事件命名纪律（防自循环）：captcha 窗口 → Rust 的四个事件是
//    `captcha-loaded` / `captcha-diagnose` / `captcha-error` / `captcha-result`；
//    Rust → 主窗口一律用**不同名**的 `captcha-success` / `captcha-failed`，
//    避免 Rust 侧监听到自己 emit 的事件形成回环。
// ============================================================================

/// 该游戏是否仍处于验证码挑战冻结态
fn trace_pending(app: &AppHandle, game_id: &str) -> bool {
    app.try_state::<ApiState>()
        .map(|s| s.captcha_pending.lock().map(|p| p.contains(game_id)).unwrap_or(false))
        .unwrap_or(false)
}

/// 解除冻结（**每一处清除路径都必须调用**，否则 trace_map 永久冻结）
pub(crate) fn unfreeze_trace(app: &AppHandle, game_id: &str) {
    if let Some(state) = app.try_state::<ApiState>() {
        if let Ok(mut p) = state.captcha_pending.lock() {
            p.remove(game_id);
        }
    }
}

/// 冻结 trace（挑战期间 notes.rs 不得覆盖它）并注册**两段式 TTL 看门狗**。
///
/// ⚠️ 状态机纪律：任何"冻结/pending 标记"必须有出口 + TTL 兜底。
///    历史事故：create 成功但开窗失败 → 标记永久残留 → trace 永不更新 → 所有验证码请求恒失败、
///    用户重试无效、**只能重启应用**。看门狗**只在仍处于 pending 时**动作，不误伤正常流程。
fn freeze_trace_with_ttl(app: AppHandle, game_id: String) {
    if let Some(state) = app.try_state::<ApiState>() {
        if let Ok(mut p) = state.captcha_pending.lock() {
            p.insert(game_id.clone());
        }
    }
    tauri::async_runtime::spawn(async move {
        // 阶段 1：5s 内滑块窗口未打开 → 解冻 + 通知
        tokio::time::sleep(Duration::from_secs(5)).await;
        if !trace_pending(&app, &game_id) {
            return;
        }
        if app.get_webview_window("captcha").is_none() {
            unfreeze_trace(&app, &game_id);
            crate::debug_log::log(&format!("看门狗: {game_id} 滑块窗口未打开，已解冻 trace"));
            let _ = app.emit(
                "captcha-failed",
                serde_json::json!({ "game_id": game_id, "message": "滑块窗口未打开，请重试" }),
            );
            return;
        }
        // 阶段 2：窗口已存在 → 再等 85s（累计 90s）仍未完成 → 解冻 + 关窗
        tokio::time::sleep(Duration::from_secs(85)).await;
        if !trace_pending(&app, &game_id) {
            return;
        }
        unfreeze_trace(&app, &game_id);
        crate::debug_log::log(&format!("看门狗: {game_id} 验证超时（90s），已解冻 trace"));
        let _ = app.emit(
            "captcha-failed",
            serde_json::json!({ "game_id": game_id, "message": "验证超时（90 秒），请重试" }),
        );
        if let Some(w) = app.get_webview_window("captcha") {
            let _ = w.hide();
        }
    });
}

/// ① 获取 geetest 挑战参数（gt + challenge）——由前端「去验证」按钮触发
///
/// ⚠️ 依赖先有一次便签请求产生的 `x-trace-id`：若缺失说明用户未先点「刷新」，
///    直接报错让用户先刷新（**不代替用户自动补发便签请求**，遵守"仅手动刷新"铁律）。
#[tauri::command]
pub async fn get_verification_challenge(
    app: AppHandle,
    state: State<'_, ApiState>,
    game_id: String,
) -> Result<serde_json::Value, String> {
    let api = load_api(&app, &state).ok_or_else(|| "未登录".to_string())?;
    let spec = crate::api::find_spec(&game_id).ok_or_else(|| format!("未注册游戏: {game_id}"))?;
    let challenge_path = crate::api::captcha::challenge_path_for(&game_id);
    let trace = state
        .trace_map
        .lock()
        .ok()
        .and_then(|m| m.get(&game_id).cloned())
        .filter(|t| !t.is_empty());
    let Some(trace) = trace else {
        return Err("缺少验证上下文（x-trace-id）。请先点「刷新」触发便签请求，再点「去验证」。".into());
    };
    // 冻结 trace：挑战进行中不得被后续便签刷新覆盖（覆盖会使 verify 脱钩）
    freeze_trace_with_ttl(app.clone(), game_id.clone());
    let (gt, challenge) = crate::api::captcha::create_verification(
        &api,
        spec.sign_game_header,
        &challenge_path,
        Some(&trace),
    )
    .await?;
    crate::debug_log::log(&format!("get_verification_challenge({game_id}): 已创建挑战"));
    Ok(serde_json::json!({ "gt": gt, "challenge": challenge }))
}

/// ② 打开滑块窗口（官方域宿主 + 注入 `__WGT__` 与 `captcha_inject.js`）
///
/// 宿主 URL 取自 v1.1 实测可用的形态（从当时 WebView2 历史记录还原）：
/// `https://webstatic.mihoyo.com/app/community-game-records/rpg/?game_id=6&ref=paimon&mhy_presentation_style=fullscreen&w_game=..&w_gt=..&w_challenge=..`
/// 借官方域身份通过 geetest 的前置校验；页面本身会被注入脚本 `document.write` 立即替换。
#[tauri::command]
pub async fn open_captcha_window(
    app: AppHandle,
    state: State<'_, ApiState>,
    game_id: String,
    gt: String,
    challenge: String,
) -> Result<(), String> {
    let api = load_api(&app, &state).ok_or_else(|| "未登录".to_string())?;
    let spec = crate::api::find_spec(&game_id).ok_or_else(|| format!("未注册游戏: {game_id}"))?;
    let challenge_path = crate::api::captcha::challenge_path_for(&game_id);
    let trace = state
        .trace_map
        .lock()
        .ok()
        .and_then(|m| m.get(&game_id).cloned())
        .unwrap_or_default();
    let cookie_str = api
        .cookie_map
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");

    let wgt = serde_json::json!({
        "game_id": game_id,
        "gt": gt,
        "challenge": challenge,
        "cookie": cookie_str,
        "device_id": api.device_id,
        "device_fp": api.device_fp,
        "trace": trace,
        "challenge_path": challenge_path,
        "signgame": spec.sign_game_header,
    });
    let init_script = format!(
        "window.__WGT__ = {};\n{}",
        serde_json::to_string(&wgt).map_err(|e| e.to_string())?,
        // ⚠️ include_str! 编译期内联：修改 public/captcha_inject.js 后必须 touch 任一 .rs 触发重编译
        include_str!("../../public/captcha_inject.js")
    );

    let url = format!(
        "https://webstatic.mihoyo.com/app/community-game-records/rpg/?game_id=6&ref=paimon&mhy_presentation_style=fullscreen&w_game={}&w_gt={}&w_challenge={}",
        game_id, gt, challenge
    );
    let parsed = url.parse::<tauri::Url>().map_err(|e| e.to_string())?;

    // 复用策略：直接销毁重建，确保 __WGT__ 与 URL 参数不陈旧
    if let Some(w) = app.get_webview_window("captcha") {
        let _ = w.destroy();
    }
    crate::debug_log::log(&format!("open_captcha_window({game_id}): 开始创建滑块窗口"));

    // 异步创建：WebView2 初始化较慢，同步执行会阻塞调用线程
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match tauri::WebviewWindowBuilder::new(
            &app2,
            "captcha",
            tauri::WebviewUrl::External(parsed),
        )
        .title("米游社验证码")
        .inner_size(420.0, 560.0)
        .resizable(false)
        .center()
        .initialization_script(&init_script)
        .build()
        {
            Ok(w) => {
                crate::debug_log::log("open_captcha_window: 滑块窗口创建成功");
                // × 按钮：销毁窗口（防止残留导致下次无法重新打开）
                let w2 = w.clone();
                w.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w2.destroy();
                    }
                });
            }
            Err(e) => {
                crate::debug_log::log(&format!("open_captcha_window: 创建失败 {e}"));
                let _ = app2.emit(
                    "captcha-failed",
                    serde_json::json!({ "message": format!("滑块窗口创建失败: {e}") }),
                );
            }
        }
    });
    Ok(())
}

/// ③ 备用通道：Rust 侧提交滑块结果换取 x-rpc-challenge token
///
/// 由 captcha 窗口的注入脚本在**浏览器 fetch 被拒时**回退调用（remote 窗口调用应用命令
/// 必须经 `capabilities/captcha.json` 的 `allow-verify-captcha-fallback` 授权）。
#[tauri::command]
pub async fn verify_captcha_fallback(
    app: AppHandle,
    state: State<'_, ApiState>,
    game_id: String,
    geetest_challenge: String,
    geetest_validate: String,
    geetest_seccode: String,
) -> Result<String, String> {
    let api = load_api(&app, &state).ok_or_else(|| "未登录".to_string())?;
    let spec = crate::api::find_spec(&game_id).ok_or_else(|| format!("未注册游戏: {game_id}"))?;
    let challenge_path = crate::api::captcha::challenge_path_for(&game_id);
    let trace = state
        .trace_map
        .lock()
        .ok()
        .and_then(|m| m.get(&game_id).cloned());
    crate::debug_log::log(&format!("verify_captcha_fallback({game_id}): Rust 通道提交验证"));
    let token = crate::api::captcha::verify_verification(
        &api,
        &geetest_challenge,
        &geetest_validate,
        &geetest_seccode,
        spec.sign_game_header,
        &challenge_path,
        trace.as_deref(),
    )
    .await?;
    // 落盘（单次使用凭证）；随后 captcha-result 事件会再写一次（幂等）
    let _ = crate::storage::challenge::set(&app, &game_id, &token);
    unfreeze_trace(&app, &game_id);
    Ok(token)
}

// ============================================================================
// 设备标识重置（2026-10-14 新增）
//
// 动因（探针实测，见报告 11_米游社验证码机制调研_1014.md §五之三）：
//   · 用**我们的真实 device_id（= WebView2 的 _MHYUUID）+ 有效 device_fp** → `1034`（要求验证码）
//   · 换成**随机 device_id** → `5003`（fp 随 bbs_device_id 绑定，换 id 即失效）
//   · 同一账号的 QQbot 用自己的设备 → 战绩域**完全正常**
//   ⇒ **服务端是按"设备"区分对待的：本应用这个设备被标记了。**
//   而 `_MHYUUID` / `DEVICEFP` 都源自 WebView2 profile，**重新登录也不会变**——
//   必须清空 WebView2 配置目录（换新 `_MHYUUID`）再重新扫码，设备身份才会真正改变。
//
// 实现：命令侧只写标记 + 删凭证，然后在**下一次启动的最早阶段**（任何 WebView 创建之前）
//       删除 EBWebView 目录（运行时该目录被占用，删不掉），再自动重启。
// ============================================================================

/// 设备重置标记文件（存在即代表"下次启动前先重置设备"）
pub fn reset_device_flag_path(app: &AppHandle) -> std::path::PathBuf {
    crate::storage::config::config_path(app).with_file_name(".reset-device")
}

/// 重置设备标识并重启：用于摆脱"设备级"风控（原神便签 1034）
///
/// ⚠️ 会清除登录状态（cookie）与 WebView2 站点数据 → **需要重新扫码登录**。
#[tauri::command]
pub async fn reset_device_identity(app: AppHandle, state: State<'_, ApiState>) -> Result<(), String> {
    // ① 打标记（下次启动最早期执行目录删除）
    let flag = reset_device_flag_path(&app);
    if let Some(dir) = flag.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&flag, b"1").map_err(|e| format!("写入重置标记失败: {e}"))?;
    // ② 删除本地登录凭证（cookies.enc + 老迁移源）
    cookies::delete(&app);
    // ③ 清掉绑定的 device_id（让新的 _MHYUUID 能被采纳）
    config::set_device_id(&app, "");
    // ④ 清空内存缓存
    *state.client.lock().unwrap() = None;
    *state.accounts.lock().unwrap() = None;
    crate::debug_log::log("reset_device_identity: 已标记设备重置，准备重启");
    // ⑤ 稍后重启（先让 invoke 返回，避免前端拿到"连接中断"）
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        h.restart();
    });
    Ok(())
}

/// 官方战绩页 URL（v1.1 时代 WebView2 History 实测使用过的形态，含 w_gt 供页面自绘极验）
const RECORD_PAGE_URL: &str = "https://webstatic.mihoyo.com/app/community-game-records/rpg/?game_id=6&ref=paimon&mhy_presentation_style=fullscreen&w_game=genshin&w_gt=846fea0fd833b173bec7a25920851739";

/// 打开米游社官方战绩页（**不注入任何脚本**），让站点自己的 JS 走完验证流程。
///
/// 依据：ZZZ OneDragon 的实践纪律「写操作优先浏览器点击（站点自算 DS、自带风控）」
/// 与 Snap Hutao 的手动验证路径（「根据米游社提示进行验证」→ 风控解除）。
///
/// ⚠️⚠️ 2026-10-14 用户实测修正：**直接打开**官方战绩页只会显示
///    「请使用米游社 App 扫码并在移动端查看」的二维码，无法继续。原因是——
///    该页面需要**米游社站点（bbs）登录态**（cookie 域 `miyoushe.com`），
///    而本应用的扫码登录走的是**米哈游通行证**（`user.mihoyo.com`，cookie 域 `mihoyo.com`），
///    只拿到 `ltoken_v2`，**没有** `cookie_token`；两个域互不可见 → 站点判定未登录。
///    （可用实现 ZZZ OneDragon 亦记载：网页扫码登录得到 `cookie_token`，但通常不带 stoken。）
///
/// 故本命令为**两步流程**：
///   ① 先打开 `https://www.miyoushe.com/`，等站点会话建立
///      （WebView 与通行证是不同 cookie 域，若站点弹出自己的登录框，需要用户再扫一次）；
///   ② 检出 `cookie_token` 后（或超时）再导航到官方战绩页。
///
/// 若站点随后弹出**它自己的**验证（点击图形/滑块）并通过，账号/设备的验证状态
/// 有机会随之解除——这是米游社官方认可的手动通道。
#[tauri::command]
pub async fn open_official_page(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("official") {
        let _ = w.destroy();
    }
    // 第一步先落在米游社站点首页（建立 bbs 会话）
    let home = "https://www.miyoushe.com/"
        .parse::<tauri::Url>()
        .map_err(|e| e.to_string())?;
    crate::debug_log::log("open_official_page: ① 先打开米游社站点首页以建立 bbs 会话（战绩页需要 cookie_token）");
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let built = tauri::WebviewWindowBuilder::new(&app2, "official", tauri::WebviewUrl::External(home))
            .title("米游社：先登录（如提示扫码请扫），随后自动打开战绩页")
            .inner_size(480.0, 720.0)
            .resizable(true)
            .center()
            .on_navigation(|u| {
                crate::debug_log::log(&format!("official 导航: {u}"));
                true
            })
            .build();
        let w = match built {
            Ok(w) => w,
            Err(e) => {
                crate::debug_log::log(&format!("open_official_page: 创建失败 {e}"));
                return;
            }
        };
        let w2 = w.clone();
        w.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = w2.destroy();
            }
        });

        // 轮询站点会话：检出 cookie_token 后导航到官方战绩页
        let w_poll = w.clone();
        let app_poll = app2.clone();
        tauri::async_runtime::spawn(async move {
            const MAX_TICKS: u32 = 120; // 120 × 1.5s = 180s
            let mut tick = 0u32;
            let mut logged_once = false;
            loop {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                if app_poll.get_webview_window("official").is_none() {
                    crate::debug_log::log("open_official_page: 窗口已关闭，停止轮询");
                    break;
                }
                tick += 1;
                let cookies = match w_poll.cookies() {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let mut has_token = false;
                let mut names: Vec<String> = Vec::new();
                for c in &cookies {
                    let d = c.domain().unwrap_or("");
                    if d.contains("miyoushe") || d.contains("mihoyo") {
                        names.push(format!("{}@{}", c.name(), d));
                    }
                    if d.contains("miyoushe") && c.name() == "cookie_token" && !c.value().is_empty() {
                        has_token = true;
                    }
                }
                if !logged_once {
                    crate::debug_log::log(&format!(
                        "open_official_page: 站点会话检查 → 共 {} 个 cookie: {}",
                        cookies.len(),
                        names.join(", ")
                    ));
                    logged_once = true;
                }
                let timeout = tick >= MAX_TICKS;
                if has_token || timeout {
                    if has_token {
                        crate::debug_log::log("open_official_page: ② 已检出 cookie_token → 导航到官方战绩页");
                    } else {
                        crate::debug_log::log(
                            "open_official_page: ② 等待超时仍未取得 cookie_token → 仍导航到官方战绩页（页面可能仍显示扫码提示）",
                        );
                    }
                    if let Ok(u) = RECORD_PAGE_URL.parse::<tauri::Url>() {
                        let _ = w_poll.navigate(u);
                    }
                    break;
                }
            }
        });
    });
    Ok(())
}
