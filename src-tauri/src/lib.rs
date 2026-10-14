use tauri::{Emitter, Manager};

mod api;
mod commands;
pub mod crypto;
mod debug_log;
mod desktop_pin;
mod gateway;
mod storage;
mod task;
mod tray;

/// 处理扫码登录窗口回传的状态（兜底路径）
/// 注：document.cookie 读不到 HttpOnly 的 ltoken/stoken，此路径通常不触发；
/// 主路径为 commands::start_qr_login 中的 WebView2 CookieManager 轮询。
fn handle_qr_state(app: &tauri::AppHandle, _url: &str, cookie: &str) {
    let map = commands::parse_cookie_str(cookie);
    if map.is_empty() {
        return;
    }
    if map.contains_key("ltoken") || map.contains_key("ltoken_v2") {
        // Listener 回调是同步上下文，apply_login_cookie 为 async（内含换取 stoken 网络请求）
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            commands::apply_login_cookie(&app, map).await;
        });
    }
}

/// 在**任何 WebView 创建之前**处理"设备重置"标记（见 `commands::reset_device_identity`）
///
/// ⚠️ 必须在 `tauri::Builder` 之前调用：Tauri 启动时会按 tauri.conf.json 创建 main 窗口，
///    一旦 WebView2 起来，`EBWebView` 目录即被占用、删不掉。
/// 成功才清除标记（失败则保留，下次启动重试）。
fn handle_pending_device_reset() {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return;
    };
    let flag = std::path::Path::new(&appdata)
        .join("com.mihoyo.widget")
        .join(".reset-device");
    if !flag.exists() {
        return;
    }
    let webview_dir = std::env::var("LOCALAPPDATA")
        .map(|d| {
            std::path::Path::new(&d)
                .join("com.mihoyo.widget")
                .join("EBWebView")
        })
        .unwrap_or_default();
    // 目录不存在也算成功（本就干净）
    let ok = if webview_dir.exists() {
        std::fs::remove_dir_all(&webview_dir).is_ok()
    } else {
        true
    };
    if ok {
        let _ = std::fs::remove_file(&flag);
    }
    debug_log::log(&format!(
        "handle_pending_device_reset: 删除 WebView2 配置目录 {:?} → {}",
        webview_dir,
        if ok { "成功" } else { "失败（保留标记，下次重试）" }
    ));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ⚠️ 必须最先执行（早于 tauri::Builder）
    handle_pending_device_reset();

    tauri::Builder::default()
        .manage(commands::ApiState::new())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 二次启动：唤起已有实例的主窗口
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        // ⚠️ 窗口状态自建持久化（替代 tauri-plugin-window-state，见 setup 中 restore/save）：
        //    插件位置恢复受"保存位置须与当前显示器相交"限制，跨屏/分辨率变化时静默丢弃
        //    → 重启后窗口重置（大小默认、位置右下角）。自建实现 clamp 到屏幕内，100% 可控。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // 注：tauri-plugin-store 已移除（前端从未调用 store 插件，而是全程走
        //     get_config/update_config 自定义命令 + Rust 原子写 config.json）；
        //     同时移除 capabilities/default.json 的 store:default 权限。
        .setup(|app| {
            crate::debug_log::log("setup 开始");

            // 扫码登录：监听登录窗口回传的 cookie 状态
            let handle = app.handle().clone();
            let handler_handle = handle.clone();
            let _unlisten = tauri::Listener::listen(&handle, "qr-login-state", move |event| {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let cookie = payload.get("cookie").and_then(|v| v.as_str()).unwrap_or("");
                    handle_qr_state(&handler_handle, url, cookie);
                }
            });

            // 拖拽窗口（Rust 侧执行，规避前端 JS 限制）
            let drag_handle = handle.clone();
            let _ = tauri::Listener::listen(&handle, "start-drag", move |_| {
                if let Some(w) = drag_handle.get_webview_window("main") {
                    let _ = w.start_dragging();
                }
            });

            // 隐藏到托盘（× 按钮）
            let hide_handle = handle.clone();
            let _ = tauri::Listener::listen(&handle, "hide-window", move |_| {
                if let Some(w) = hide_handle.get_webview_window("main") {
                    let _ = w.hide();
                }
            });

            // —— geetest 验证码链路：captcha 窗口 → Rust 的四个事件 ——
            //   ⚠️ 事件命名纪律（防自循环）：Rust 发给主窗口的一律用**不同名**的
            //      `captcha-success` / `captcha-failed`，否则会监听到自己 emit 的事件形成回环。

            // ① 滑块页面加载完成（诊断）
            let _ = tauri::Listener::listen(&handle, "captcha-loaded", |event| {
                crate::debug_log::log(&format!("captcha-loaded: {}", event.payload()));
            });
            // ② 注入脚本诊断（仅记日志，排查滑块渲染/网络问题时看这里）
            let _ = tauri::Listener::listen(&handle, "captcha-diagnose", |event| {
                crate::debug_log::log(&format!("captcha-diag: {}", event.payload()));
            });
            // ③ 验证失败（滑块窗口内浏览器通道 + Rust 通道均失败）→ 解冻 trace + 转发主窗口
            let err_handle = handle.clone();
            let _ = tauri::Listener::listen(&handle, "captcha-error", move |event| {
                let v: serde_json::Value =
                    serde_json::from_str(event.payload()).unwrap_or(serde_json::Value::Null);
                let game_id = v.get("game_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let message = v
                    .get("message")
                    .and_then(|x| x.as_str())
                    .unwrap_or("验证失败")
                    .to_string();
                if !game_id.is_empty() {
                    crate::commands::unfreeze_trace(&err_handle, &game_id);
                }
                if let Some(w) = err_handle.get_webview_window("captcha") {
                    let _ = w.hide();
                }
                crate::debug_log::log(&format!("captcha-error: {game_id} {message}"));
                let _ = err_handle.emit(
                    "captcha-failed",
                    serde_json::json!({ "game_id": game_id, "message": message }),
                );
            });
            // ④ 验证成功 → token 落盘（单次使用）+ 解冻 trace + 关窗 + 通知主窗口
            let ok_handle = handle.clone();
            let _ = tauri::Listener::listen(&handle, "captcha-result", move |event| {
                let v: serde_json::Value =
                    serde_json::from_str(event.payload()).unwrap_or(serde_json::Value::Null);
                let game_id = v.get("game_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let token = v.get("token").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if game_id.is_empty() || token.is_empty() {
                    crate::debug_log::log("captcha-result: payload 缺 game_id/token，已忽略");
                    return;
                }
                match crate::storage::challenge::set(&ok_handle, &game_id, &token) {
                    Ok(_) => crate::debug_log::log(&format!("captcha-result: {game_id} token 已落盘")),
                    Err(e) => crate::debug_log::log(&format!("captcha-result: token 落盘失败 {e}")),
                }
                crate::commands::unfreeze_trace(&ok_handle, &game_id);
                if let Some(w) = ok_handle.get_webview_window("captcha") {
                    let _ = w.hide();
                }
                // ⚠️ 刻意**不**在此自动请求便签 —— 铁律"仅手动刷新"；只告知前端"验证已完成"
                let _ = ok_handle.emit("captcha-success", serde_json::json!({ "game_id": game_id }));
            });

            tray::setup_tray(app.handle())?;
            task::spawn_scheduler(app.handle().clone());
            crate::debug_log::log("setup: 托盘与调度器已装配");

            // ⚠️ 战绩域（便签）**不做任何启动预取**——用户本次明确要求"仅手动刷新"。
            //    历史教训：启动/定时/切页签自动请求便签 → 持续续期风控滑动窗口 → 永不消退。

            // 关闭按钮行为：隐藏到托盘而非退出
            if let Some(w) = app.get_webview_window("main") {
                let w2 = w.clone();
                w.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w2.hide();
                    }
                });

                // —— 窗口大小/位置持久化（自建，替代失效的 window-state 插件） ——
                // 恢复：从 config.widget_position 读取上次大小/位置，clamp 到当前主屏可见区域
                //   （防止保存值超出当前屏幕导致窗口丢失/回到默认右下角）
                let cfg = crate::storage::config::load(app.handle());
                if let Some(pos) = &cfg.widget_position {
                    let pos_x = pos.x;
                    let pos_y = pos.y;
                    let pos_w = pos.w;
                    let pos_h = pos.h;
                    // clamp 到主显示器物理范围（留 60px 边距，确保标题/拖动区域可见）
                    if let Ok(Some(m)) = w.primary_monitor() {
                        let msize = m.size();
                        let mx = (msize.width as i32 - 60).max(0);
                        let my = (msize.height as i32 - 60).max(0);
                        let _ = w.set_position(tauri::PhysicalPosition::new(
                            pos_x.clamp(0, mx),
                            pos_y.clamp(0, my),
                        ));
                    } else {
                        let _ = w.set_position(tauri::PhysicalPosition::new(pos_x, pos_y));
                    }
                    let _ = w.set_size(tauri::PhysicalSize::new(
                        (pos_w as u32).max(260),
                        (pos_h as u32).max(300),
                    ));
                }
                // 保存：监听移动/缩放，500ms debounce 写回 config（拖动期间不频繁写盘）
                let w_evt = w.clone();
                let app_evt = app.handle().clone();
                let last_save = std::sync::Arc::new(std::sync::Mutex::new(
                    std::time::Instant::now()
                        - std::time::Duration::from_secs(10),
                ));
                w.on_window_event(move |event| {
                    if let tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) = event {
                        let mut last = last_save.lock().unwrap();
                        if last.elapsed() < std::time::Duration::from_millis(500) {
                            return;
                        }
                        *last = std::time::Instant::now();
                        let w2 = w_evt.clone();
                        let a2 = app_evt.clone();
                        tauri::async_runtime::spawn(async move {
                            if let (Ok(pos), Ok(size)) = (w2.outer_position(), w2.outer_size())
                            {
                                let p = crate::storage::config::WidgetPosition {
                                    x: pos.x,
                                    y: pos.y,
                                    w: size.width as i32,
                                    h: size.height as i32,
                                };
                                let mut cfg = crate::storage::config::load(&a2);
                                cfg.widget_position = Some(p);
                                let _ = crate::storage::config::save(&a2, &cfg);
                            }
                        });
                    }
                });

                // 显示主窗口（登录态由前端判断，未登录时前端弹出登录窗）
                let _ = w.show();
                // 钉桌面（模拟小组件置于桌面）
                let _ = desktop_pin::attach_to_desktop(&w);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::login_with_cookies,
            commands::get_login_status,
            commands::get_games,
            commands::get_sign_info,
            commands::manual_sign_in,
            commands::get_daily_note,
            commands::get_verification_challenge,
            commands::open_captcha_window,
            commands::verify_captcha_fallback,
            commands::reset_device_identity,
            commands::open_official_page,
            commands::get_config,
            commands::update_config,
            commands::start_qr_login,
            commands::logout,
            commands::set_auto_start,
            commands::get_auto_start,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
