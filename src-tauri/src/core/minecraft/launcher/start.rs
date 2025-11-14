use windows::core::{HSTRING};
use windows::Win32::System::Com::{CoInitializeEx, CoCreateInstance, COINIT_APARTMENTTHREADED, CLSCTX_ALL};
use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, ACTIVATEOPTIONS};

use std::ptr;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
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

pub fn launch_win32(package_folder: &str) -> io::Result<Option<u32>> {
    let folder = Path::new(package_folder);
    if !folder.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("package folder not found: {}", package_folder),
        ));
    }

    // 先扫描当前目录下的 exe，优先匹配常见名字
    let mut candidate_exes: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(folder) {
        for entry in rd.flatten() {
            let p = entry.path();
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if ext.eq_ignore_ascii_case("exe") {
                    if let Some(fname) = p.file_name().and_then(|f| f.to_str()) {
                        let fname_l = fname.to_lowercase();
                        // 高优先级匹配
                        if fname_l.contains("minecraft") || fname_l.contains("bedrock") || fname_l.contains("launcher") {
                            // 立刻尝试启动优先 exe
                            match Command::new(&p).spawn() {
                                Ok(child) => return Ok(Some(child.id())),
                                Err(err) => {
                                    // 记录为候选，继续尝试其他 exe
                                    candidate_exes.push(p.clone());
                                    eprintln!("尝试启动 {} 失败: {:?}", p.display(), err);
                                    continue;
                                }
                            }
                        } else {
                            // 低优先级候选，稍后尝试
                            candidate_exes.push(p.clone());
                        }
                    }
                }
            }
        }
    }

    // 回退：尝试候选 exe（按发现顺序）
    for p in candidate_exes.into_iter() {
        match Command::new(&p).spawn() {
            Ok(child) => return Ok(Some(child.id())),
            Err(e) => {
                eprintln!("回退启动 {} 失败: {:?}", p.display(), e);
                continue;
            }
        }
    }

    let try_names = ["Minecraft.Windows.exe"];
    for name in &try_names {
        let p = folder.join(name);
        if p.exists() {
            match Command::new(&p).spawn() {
                Ok(child) => return Ok(Some(child.id())),
                Err(e) => {
                    eprintln!("尝试启动 {} 失败: {:?}", p.display(), e);
                }
            }
        }
    }

    Ok(None)
}