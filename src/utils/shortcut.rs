use anyhow::{Context, Result};
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;

#[cfg(target_os = "windows")]
pub fn create_desktop_shortcut(version_folder: &str, display_name: &str) -> Result<PathBuf> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};

    let desktop_dir = get_desktop_dir()?;
    let safe_name = sanitize_filename(display_name);
    let shortcut_path = desktop_dir.join(format!("{safe_name}.lnk"));

    let exe_path = std::env::current_exe().context("无法获取应用程序二进制路径")?;
    let exe_dir = exe_path.parent().context("无法获取可执行文件所在目录")?;

    let arguments = format!("--launch-version \"{version_folder}\"");
    let description = format!("启动 {display_name}");

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .context("创建 COM IShellLinkW 失败")?;

        let exe_path_wide: Vec<u16> = exe_path.as_os_str().encode_wide().chain(Some(0)).collect();
        let args_wide: Vec<u16> = std::ffi::OsStr::new(&arguments)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let dir_wide: Vec<u16> = exe_dir.as_os_str().encode_wide().chain(Some(0)).collect();
        let desc_wide: Vec<u16> = std::ffi::OsStr::new(&description)
            .encode_wide()
            .chain(Some(0))
            .collect();

        shell_link.SetPath(PCWSTR(exe_path_wide.as_ptr()))?;
        shell_link.SetArguments(PCWSTR(args_wide.as_ptr()))?;
        shell_link.SetWorkingDirectory(PCWSTR(dir_wide.as_ptr()))?;
        shell_link.SetDescription(PCWSTR(desc_wide.as_ptr()))?;
        shell_link.SetIconLocation(PCWSTR(exe_path_wide.as_ptr()), 0)?;

        let persist_file: IPersistFile = shell_link.cast()?;
        let shortcut_path_wide: Vec<u16> = shortcut_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();

        persist_file.Save(PCWSTR(shortcut_path_wide.as_ptr()), true)?;
    }

    Ok(shortcut_path)
}

#[cfg(not(target_os = "windows"))]
pub fn create_desktop_shortcut(_version_folder: &str, _display_name: &str) -> Result<PathBuf> {
    anyhow::bail!("桌面快捷方式仅在 Windows 系统受支持");
}

fn sanitize_filename(name: &str) -> String {
    let invalid_chars = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    let mut clean: String = name
        .chars()
        .map(|c| if invalid_chars.contains(&c) { '_' } else { c })
        .collect();
    if clean.trim().is_empty() {
        clean = "Minecraft_Version".to_string();
    }
    clean
}

#[cfg(target_os = "windows")]
fn get_desktop_dir() -> Result<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    unsafe {
        if let Ok(path_ptr) = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) {
            let path_str = path_ptr.to_string()?;
            CoTaskMemFree(Some(path_ptr.as_ptr() as *const _));
            return Ok(PathBuf::from(path_str));
        }
    }

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let desktop = PathBuf::from(user_profile).join("Desktop");
        if desktop.exists() {
            return Ok(desktop);
        }
    }

    anyhow::bail!("无法获取 Windows 桌面目录")
}
