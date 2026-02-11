#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::UI::Shell::*,
    Win32::UI::WindowsAndMessaging::*,
};

#[tauri::command]
pub async fn open_path(path: String) -> std::result::Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            // 1. 转换路径和操作为 Windows 宽字符 (HSTRING)
            let path_h = HSTRING::from(&path);
            let operation = HSTRING::from("open");

            // 2. 调用 ShellExecuteW
            // 注意：HWND::default() 创建的是一个空句柄 (0)，符合后台打开的需求
            let result = ShellExecuteW(
                Option::from(HWND::default()),
                PCWSTR(operation.as_ptr()),
                PCWSTR(path_h.as_ptr()),
                PCWSTR(std::ptr::null()),
                PCWSTR(std::ptr::null()),
                SW_SHOW,
            );

            // 3. 检查结果
            // ShellExecuteW 返回值大于 32 表示成功
            if (result.0 as isize) > 32 {
                Ok(())
            } else {
                Err(format!("调用 ShellExecuteW 失败，错误代码: {:?}", result))
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        std::process::Command::new(cmd)
            .arg(&path)
            .spawn()
            .map_err(|e| format!("无法打开路径: {}", e))?;
        Ok(())
    }
}