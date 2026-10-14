#[cfg(test)]
mod legacy_tests {
    use mihoyo_widget_lib::crypto::dpapi;
    use std::path::PathBuf;

    /// 实测：读取老版 %APPDATA%\MiHoYoWidget\cookies.enc 并用 DPAPI 解密
    /// 验证与老版 C# CookieManager 的兼容性（无感迁移的关键）
    #[test]
    fn decrypt_legacy_cookies() {
        let path = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("MiHoYoWidget")
            .join("cookies.enc");

        if !path.exists() {
            eprintln!("[skip] 老版 cookies.enc 不存在: {:?}", path);
            return; // 环境无关时跳过（不视为失败）
        }

        let data = std::fs::read(&path).expect("read cookies.enc");
        let plain = dpapi::decrypt(&data).expect("DPAPI 解密老版 cookies.enc 失败");
        let text = String::from_utf8(plain).expect("解密结果应为 UTF-8 JSON");

        let json: serde_json::Value =
            serde_json::from_str(&text).expect("解密结果应为合法 JSON");
        eprintln!("[ok] 老版 cookies 解密成功，字段: {:?}", json.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()));

        // 关键字段应存在
        let has_ltoken = json.get("ltoken").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        let has_ltuid = json.get("ltuid").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        assert!(has_ltoken || has_ltuid, "应至少包含 ltoken 或 ltuid");

        // 输出 cookie 明文到临时文件（仅本地调试用）
        if let Ok(cookie_str) = std::env::var("DUMP_COOKIE") {
            if cookie_str == "1" {
                let map = json.as_object().unwrap();
                let joined: Vec<String> = map
                    .iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                    .collect();
                std::fs::write(
                    std::env::var("APPDATA").map(PathBuf::from).unwrap_or_default()
                        .join("widget-cookie-dump.txt"),
                    joined.join("; "),
                )
                .expect("write cookie dump");
            }
        }
    }
}
