use super::callbacks::{self, host_log, host_send_event};
use super::interface::{InitPluginFn, PluginContext};
pub(crate) use super::models::PluginManifest;
use libloading::{Library, Symbol};
use once_cell::sync::Lazy;
use std::{
    collections::HashMap,
    ffi::CString,
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
use tauri::AppHandle;
use tracing::{error, info};

const PLUGINS_DIR: &str = "./BMCBL/plugins";

static SCANNER: Lazy<Mutex<PluginScanner>> =
    Lazy::new(|| Mutex::new(PluginScanner::new(PLUGINS_DIR)));

pub struct PluginScanner {
    plugins_dir: PathBuf,
    cache: HashMap<PathBuf, (SystemTime, PluginManifest)>,
    loaded_libraries: HashMap<String, Library>,
    // 保持 CString 的所有权
    plugin_names: HashMap<String, CString>,
}

impl PluginScanner {
    fn new<P: AsRef<Path>>(dir: P) -> Self {
        Self {
            plugins_dir: dir.as_ref().to_path_buf(),
            cache: HashMap::new(),
            loaded_libraries: HashMap::new(),
            plugin_names: HashMap::new(),
        }
    }

    pub fn scan_plugins(&mut self, app_handle: &AppHandle) -> Vec<PluginManifest> {
        // 设置全局 Handle 供回调使用
        callbacks::set_global_app_handle(app_handle.clone());

        let plugins_dir = &self.plugins_dir;
        let mut manifests = Vec::new();

        if !plugins_dir.exists() {
            let _ = fs::create_dir_all(plugins_dir);
        }

        // 1. 读取目录
        let read_dir = match fs::read_dir(plugins_dir) {
            Ok(rd) => rd,
            Err(e) => {
                info!("插件目录无法读取: {:?} ({})", plugins_dir, e);
                return manifests;
            }
        };

        // 2. 收集插件目录
        let mut plugin_dirs = Vec::new();
        for entry in read_dir.flatten() {
            if entry.path().is_dir() {
                plugin_dirs.push(entry.path());
            }
        }

        // 3. 处理 Manifest (缓存检查)
        let mut to_load = Vec::new();

        for dir in plugin_dirs {
            let manifest_path = dir.join("manifest.json");
            let abs_root_path = fs::canonicalize(&dir)
                .unwrap_or_else(|_| dir.clone())
                .to_string_lossy()
                .to_string();

            if let Ok(meta) = fs::metadata(&manifest_path) {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let mut cache_hit = false;

                if let Some((cached_mtime, cached_manifest)) = self.cache.get_mut(&dir) {
                    if *cached_mtime == mtime {
                        // 缓存命中
                        let mut m = cached_manifest.clone();
                        m.root_path = abs_root_path.clone(); // 更新路径以防变动
                        manifests.push(m.clone());

                        if m.r#type == "native" {
                            self.ensure_dll_loaded(&m);
                        }
                        cache_hit = true;
                    }
                }

                if !cache_hit {
                    to_load.push((dir, manifest_path, mtime, abs_root_path));
                }
            }
        }

        // 4. 加载新的/修改过的插件
        for (dir, manifest_path, mtime, abs_root_path) in to_load {
            match Self::load_manifest_from_file(&manifest_path) {
                Ok(mut manifest) => {
                    manifest.root_path = abs_root_path;

                    if manifest.r#type == "native" {
                        self.load_dll(&manifest);
                    }

                    self.cache.insert(dir, (mtime, manifest.clone()));
                    manifests.push(manifest);
                }
                Err(e) => error!("Manifest 解析失败 {:?}: {}", manifest_path, e),
            }
        }

        manifests
    }

    fn ensure_dll_loaded(&mut self, manifest: &PluginManifest) {
        if !self.loaded_libraries.contains_key(&manifest.name) {
            info!("重新加载 Native 插件: {}", manifest.name);
            self.load_dll(manifest);
        }
    }

    fn load_dll(&mut self, manifest: &PluginManifest) {
        let dll_path = Path::new(&manifest.root_path).join(&manifest.entry);
        info!("正在加载 DLL: {:?}", dll_path);

        unsafe {
            // 1. 加载 Library
            let lib = match Library::new(&dll_path) {
                Ok(l) => l,
                Err(e) => {
                    error!("DLL 加载失败 [{}]: {}", manifest.name, e);
                    return;
                }
            };

            // 2. 获取初始化符号
            let init_fn: Symbol<InitPluginFn> = match lib.get(b"init_plugin") {
                Ok(s) => s,
                Err(e) => {
                    error!("DLL 缺少 init_plugin 符号 [{}]: {}", manifest.name, e);
                    return;
                }
            };

            // 3. 准备名称指针 (生命周期管理)
            let name_c = match CString::new(manifest.name.clone()) {
                Ok(c) => c,
                Err(_) => {
                    error!("插件名称包含非法字符: {}", manifest.name);
                    return;
                }
            };
            self.plugin_names.insert(manifest.name.clone(), name_c);
            let name_ptr = self.plugin_names.get(&manifest.name).unwrap().as_ptr();

            // 4. 构建上下文
            let context = PluginContext {
                api_version: 1,
                plugin_name: name_ptr,
                log_fn: host_log,
                send_event_fn: host_send_event, // 使用 callbacks.rs 中的新版函数
            };

            // 5. 初始化插件
            let result = init_fn(&context);
            if result == 0 {
                info!("Native 插件初始化成功: {}", manifest.name);
                self.loaded_libraries.insert(manifest.name.clone(), lib);
            } else {
                error!("Native 插件初始化返回错误代码: {}", result);
                self.plugin_names.remove(&manifest.name);
            }
        }
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

// -------- 公开 API --------

pub fn scan_plugins(app_handle: &AppHandle) -> Vec<PluginManifest> {
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