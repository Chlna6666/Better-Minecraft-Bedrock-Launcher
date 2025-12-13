use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
use tauri::Runtime;
use tracing::info;

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
    pub entry: String,
    pub r#type: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub version: String,
    pub authors: Vec<PluginAuthor>,
    pub dependencies: Option<Vec<PluginDependency>>,
    #[serde(skip_deserializing, default)]
    pub root_path: String,
}

// -------- 可配置项 --------
const PLUGINS_DIR: &str = "./BMCBL/plugins";

// 单例 scanner
static SCANNER: Lazy<Mutex<PluginScanner>> =
    Lazy::new(|| Mutex::new(PluginScanner::new(PLUGINS_DIR)));

struct PluginScanner {
    plugins_dir: PathBuf,
    // cache: plugin_dir_path -> (modified_time, manifest)
    cache: HashMap<PathBuf, (SystemTime, PluginManifest)>,
}

impl PluginScanner {
    fn new<P: AsRef<Path>>(dir: P) -> Self {
        Self {
            plugins_dir: dir.as_ref().to_path_buf(),
            cache: HashMap::new(),
        }
    }

    fn scan_plugins<R: Runtime>(
        &mut self,
        _app_handle: &tauri::AppHandle<R>,
    ) -> Vec<PluginManifest> {
        let plugins_dir = &self.plugins_dir;
        let mut manifests = Vec::new();

        // 确保插件主目录存在
        if !plugins_dir.exists() {
            // 尝试创建目录，方便用户
            let _ = fs::create_dir_all(plugins_dir);
        }

        let read_dir = match fs::read_dir(plugins_dir) {
            Ok(rd) => rd,
            Err(e) => {
                info!("插件目录无法读取: {:?} ({})", plugins_dir, e);
                return manifests;
            }
        };

        let mut plugin_dirs = Vec::new();
        for entry in read_dir.flatten() {
            let p = entry.path();
            if p.is_dir() {
                plugin_dirs.push(p);
            }
        }

        let mut to_load = Vec::new();

        // 1. 遍历目录，决定是读缓存还是重新加载
        for dir in plugin_dirs {
            let manifest_path = dir.join("manifest.json");

            // ✅ 获取该插件目录的绝对路径 (convertFileSrc 需要)
            let abs_root_path = match fs::canonicalize(&dir) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => dir.to_string_lossy().to_string(), // 降级处理
            };

            if let Ok(meta) = fs::metadata(&manifest_path) {
                if let Ok(mtime) = meta.modified() {
                    // 检查缓存
                    if let Some((cached_mtime, cached_manifest)) = self.cache.get_mut(&dir) {
                        if *cached_mtime == mtime {
                            // ✅ 即使是缓存，也要确保 root_path 是最新的绝对路径 (防止移动文件夹)
                            cached_manifest.root_path = abs_root_path;
                            manifests.push(cached_manifest.clone());
                            continue;
                        }
                    }
                    to_load.push((dir.clone(), manifest_path.clone(), mtime, abs_root_path));
                } else {
                    to_load.push((
                        dir.clone(),
                        manifest_path.clone(),
                        SystemTime::UNIX_EPOCH,
                        abs_root_path,
                    ));
                }
            }
        }

        // 2. 加载未命中的插件
        for (dir, manifest_path, mtime, abs_root_path) in to_load {
            match Self::load_manifest_from_file(&manifest_path) {
                Ok(mut manifest) => {
                    // ✅ 注入绝对路径
                    manifest.root_path = abs_root_path;

                    self.cache.insert(dir.clone(), (mtime, manifest.clone()));
                    manifests.push(manifest);
                }
                Err(e) => tracing::error!("读取 manifest 失败 {:?}: {}", manifest_path, e),
            }
        }

        if manifests.is_empty() {
            info!("未加载到任何插件。");
        } else {
            for m in &manifests {
                info!("已加载插件: {} | 路径: {}", m.name, m.root_path);
            }
        }

        manifests
    }

    fn load_manifest_from_file(
        manifest_path: &Path,
    ) -> Result<PluginManifest, Box<dyn std::error::Error>> {
        let f = fs::File::open(manifest_path)?;
        let reader = BufReader::new(f);
        let manifest = serde_json::from_reader(reader)?;
        Ok(manifest)
    }
}

pub fn scan_plugins<R: Runtime>(app_handle: &tauri::AppHandle<R>) -> Vec<PluginManifest> {
    let mut scanner = SCANNER.lock().expect("scanner mutex poisoned");
    scanner.scan_plugins(app_handle)
}

pub fn read_plugin_script_file(
    plugin_name: &str,
    entry_path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let script_path = Path::new(PLUGINS_DIR).join(plugin_name).join(entry_path);
    let bytes = fs::read(&script_path)?;
    let s = String::from_utf8(bytes)?;
    Ok(s)
}
