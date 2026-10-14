//! 钉桌面：窗口置于所有窗口底部（模拟桌面小组件，HWND_BOTTOM + 无激活/无Alt+Tab）
use tauri::Runtime;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, HWND_BOTTOM, SET_WINDOW_POS_FLAGS,
    WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

// SWP_NOSIZE(1) | SWP_NOMOVE(2) | SWP_NOACTIVATE(0x10)
const SWP_FLAGS: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(0x13);

pub fn attach_to_desktop<R: Runtime>(window: &tauri::WebviewWindow<R>) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;

    unsafe {
        // 扩展样式：禁止激活 + 隐藏于 Alt+Tab
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE); // i32
        let new_ex = (WINDOW_EX_STYLE(ex as u32) | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0 as i32;
        SetWindowLongW(hwnd, GWL_EXSTYLE, new_ex);

        // 置底（桌面小组件）
        SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_FLAGS).map_err(|e| e.to_string())?;
    }
    Ok(())
}
