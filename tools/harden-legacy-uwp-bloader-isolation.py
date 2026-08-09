from pathlib import Path

root = Path(__file__).resolve().parents[1]
path = root / "src/core/minecraft/launcher/task.rs"
text = path.read_text(encoding="utf-8")

old = '''    if request.auto_start
        && !version_config.disable_mod_loading
        && let Ok(mods) = load_mods_config(&mods_dir).await
'''
new = '''    if request.auto_start
        && !legacy_uwp_bloader_isolation
        && !version_config.disable_mod_loading
        && let Ok(mods) = load_mods_config(&mods_dir).await
'''
if text.count(old) != 1:
    raise SystemExit(f"mod-plan anchor count={text.count(old)}")
text = text.replace(old, new, 1)

start_marker = '''        let exe_dir = exe_path.parent().ok_or("无效的游戏目录".to_string())?;\n'''
end_marker = '''    advance_step(task_id, "patching", "启动环境准备完成".to_string());\n'''
start = text.find(start_marker)
end = text.find(end_marker, start)
if start < 0 or end < 0:
    raise SystemExit(f"prep block anchors not found: start={start}, end={end}")

replacement = '''        let exe_dir = exe_path.parent().ok_or("无效的游戏目录".to_string())?;
        let injector_name = "BLoader.dll";
        let injector_target_path = exe_dir.join(injector_name);

        if legacy_uwp_bloader_isolation {
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

            if injector_target_path.exists() {
                remove_readonly(&injector_target_path);
                fs::remove_file(&injector_target_path).map_err(|error| {
                    format!(
                        "1.18.30 隔离模式无法删除 {}: {error}",
                        injector_target_path.display()
                    )
                })?;
            }
            if injector_target_path.exists() {
                return Err(format!(
                    "1.18.30 隔离模式仍检测到 {}；已中止启动",
                    injector_target_path.display()
                ));
            }

            append_log(
                task_id,
                "1.18.30 兼容性隔离：原始 PE 已恢复，BLoader.dll 已移除，本次不会加载 BLoader 或任何 Mod"
                    .to_string(),
            );
            info!(
                task_id = %task_id,
                version = %identity_version,
                exe_path = %exe_path.display(),
                "legacy UWP isolation active: original PE restored and BLoader binary removed"
            );
        } else {
            let local_data_root = exe_dir.join(BLOADER_DEFAULT_REDIRECTION_ROOT);
            if !local_data_root.exists() {
                fs::create_dir_all(&local_data_root)
                    .map_err(|error| format!("创建重定向目录失败: {error}"))?;
            }
            let _ = grant_all_application_packages_access(&local_data_root);

            let mut need_update = true;
            if injector_target_path.exists() {
                remove_readonly(&injector_target_path);
                if let Ok(disk_bytes) = fs::read(&injector_target_path) {
                    need_update = bloader::version_string(&disk_bytes).as_deref()
                        != Some(bloader::embedded_version_string());
                }
            }

            if need_update {
                let injector_bytes = bloader::bytes()?;
                ensure_file_in_dir(exe_dir, injector_name, injector_bytes)?;
            }

            let file_redirections =
                version_config.effective_file_redirections(Path::new(package_folder));
            if !file_redirections.is_empty() {
                append_log(
                    task_id,
                    format!("已配置 {} 条文件重定向", file_redirections.len()),
                );
            }

            let _ = write_bloader_config(
                exe_dir,
                version_config.disable_mod_loading,
                version_config.enable_redirection,
                json!(file_redirections),
                json!(startup_mods_relative_paths),
            )?;
            remove_legacy_preloader_config(exe_dir);

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
    }
'''

text = text[:start] + replacement + text[end:]
path.write_text(text, encoding="utf-8", newline="\n")
print("hardened", path)
