use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::{Path, PathBuf}};
use tauri::command;
use tokio::{fs, io::AsyncWriteExt};
use tracing::{debug, error};

#[derive(Serialize, Deserialize, Clone, Debug)]
struct DllConfig {
    enabled: bool,
    delay: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct InjectConfig {
    files: HashMap<String, DllConfig>,
}

#[derive(Serialize, Debug)]
struct ModItem {
    id: String,
    name: String,
    enabled: bool,
    delay: u64,
    path: String,
}

// ---------------- Helper functions ----------------

fn versions_base() -> PathBuf {
    PathBuf::from("./BMCBL/versions")
}

fn mods_dir_for(folder_name: &str) -> PathBuf {
    versions_base().join(folder_name).join("mods")
}

async fn read_inject_config(mods_dir: &Path) -> Result<InjectConfig, String> {
    let cfg_path = mods_dir.join("inject_config.json");
    if fs::metadata(&cfg_path).await.is_err() {
        return Ok(InjectConfig::default());
    }
    match fs::read_to_string(&cfg_path).await {
        Ok(raw) => match serde_json::from_str::<InjectConfig>(&raw) {
            Ok(cfg) => Ok(cfg),
            Err(e) => {
                // backup corrupted
                let bak = mods_dir.join("inject_config.json.bak");
                let _ = fs::rename(&cfg_path, &bak).await;
                error!("inject_config.json parse failed, backed up to {:?}: {:?}", bak, e);
                Ok(InjectConfig::default())
            }
        },
        Err(e) => {
            error!("read inject_config.json failed: {:?}", e);
            Ok(InjectConfig::default())
        }
    }
}

async fn write_inject_config(mods_dir: &Path, cfg: &InjectConfig) -> Result<(), String> {
    let cfg_path = mods_dir.join("inject_config.json");
    let tmp_path = mods_dir.join("inject_config.json.tmp");
    let content = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config failed: {}", e))?;
    // write tmp
    let mut f = fs::File::create(&tmp_path).await.map_err(|e| format!("create tmp failed: {}", e))?;
    f.write_all(content.as_bytes()).await.map_err(|e| format!("write tmp failed: {}", e))?;
    // rename
    fs::rename(&tmp_path, &cfg_path).await.map_err(|e| format!("rename tmp failed: {}", e))?;
    Ok(())
}


#[command]
pub async fn get_mod_list(folder_name: String) -> Result<serde_json::Value, String> {
    let base = PathBuf::from("./BMCBL/versions");
    let version_dir = base.join(&folder_name);
    let mods_dir = version_dir.join("mods");

    // 如果版本目录不存在，返回空数组
    if !version_dir.exists() {
        debug!("版本目录不存在: {}", version_dir.display());
        return Ok(serde_json::json!([]));
    }

    // 确保 mods 目录存在（如果不存在就创建）
    if !mods_dir.exists() {
        if let Err(e) = fs::create_dir_all(&mods_dir).await {
            return Err(format!("无法创建 mods 目录 {}: {}", mods_dir.display(), e));
        }
        debug!("已创建 mods 目录: {}", mods_dir.display());
    }

    // 收集 mods 目录下的所有 dll 文件名（不包含路径）
    let mut dll_names: Vec<String> = Vec::new();
    match fs::read_dir(&mods_dir).await {
        Ok(mut rd) => {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let p = entry.path();
                if p.extension()
                    .and_then(|ext| ext.to_str())
                    .map_or(false, |ext| ext.eq_ignore_ascii_case("dll"))
                {
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()) {
                        dll_names.push(name);
                    }
                }
            }
        }
        Err(e) => {
            return Err(format!("读取 mods 目录失败 {}: {}", mods_dir.display(), e));
        }
    }

    // inject_config.json 路径
    let cfg_path = mods_dir.join("inject_config.json");

    // 读取已有配置（或使用默认），并补全缺失项
    let mut config = InjectConfig::default();
    if fs::metadata(&cfg_path).await.is_ok() {
        match fs::read_to_string(&cfg_path).await {
            Ok(raw) => {
                match serde_json::from_str::<InjectConfig>(&raw) {
                    Ok(mut c) => {
                        // 补全丢失的 dll 条目（默认 enabled=true, delay=0）
                        let mut changed = false;
                        for dll in dll_names.iter() {
                            if !c.files.contains_key(dll) {
                                c.files.insert(dll.clone(), DllConfig { enabled: true, delay: 0 });
                                changed = true;
                            }
                        }
                        // 如果配置中存在已删除的文件，也不强制删除配置项（保持历史），但后面只返回存在的文件
                        if changed {
                            let _ = write_inject_config(&mods_dir, &c).await;
                        }
                        config = c;
                    }
                    Err(e) => {
                        // 解析失败，备份并继续使用默认（后面会用 dll_names 构造默认）
                        let bak = mods_dir.join("inject_config.json.bak");
                        let _ = fs::rename(&cfg_path, &bak).await;
                        debug!("inject_config.json 解析失败，已备份到 {}: {:?}", bak.display(), e);
                    }
                }
            }
            Err(e) => {
                debug!("读取 inject_config.json 失败 {}: {:?}", cfg_path.display(), e);
            }
        }
    }

    // 如果配置为空（或没有任何条目），则使用 dll_names 构造默认配置并写入文件
    if config.files.is_empty() {
        for dll in dll_names.iter() {
            config.files.insert(dll.clone(), DllConfig { enabled: true, delay: 0 });
        }
        let _ = write_inject_config(&mods_dir, &config).await;
    }

    // 构建结果：对 mods 目录中实际存在的 dll 返回条目（包含 enabled + delay）
    let mut result: Vec<ModItem> = Vec::new();
    for dll in dll_names.into_iter() {
        // 获取配置中对应的状态（如果配置没有则使用默认）
        let cfg = config.files.get(&dll).cloned().unwrap_or(DllConfig { enabled: true, delay: 0 });

        let candidate = mods_dir.join(&dll);
        match fs::canonicalize(&candidate).await {
            Ok(abs) => {
                if abs.exists() {
                    let path_str = abs.to_string_lossy().to_string();
                    result.push(ModItem {
                        id: dll.clone(),
                        name: dll.clone(),
                        enabled: cfg.enabled,
                        delay: cfg.delay,
                        path: path_str,
                    });
                } else {
                    debug!("文件不存在，跳过: {}", candidate.display());
                }
            }
            Err(e) => {
                debug!("canonicalize 失败，跳过 {}: {:?}", candidate.display(), e);
            }
        }
    }

    // 返回 JSON 数组
    match serde_json::to_value(&result) {
        Ok(v) => Ok(v),
        Err(e) => Err(format!("序列化失败: {}", e)),
    }
}


