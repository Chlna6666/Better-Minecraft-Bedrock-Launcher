//! 插件沙箱策略数据：capability 位、网络/资源/外链 allowlist 与资源限额。
//!
//! 这里只保存解析后的纯数据；manifest 解析与应用侧 UI 归宿主（app）负责。

/// capability 位集（与 manifest 的 `capabilities` 声明一一对应）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PluginCapabilities {
    pub ui_page: bool,
    pub ui_window: bool,
    pub ui_hook: bool,
    pub event_global: bool,
    pub toast: bool,
    pub network_http: bool,
    pub clipboard_read: bool,
    pub clipboard_write: bool,
    pub external_open: bool,
    pub resource_read: bool,
    pub storage_kv: bool,
    pub task_progress: bool,
    pub config_read: bool,
    pub config_write: bool,
    pub download_start: bool,
}

/// manifest `limits` 的运行时形态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginLimits {
    pub max_http_bytes: u64,
    pub max_resource_bytes: u64,
    pub max_storage_bytes: u64,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            max_http_bytes: 512 * 1024,
            max_resource_bytes: 1024 * 1024,
            max_storage_bytes: 1024 * 1024,
        }
    }
}

/// manifest allowlist 原始条目。
/// - URL：仅允许 HTTPS，按源和路径段匹配；
/// - 路径：条目去尾 `/` 后按目录边界前缀匹配；
/// - 空列表表示不允许任何目标。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Allowlist {
    pub entries: Vec<String>,
}

impl Allowlist {
    #[must_use]
    pub fn new(entries: Vec<String>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn allows_url(&self, url: &str) -> bool {
        let Some(target) = https_url(url) else {
            return false;
        };
        self.entries
            .iter()
            .filter_map(|allowed| https_url(allowed))
            .any(|allowed| {
                if target.origin() != allowed.origin()
                    || allowed
                        .query()
                        .is_some_and(|query| target.query() != Some(query))
                {
                    return false;
                }
                target.path() == allowed.path()
                    || target
                        .path()
                        .strip_prefix(allowed.path())
                        .is_some_and(|suffix| {
                            allowed.path().ends_with('/') || suffix.starts_with('/')
                        })
            })
    }

    #[must_use]
    pub fn allows_path(&self, path: &str) -> bool {
        self.entries.iter().any(|allowed| {
            let allowed = allowed.trim_end_matches('/');
            path == allowed || path.starts_with(&format!("{allowed}/"))
        })
    }
}

fn https_url(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text).ok()?;
    (url.scheme() == "https"
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

/// 每个插件解析后的沙箱策略快照。
#[derive(Clone, Debug, Default)]
pub struct PluginPolicy {
    pub plugin_id: String,
    pub capabilities: PluginCapabilities,
    pub limits: PluginLimits,
    pub network_allow: Allowlist,
    pub resource_allow: Allowlist,
    pub external_allow: Allowlist,
}

impl PluginPolicy {
    #[must_use]
    pub fn require(&self, granted: bool, operation: &str) -> Result<(), crate::PluginError> {
        if granted {
            return Ok(());
        }
        Err(crate::PluginError::denied(format!(
            "plugin {} lacks the capability required for {operation}",
            self.plugin_id
        )))
    }
}
