use std::{collections::HashMap, fs, io::BufReader, path::{Path, PathBuf}, sync::Mutex, time::SystemTime};
use serde::{Deserialize, Serialize};
use tauri::{Manager, Runtime};
use tracing::info;
use once_cell::sync::Lazy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

// -------- 可配置项 --------
const PLUGINS_DIR: &str = "./BMCBL/plugins";

// 单例 scanner（避免每次调用都重建缓存）
static SCANNER: Lazy<Mutex<PluginScanner>> = Lazy::new(|| Mutex::new(PluginScanner::new(PLUGINS_DIR)));

struct PluginScanner {
    plugins_dir: PathBuf,
    // cache: plugin_dir_path -> (modified_time, manifest)
    cache: HashMap<PathBuf, (SystemTime, PluginManifest)>,
}

impl PluginScanner {
    fn new<P: AsRef<Path>>(dir: P) -> Self {
        Self { plugins_dir: dir.as_ref().to_path_buf(), cache: HashMap::new() }
    }

    /// 主扫描入口，返回已加载的 manifests（会自动利用缓存）
    fn scan_plugins<R: Runtime>(&mut self, _app_handle: &tauri::AppHandle<R>) -> Vec<PluginManifest> {
        let plugins_dir = &self.plugins_dir;
        let mut manifests = Vec::new();

        let read_dir = match fs::read_dir(plugins_dir) {
            Ok(rd) => rd,
            Err(e) => {
                info!("插件目录不存在或无法读取: {:?} ({})", plugins_dir, e);
                return manifests;
            }
        };

        // 收集所有子目录（插件目录），避免在迭代时同时修改 cache
        let mut plugin_dirs = Vec::new();
        for entry in read_dir.flatten() {
            let p = entry.path();
            if p.is_dir() {
                plugin_dirs.push(p);
            }
        }

        // 准备需要加载（或刷新）的列表，与直接从缓存获取的列表
        let mut to_load = Vec::new();
        for dir in plugin_dirs {
            let manifest_path = dir.join("manifest.json");
            if let Ok(meta) = fs::metadata(&manifest_path) {
                if let Ok(mtime) = meta.modified() {
                    // 如果在 cache 中并且修改时间相同 -> 使用缓存
                    if let Some((cached_mtime, cached_manifest)) = self.cache.get(&dir) {
                        if *cached_mtime == mtime {
                            manifests.push(cached_manifest.clone());
                            continue;
                        }
                    }
                    // 需要重新读取
                    to_load.push((dir.clone(), manifest_path.clone(), mtime));
                } else {
                    // metadata 没有 modified 信息，直接尝试读取
                    to_load.push((dir.clone(), manifest_path.clone(), SystemTime::UNIX_EPOCH));
                }
            } else {
                // manifest 文件不存在或无法 stat，跳过
                continue;
            }
        }

        // 并行或串行加载 to_load
        #[cfg(feature = "parallel")]
        {
            use rayon::prelude::*;
            let loaded: Vec<_> = to_load.par_iter()
                .map(|(dir, manifest_path, mtime)| {
                    match Self::load_manifest_from_file(manifest_path) {
                        Ok(manifest) => Some((dir.clone(), *mtime, manifest)),
                        Err(e) => {
                            tracing::error!("读取 manifest 失败 {:?}: {}", manifest_path, e);
                            None
                        }
                    }
                })
                .filter_map(|x| x)
                .collect();

            for (dir, mtime, manifest) in loaded {
                self.cache.insert(dir.clone(), (mtime, manifest.clone()));
                manifests.push(manifest);
            }
        }
        #[cfg(not(feature = "parallel"))]
        {
            for (dir, manifest_path, mtime) in to_load {
                match Self::load_manifest_from_file(&manifest_path) {
                    Ok(manifest) => {
                        self.cache.insert(dir.clone(), (mtime, manifest.clone()));
                        manifests.push(manifest);
                    }
                    Err(e) => tracing::error!("读取 manifest 失败 {:?}: {}", manifest_path, e),
                }
            }
        }

        if manifests.is_empty() {
            tracing::info!("未加载到任何插件。");
        } else {
            // 可选：打印加载信息（作者合并字符串）
            for m in &manifests {
                let authors = m.authors.iter().map(|a| {
                    if let Some(link) = &a.link { format!("{} ({})", a.name, link) } else { a.name.clone() }
                }).collect::<Vec<_>>().join(", ");
                info!("已加载插件: {} (版本: {}) | 作者: {}", m.name, m.version, authors);
            }
        }

        manifests
    }

    /// 从文件读取并解析 manifest（使用 BufReader + serde_json::from_reader 避开中间 String）
    fn load_manifest_from_file(manifest_path: &Path) -> Result<PluginManifest, Box<dyn std::error::Error>> {
        let f = fs::File::open(manifest_path)?;
        let reader = BufReader::new(f);
        let manifest = serde_json::from_reader(reader)?;
        Ok(manifest)
    }
}

/// 兼容旧的函数签名：调用全局 scanner
pub fn scan_plugins<R: Runtime>(app_handle: &tauri::AppHandle<R>) -> Vec<PluginManifest> {
    let mut scanner = SCANNER.lock().expect("scanner mutex poisoned");
    scanner.scan_plugins(app_handle)
}

/// 读取插件脚本：直接读取为 bytes 然后尝试 utf8，出错会返回 err
pub fn read_plugin_script_file(plugin_name: &str, entry_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let script_path = Path::new(PLUGINS_DIR).join(plugin_name).join(entry_path);
    // 读取 bytes 并验证 utf8（避免两次拷贝）
    let bytes = fs::read(&script_path)?;
    let s = String::from_utf8(bytes)?;
    Ok(s)
}
