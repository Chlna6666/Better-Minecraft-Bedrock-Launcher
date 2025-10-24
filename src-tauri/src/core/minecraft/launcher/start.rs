use windows::core::{HSTRING};
use windows::Win32::System::Com::{CoInitializeEx, CoCreateInstance, COINIT_APARTMENTTHREADED, CLSCTX_ALL};
use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, ACTIVATEOPTIONS};

use std::ptr;
use std::io;
use std::os::windows::process::CommandExt;
use std::process::Command;
use tracing::info;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

fn launch_uwp_winapi(app_user_model_id: &str) -> Result<u32, windows::core::Error> {
    let hr = unsafe { CoInitializeEx(Some(ptr::null()), COINIT_APARTMENTTHREADED) };
    if !hr.is_ok() {
        return Err(hr.into());
    }

    let activator: IApplicationActivationManager = unsafe {
        CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)?
    };

    let result = unsafe {
        activator.ActivateApplication(
            &HSTRING::from(app_user_model_id),
            &HSTRING::new(),
            ACTIVATEOPTIONS(0),
        )
    };

    match result {
        Ok(pid) => {
            info!("通过 IApplicationActivationManager 启动成功，PID: {}", pid);
            Ok(pid)
        }
        Err(e) => Err(e),
    }
}

pub fn launch_uwp(edition: &str) -> io::Result<Option<u32>> {
    let app_user_model_id = match edition {
        "Microsoft.MinecraftUWP" => "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App",
        "Microsoft.MinecraftWindowsBeta" => "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe!App",
        "Microsoft.MinecraftEducationEdition" => "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition",
        "Microsoft.MinecraftEducationPreview" => "Microsoft.MinecraftEducationPreview_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition",
        _ => return Ok(None),
    };

    match launch_uwp_winapi(app_user_model_id) {
        Ok(pid) => Ok(Some(pid)),
        Err(e) => {
            info!("WinAPI 启动失败，尝试 cmd: {}", e);
            let status = Command::new("cmd")
                .arg("/C")
                .arg("start")
                .arg(format!("shell:appsFolder\\{}", app_user_model_id))
                .creation_flags(CREATE_NO_WINDOW.0)
                .status()?;
            info!("cmd 启动状态: {:?}", status);
            Ok(None)
        }
    }
}
