from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PATH = ROOT / "src/core/minecraft/launcher/task.rs"
text = PATH.read_text(encoding="utf-8")


def replace_once(old: str, new: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected exactly one anchor, got {count}: {old[:160]!r}")
    text = text.replace(old, new, 1)


replace_once(
    '''    compare_versions(version, "1.21.12000.21") != Ordering::Less
}

pub fn embedded_dll_version_string()''',
    '''    compare_versions(version, "1.21.12000.21") != Ordering::Less
}

fn requires_legacy_uwp_bloader_isolation(version: &str) -> bool {
    let parsed = parse_version_to_vec_simple(version);
    parsed.get(0) == Some(&1) && parsed.get(1) == Some(&18) && parsed.get(2) == Some(&30)
}

pub fn embedded_dll_version_string()''',
)

replace_once(
    '''    let is_win32 = is_win32_version(&identity_version);
    info!(''',
    '''    let is_win32 = is_win32_version(&identity_version);
    let legacy_uwp_bloader_isolation =
        !is_win32 && requires_legacy_uwp_bloader_isolation(&identity_version);
    info!(''',
)

replace_once(
    '''        if let Err(error) = ensure_backup(&exe_path) {
            warn!("无法创建 EXE 备份，将继续使用自标记还原机制: {error}");
        }

        if is_file_patched(&exe_path) {
            append_log(task_id, "检测到 PE 已包含补丁标记，跳过修补".to_string());
        } else {
            let _ = restore_original_pe(&exe_path);
            remove_readonly(&exe_path);
            inject_dll_import(&exe_path, injector_name, None)
                .map_err(|error| format!("PE 修改失败: {error}"))?;
            append_log(task_id, "静态注入环境已部署".to_string());
        }
''',
    '''        if legacy_uwp_bloader_isolation {
            if is_file_patched(&exe_path) {
                remove_readonly(&exe_path);
                restore_original_pe(&exe_path)
                    .map_err(|error| format!("1.18.30 隔离模式还原原始 PE 失败: {error}"))?;
            }
            if is_file_patched(&exe_path) {
                return Err(
                    "1.18.30 隔离模式无法移除 BLoader 静态 PE 导入；已中止启动以避免无效 A/B 测试"
                        .to_string(),
                );
            }
            append_log(
                task_id,
                "1.18.30 兼容性隔离：已恢复原始 PE，本次不会静态加载 BLoader.dll"
                    .to_string(),
            );
            info!(
                task_id = %task_id,
                version = %identity_version,
                exe_path = %exe_path.display(),
                "legacy UWP isolation active: BLoader static import disabled"
            );
        } else {
            if let Err(error) = ensure_backup(&exe_path) {
                warn!("无法创建 EXE 备份，将继续使用自标记还原机制: {error}");
            }

            if is_file_patched(&exe_path) {
                append_log(task_id, "检测到 PE 已包含补丁标记，跳过修补".to_string());
            } else {
                let _ = restore_original_pe(&exe_path);
                remove_readonly(&exe_path);
                inject_dll_import(&exe_path, injector_name, None)
                    .map_err(|error| format!("PE 修改失败: {error}"))?;
                append_log(task_id, "静态注入环境已部署".to_string());
            }
        }
''',
)

replace_once(
    '''        let pid = match activated_pid {
            Some(pid) if pid > 0 => pid,
            _ => wait_for_uwp_pid(target_exe, &pfn)
                .await
                .ok_or("启动超时".to_string())?,
        };
        if !version_config.disable_mod_loading {
            let log_task_id = task_id.to_string();
            handle_delayed_injection(
                pid,
                delayed_mods,
                Arc::new(move |message: String| {
                    append_log(&log_task_id, message);
                }),
                false,
            );
        }
        info!(task_id = %task_id, pid, "UWP 版本启动成功");
''',
    '''        let pid = match activated_pid {
            Some(pid) if pid > 0 => pid,
            _ => wait_for_uwp_pid(target_exe, &pfn)
                .await
                .ok_or("启动超时".to_string())?,
        };
        if legacy_uwp_bloader_isolation {
            append_log(
                task_id,
                "1.18.30 兼容性隔离：本次同时禁用全部 Mod 注入，确保测试进程完全不加载 BLoader"
                    .to_string(),
            );
        } else if !version_config.disable_mod_loading {
            let log_task_id = task_id.to_string();
            handle_delayed_injection(
                pid,
                delayed_mods,
                Arc::new(move |message: String| {
                    append_log(&log_task_id, message);
                }),
                false,
            );
        }
        info!(task_id = %task_id, pid, "UWP 版本启动成功");
''',
)

replace_once(
    '''pub fn embedded_dll_version_string() -> Option<String> {
    Some(bloader::embedded_version_string().to_string())
}
''',
    '''pub fn embedded_dll_version_string() -> Option<String> {
    Some(bloader::embedded_version_string().to_string())
}

#[cfg(test)]
mod legacy_uwp_bloader_isolation_tests {
    use super::requires_legacy_uwp_bloader_isolation;

    #[test]
    fn isolates_only_minecraft_1_18_30_family() {
        assert!(requires_legacy_uwp_bloader_isolation("1.18.30.4"));
        assert!(requires_legacy_uwp_bloader_isolation("1.18.30.0"));
        assert!(!requires_legacy_uwp_bloader_isolation("1.18.31.0"));
        assert!(!requires_legacy_uwp_bloader_isolation("1.19.0.0"));
        assert!(!requires_legacy_uwp_bloader_isolation("1.21.12000.21"));
    }
}
''',
)

PATH.write_text(text, encoding="utf-8", newline="\n")
print("patched", PATH)
