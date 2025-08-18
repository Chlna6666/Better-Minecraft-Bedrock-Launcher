use std::{path::Path, fs};
use serde::{Deserialize, Serialize};
use tauri::{Manager, Runtime};
use tracing::info;

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    pub link: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginDependency {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginManifest {
    pub manifest_version: u32,
    pub entry: String,            // 插件入口文件
    pub r#type: Option<String>,   // 插件类型
    pub name: String,             // 插件名字
    pub description: Option<String>, // 插件介绍
    pub icon: Option<String>,     // 插件图标路径
    pub version: String,          // 版本号
    pub authors: Vec<PluginAuthor>, // 作者信息
    pub dependencies: Option<Vec<PluginDependency>>, // 插件依赖
}

// 扫描指定目录下的插件

pub fn scan_plugins<R: Runtime>(app_handle: &tauri::AppHandle<R>) -> Vec<PluginManifest> {
    let plugins_dir = Path::new("./BMCBL/plugins");

    let mut manifests = Vec::new();

    if let Ok(entries) = std::fs::read_dir(plugins_dir) {
        for entry in entries.flatten() {
            if let Ok(manifest) = load_manifest(&entry.path()) {
                // 构造作者信息字符串
                let authors = manifest.authors.iter()
                    .map(|a| {
                        if let Some(link) = &a.link {
                            format!("{} ({})", a.name, link)
                        } else {
                            a.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");

                info!("已加载插件: {} (版本: {}) | 作者: {}", manifest.name, manifest.version, authors);
                manifests.push(manifest);
            }
        }
    } else {
        info!("插件目录不存在或无法读取: {:?}", plugins_dir);
    }

    if manifests.is_empty() {
        println!("未加载到任何插件。");
    }

    manifests
}

// 加载插件清单文件
fn load_manifest(plugin_dir: &Path) -> Result<PluginManifest, Box<dyn std::error::Error>> {
    let manifest_path = plugin_dir.join("manifest.json");
    let manifest_str = fs::read_to_string(manifest_path)?;
    Ok(serde_json::from_str(&manifest_str)?)
}

// 加载插件脚本内容
pub fn read_plugin_script_file(plugin_name: &str, entry_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let script_path = Path::new("./BMCBL/plugins")
        .join(plugin_name)
        .join(entry_path);

    fs::read_to_string(script_path).map_err(|e| e.into())
}