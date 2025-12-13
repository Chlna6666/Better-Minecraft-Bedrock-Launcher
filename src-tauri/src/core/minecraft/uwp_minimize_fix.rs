// uwp_minimize_fix.rs
// 使用 Rust + windows crate 启用 UWP 包的调试（等同于 C++ 中的 IPackageDebugSettings::EnableDebugging）

use std::{ffi::OsStr, os::windows::prelude::OsStrExt};

use windows::core::{Result, GUID, HSTRING, PCWSTR};
use windows::Management::Deployment::PackageManager;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::IPackageDebugSettings;

pub fn enable_debugging_for_package(package_name: &str) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;

        let clsid_pkg_debug = GUID::from_u128(0xb1aec16f_2383_4852_b0e9_8f0b1dc66b4d);
        let pkg_debug: IPackageDebugSettings =
            CoCreateInstance(&clsid_pkg_debug, None, CLSCTX_INPROC_SERVER)?;

        // 先尝试启用传入的包名（可能是 family 或 full name）
        let wide: Vec<u16> = OsStr::new(package_name).encode_wide().chain([0]).collect();
        let _ = pkg_debug.EnableDebugging(PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null());

        // 用 PackageManager 获取所有 full names
        let pm = PackageManager::new()?;
        let family_hs: HSTRING = HSTRING::from(package_name);
        if let Ok(packages) = pm.FindPackagesByPackageFamilyName(&family_hs) {
            for pkg in packages {
                if let Ok(pkg_id) = pkg.Id() {
                    if let Ok(full_name_hstr) = pkg_id.FullName() {
                        let full_name_str: String = full_name_hstr.to_string();
                        let wide: Vec<u16> = full_name_str.encode_utf16().chain([0]).collect();
                        let _ = pkg_debug.EnableDebugging(
                            PCWSTR(wide.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                        );
                    }
                }
            }
        }

        CoUninitialize();
        Ok(())
    }
}