/// unified set_mod: 设置 mod 的 enabled + delay 并持久化到 inject_config.json
#[command]
pub async fn set_mod(folder_name: String, mod_id: String, enabled: bool, delay: u64) -> Result<String, String> {
    let mods_dir = mods_dir_for(&folder_name);
    if fs::metadata(&mods_dir).await.is_err() {
        return Err(format!("mods 目录不存在: {}", mods_dir.display()));
    }

    let mut cfg = read_inject_config(&mods_dir).await.unwrap_or_default();

    // ensure entry exists
    cfg.files.entry(mod_id.clone()).or_insert(DllConfig { enabled, delay });

    // set fields
    if let Some(entry) = cfg.files.get_mut(&mod_id) {
        entry.enabled = enabled;
        entry.delay = delay;
    }

    write_inject_config(&mods_dir, &cfg).await.map_err(|e| format!("写入配置失败: {}", e))?;

    Ok(format!("set_mod {} ok", mod_id))
}

/// import_mods: copy files into mods dir, add to config
#[command]
pub async fn import_mods(folder_name: String, paths: Vec<String>) -> Result<serde_json::Value, String> {
    let mods_dir = mods_dir_for(&folder_name);
    if let Err(e) = fs::create_dir_all(&mods_dir).await {
        return Err(format!("无法创建 mods 目录 {}: {}", mods_dir.display(), e));
    }

    let mut cfg = read_inject_config(&mods_dir).await.unwrap_or_default();

    let mut imported: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for p in paths.into_iter() {
        let src = PathBuf::from(&p);
        if !src.exists() {
            errors.push(format!("源文件不存在: {}", p));
            continue;
        }
        let filename = match src.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                errors.push(format!("无法识别文件名: {}", p));
                continue;
            }
        };
        let dest = mods_dir.join(&filename);

        // copy (overwrite) - change strategy here if you want rename instead
        match fs::copy(&src, &dest).await {
            Ok(_) => {
                imported.push(filename.clone());
                cfg.files.entry(filename.clone()).or_insert(DllConfig { enabled: true, delay: 0 });
            }
            Err(e) => {
                errors.push(format!("复制 {} 到 {} 失败: {}", src.display(), dest.display(), e));
            }
        }
    }

    if let Err(e) = write_inject_config(&mods_dir, &cfg).await {
        return Err(format!("写入配置失败: {}", e));
    }

    Ok(serde_json::json!({ "imported": imported, "errors": errors }))
}

/// delete_mods: delete files and remove from config
#[command]
pub async fn delete_mods(folder_name: String, mod_ids: Vec<String>) -> Result<String, String> {
    let mods_dir = mods_dir_for(&folder_name);
    if fs::metadata(&mods_dir).await.is_err() {
        return Err(format!("mods 目录不存在: {}", mods_dir.display()));
    }

    let mut cfg = read_inject_config(&mods_dir).await.unwrap_or_default();
    let mut errors: Vec<String> = Vec::new();

    for id in &mod_ids {
        let candidate = mods_dir.join(id);
        match fs::remove_file(&candidate).await {
            Ok(_) => debug!("deleted mod file: {:?}", candidate),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!("删除文件 {} 失败: {}", candidate.display(), e));
                } else {
                    debug!("file not found, skip: {:?}", candidate);
                }
            }
        }
        cfg.files.remove(id);
    }

    if let Err(e) = write_inject_config(&mods_dir, &cfg).await {
        errors.push(format!("写入配置失败: {}", e));
    }

    if !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Ok(format!("deleted {} mods", mod_ids.len()))
    }
}
