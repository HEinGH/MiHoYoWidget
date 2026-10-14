fn main() {
    // 声明应用自定义命令，生成 allow-<command> 权限。
    // ⚠️ Tauri v2 ACL（2026-08-30 源码实锤）：一旦用 AppManifest::commands 声明了
    //    任意应用命令，app ACL manifest 即存在 → 【所有】应用命令（含 local 的 main 窗口
    //    调用）都必须有显式 allow 权限，否则被拒（webview/mod.rs:
    //    `if (plugin_command.is_some() || has_app_acl_manifest || !is_local) ... && acl.is_none() { reject }`）。
    //    因此必须把全部命令声明出来，并在 default.json（main 窗口）逐一授权。
    //
    // ⚠️ v1.2.1（2026-10-14）：风控消退，恢复 `get_daily_note`（每日便签，战绩域）。
    //    该命令**仅允许**由前端「刷新」按钮显式触发——任何自动路径都不得调用。
    //    同日接线 geetest：新增 get_verification_challenge / open_captcha_window /
    //    verify_captcha_fallback，并恢复 capabilities/captcha.json（滑块窗口专用授权）。
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&[
                "get_app_state",
                "login_with_cookies",
                "get_login_status",
                "get_games",
                "get_sign_info",
                "manual_sign_in",
                "get_daily_note",
                "get_verification_challenge",
                "open_captcha_window",
                "verify_captcha_fallback",
                "reset_device_identity",
                "open_official_page",
                "get_config",
                "update_config",
                "start_qr_login",
                "logout",
                "set_auto_start",
                "get_auto_start",
            ]),
        ),
    )
    .unwrap();
}
