//! 简单文件日志（调试用，release 可移除）
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

fn log_path() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("mihoyo-widget-debug.log")
}

pub fn log(msg: &str) {
    let line = format!("{} [widget] {}\n", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(line.as_bytes());
    }
    // 同时输出到 stderr（debug console 可见）
    eprintln!("{}", line.trim_end());
}
