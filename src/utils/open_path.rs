#[cfg(target_os = "windows")]
use windows::{
    Win32::Foundation::HWND,
    Win32::UI::Shell::ShellExecuteW,
    Win32::UI::WindowsAndMessaging::SW_SHOW,
    core::{HSTRING, PCWSTR},
};

pub async fn open_path(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            // SAFETY: ShellExecuteW is an OS API. We pass valid null-terminated wide strings
            // backed by HSTRING, and we do not dereference the returned handle.
            let path_h = HSTRING::from(&path);
            let operation = HSTRING::from("open");
            let result = ShellExecuteW(
                Option::from(HWND::default()),
                PCWSTR(operation.as_ptr()),
                PCWSTR(path_h.as_ptr()),
                PCWSTR(std::ptr::null()),
                PCWSTR(std::ptr::null()),
                SW_SHOW,
            );
            if (result.0 as isize) > 32 {
                Ok(())
            } else {
                Err(format!("ShellExecuteW failed: {:?}", result))
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let cmd = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(cmd)
            .arg(&path)
            .spawn()
            .map_err(|e| format!("无法打开路径: {}", e))?;
        Ok(())
    }
}
