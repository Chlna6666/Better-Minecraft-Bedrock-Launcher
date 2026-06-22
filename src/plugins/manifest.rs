use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Entry};
use tracing::warn;

pub const CURRENT_API_VERSION: &str = "0.4";
pub const PLUGIN_MANIFEST_FILE: &str = "plugin.toml";
pub const DEFAULT_ENTRY: &str = "plugin.wasm";
pub const PLUGIN_PACKAGE_EXTENSION: &str = "bmcblx";
pub const PLUGIN_USER_CONFIG_FILE: &str = "config.toml";
pub const CURRENT_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_MEMORY_LIMIT_MB: u32 = 64;
pub const DEFAULT_HTTP_LIMIT_BYTES: u64 = 512 * 1024;
pub const DEFAULT_RESOURCE_LIMIT_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_STORAGE_LIMIT_BYTES: u64 = 1024 * 1024;
pub const HARD_MEMORY_LIMIT_MB: u32 = 64;
pub const HARD_HTTP_LIMIT_BYTES: u64 = 1024 * 1024;
pub const HARD_RESOURCE_LIMIT_BYTES: u64 = 4 * 1024 * 1024;
pub const HARD_STORAGE_LIMIT_BYTES: u64 = 8 * 1024 * 1024;
const PENDING_BACKUP_FILE: &str = ".bmcbl-plugin-backup";
const SHA256_PREFIX: &str = "sha256:";

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PluginCapability {
    UiPage,
    UiWindow,
    UiHook,
    EventGlobal,
    Toast,
    NetworkHttp,
    ClipboardRead,
    ClipboardWrite,
    ExternalOpen,
    ResourceRead,
    StorageKv,
    TaskProgress,
    ConfigRead,
    ConfigWrite,
}

impl PluginCapability {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ui.page" => Some(Self::UiPage),
            "ui.window" => Some(Self::UiWindow),
            "ui.hook" => Some(Self::UiHook),
            "event.global" => Some(Self::EventGlobal),
            "toast" => Some(Self::Toast),
            "network.http" => Some(Self::NetworkHttp),
            "clipboard.read" => Some(Self::ClipboardRead),
            "clipboard.write" => Some(Self::ClipboardWrite),
            "external.open" => Some(Self::ExternalOpen),
            "resource.read" => Some(Self::ResourceRead),
            "storage.kv" => Some(Self::StorageKv),
            "task.progress" => Some(Self::TaskProgress),
            "config.read" => Some(Self::ConfigRead),
            "config.write" => Some(Self::ConfigWrite),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UiPage => "ui.page",
            Self::UiWindow => "ui.window",
            Self::UiHook => "ui.hook",
            Self::EventGlobal => "event.global",
            Self::Toast => "toast",
            Self::NetworkHttp => "network.http",
            Self::ClipboardRead => "clipboard.read",
            Self::ClipboardWrite => "clipboard.write",
            Self::ExternalOpen => "external.open",
            Self::ResourceRead => "resource.read",
            Self::StorageKv => "storage.kv",
            Self::TaskProgress => "task.progress",
            Self::ConfigRead => "config.read",
            Self::ConfigWrite => "config.write",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLoadOrder {
    Startup,
}

impl Default for PluginLoadOrder {
    fn default() -> Self {
        Self::Startup
    }
}

#[derive(Clone, Debug, Deserialize)]
struct RawPluginManifest {
    schema_version: u32,
    id: String,
    name: String,
    version: String,
    api_version: String,
    #[serde(default = "default_entry")]
    entry: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    website: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    load_order: PluginLoadOrder,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    readme: Option<String>,
    #[serde(default)]
    readme_locales: BTreeMap<String, String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    lang_dir: Option<String>,
    #[serde(default)]
    config_default: Option<String>,
    #[serde(default)]
    config_schema: Option<String>,
    #[serde(default)]
    package_hash: Option<String>,
    #[serde(default)]
    permissions: RawPluginPermissions,
    #[serde(default)]
    limits: RawPluginLimits,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RawPluginPermissions {
    #[serde(default)]
    network: RawAllowList,
    #[serde(default)]
    resource: RawAllowList,
    #[serde(default)]
    external: RawAllowList,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RawAllowList {
    #[serde(default)]
    allow: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RawPluginLimits {
    #[serde(default)]
    memory_mb: Option<u32>,
    #[serde(default)]
    max_http_bytes: Option<u64>,
    #[serde(default)]
    max_resource_bytes: Option<u64>,
    #[serde(default)]
    max_storage_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluginPermissions {
    pub network_allow: Vec<String>,
    pub resource_allow: Vec<String>,
    pub external_allow: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginLimits {
    pub memory_mb: u32,
    pub max_http_bytes: u64,
    pub max_resource_bytes: u64,
    pub max_storage_bytes: u64,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            memory_mb: DEFAULT_MEMORY_LIMIT_MB,
            max_http_bytes: DEFAULT_HTTP_LIMIT_BYTES,
            max_resource_bytes: DEFAULT_RESOURCE_LIMIT_BYTES,
            max_storage_bytes: DEFAULT_STORAGE_LIMIT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub entry: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub website: Option<String>,
    pub license: Option<String>,
    pub load_order: PluginLoadOrder,
    pub tags: Vec<String>,
    pub readme: Option<String>,
    pub readme_locales: BTreeMap<String, String>,
    pub icon: Option<String>,
    pub lang_dir: Option<String>,
    pub config_default: Option<String>,
    pub config_schema: Option<String>,
    pub package_hash: Option<String>,
    pub network_allowlist: Vec<String>,
    pub permissions: PluginPermissions,
    pub limits: PluginLimits,
    pub capabilities: BTreeSet<PluginCapability>,
    pub root_dir: PathBuf,
    pub pending_backup_dir: Option<PathBuf>,
}

impl PluginManifest {
    pub fn load_from_dir(root_dir: impl AsRef<Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref();
        let manifest_path = root_dir.join(PLUGIN_MANIFEST_FILE);
        let content = fs::read_to_string(&manifest_path)
            .with_context(|| format!("read plugin manifest {}", manifest_path.display()))?;
        Self::parse(root_dir, &content)
            .with_context(|| format!("parse plugin manifest {}", manifest_path.display()))
    }

    pub fn parse(root_dir: impl AsRef<Path>, content: &str) -> Result<Self> {
        let raw: RawPluginManifest = toml::from_str(content)?;
        validate_plugin_id(&raw.id)?;

        if raw.schema_version != CURRENT_SCHEMA_VERSION {
            bail!("unsupported plugin schema_version {}", raw.schema_version);
        }

        if raw.api_version != CURRENT_API_VERSION {
            bail!(
                "unsupported plugin api_version {}, expected {}",
                raw.api_version,
                CURRENT_API_VERSION
            );
        }

        if raw.name.trim().is_empty() {
            bail!("plugin name must not be empty");
        }

        if raw.version.trim().is_empty() {
            bail!("plugin version must not be empty");
        }

        if let Some(website) = &raw.website {
            if website.trim().is_empty() {
                bail!("plugin website must not be empty when present");
            }
        }

        if let Some(license) = &raw.license {
            if license.trim().is_empty() {
                bail!("plugin license must not be empty when present");
            }
        }

        if raw.entry.trim().is_empty() {
            bail!("plugin entry must not be empty");
        }

        if raw.entry.contains('\\') || raw.entry.contains('/') || raw.entry.contains("..") {
            bail!("plugin entry must be a file name inside the plugin directory");
        }
        validate_optional_package_path(raw.readme.as_deref(), false)?;
        validate_optional_package_paths(raw.readme_locales.values().map(String::as_str), false)?;
        validate_optional_package_path(raw.icon.as_deref(), false)?;
        validate_optional_package_path(raw.lang_dir.as_deref(), true)?;
        validate_optional_package_path(raw.config_default.as_deref(), false)?;
        validate_optional_package_path(raw.config_schema.as_deref(), false)?;
        let permissions = plugin_permissions_from_raw(raw.permissions)?;
        let limits = plugin_limits_from_raw(raw.limits);

        let mut capabilities = BTreeSet::new();
        for capability in &raw.capabilities {
            let Some(parsed) = PluginCapability::parse(capability) else {
                bail!("unsupported plugin capability {capability}");
            };
            capabilities.insert(parsed);
        }
        if capabilities.is_empty() {
            bail!("plugin capabilities must not be empty");
        }

        let mut tags = Vec::new();
        for tag in raw.tags {
            let tag = tag.trim();
            if tag.is_empty() {
                bail!("plugin tags must not contain empty values");
            }
            tags.push(tag.to_string());
        }

        if let Some(package_hash) = &raw.package_hash {
            validate_package_hash(package_hash)?;
        }

        let root_dir = root_dir.as_ref().to_path_buf();
        let pending_backup_dir = pending_backup_dir_for(&root_dir);

        Ok(Self {
            schema_version: raw.schema_version,
            id: raw.id,
            name: raw.name,
            version: raw.version,
            api_version: raw.api_version,
            entry: raw.entry,
            authors: raw.authors,
            description: raw.description,
            website: raw.website,
            license: raw.license,
            load_order: raw.load_order,
            tags,
            readme: raw.readme,
            readme_locales: raw.readme_locales,
            icon: raw.icon,
            lang_dir: raw.lang_dir,
            config_default: raw.config_default,
            config_schema: raw.config_schema,
            package_hash: raw.package_hash,
            network_allowlist: permissions.network_allow.clone(),
            permissions,
            limits,
            capabilities,
            root_dir,
            pending_backup_dir,
        })
    }

    pub fn wasm_path(&self) -> PathBuf {
        self.root_dir.join(&self.entry)
    }

    pub fn readme_path(&self) -> Option<PathBuf> {
        self.readme.as_ref().map(|path| self.root_dir.join(path))
    }

    pub fn readme_path_for_locale(&self, locale: &str) -> Option<PathBuf> {
        for candidate in readme_locale_candidates(locale) {
            if let Some(path) = self.readme_locales.get(&candidate) {
                return Some(self.root_dir.join(path));
            }
        }

        self.readme_path().or_else(|| {
            self.readme_locales
                .values()
                .next()
                .map(|path| self.root_dir.join(path))
        })
    }

    pub fn icon_path(&self) -> Option<PathBuf> {
        self.icon.as_ref().map(|path| self.root_dir.join(path))
    }

    pub fn lang_dir_path(&self) -> Option<PathBuf> {
        self.lang_dir.as_ref().map(|path| self.root_dir.join(path))
    }

    pub fn config_default_path(&self) -> Option<PathBuf> {
        self.config_default
            .as_ref()
            .map(|path| self.root_dir.join(path))
    }

    pub fn config_schema_path(&self) -> Option<PathBuf> {
        self.config_schema
            .as_ref()
            .map(|path| self.root_dir.join(path))
    }

    pub fn user_config_path(&self) -> PathBuf {
        self.root_dir.join(PLUGIN_USER_CONFIG_FILE)
    }

    pub fn has_capability(&self, capability: &PluginCapability) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn require_capability(&self, capability: PluginCapability) -> Result<()> {
        if self.has_capability(&capability) {
            return Ok(());
        }

        Err(anyhow!(
            "plugin {} missing required capability {}",
            self.id,
            capability.as_str()
        ))
    }

    pub fn allows_network_url(&self, url: &str) -> bool {
        allowlist_contains_url(&self.permissions.network_allow, url)
    }

    pub fn allows_external_url(&self, url: &str) -> bool {
        allowlist_contains_url(&self.permissions.external_allow, url)
    }

    pub fn resource_path(&self, path: &str) -> Result<PathBuf> {
        let relative_path = validate_package_path(path, false)?;
        let normalized = relative_path.to_string_lossy().replace('\\', "/");
        if !allowlist_contains_path(&self.permissions.resource_allow, &normalized) {
            bail!(
                "plugin {} is not allowed to read resource {}",
                self.id,
                normalized
            );
        }
        Ok(self.root_dir.join(relative_path))
    }
}

fn plugin_permissions_from_raw(raw: RawPluginPermissions) -> Result<PluginPermissions> {
    validate_url_allowlist("permissions.network.allow", &raw.network.allow)?;
    validate_url_allowlist("permissions.external.allow", &raw.external.allow)?;
    validate_package_allowlist("permissions.resource.allow", &raw.resource.allow)?;
    Ok(PluginPermissions {
        network_allow: raw.network.allow,
        resource_allow: raw.resource.allow,
        external_allow: raw.external.allow,
    })
}

fn plugin_limits_from_raw(raw: RawPluginLimits) -> PluginLimits {
    PluginLimits {
        memory_mb: raw
            .memory_mb
            .unwrap_or(DEFAULT_MEMORY_LIMIT_MB)
            .clamp(1, HARD_MEMORY_LIMIT_MB),
        max_http_bytes: raw
            .max_http_bytes
            .unwrap_or(DEFAULT_HTTP_LIMIT_BYTES)
            .clamp(1, HARD_HTTP_LIMIT_BYTES),
        max_resource_bytes: raw
            .max_resource_bytes
            .unwrap_or(DEFAULT_RESOURCE_LIMIT_BYTES)
            .clamp(1, HARD_RESOURCE_LIMIT_BYTES),
        max_storage_bytes: raw
            .max_storage_bytes
            .unwrap_or(DEFAULT_STORAGE_LIMIT_BYTES)
            .clamp(1, HARD_STORAGE_LIMIT_BYTES),
    }
}

fn allowlist_contains_url(allowlist: &[String], url: &str) -> bool {
    allowlist
        .iter()
        .any(|allowed| url == allowed || url.starts_with(allowed))
}

fn allowlist_contains_path(allowlist: &[String], path: &str) -> bool {
    allowlist.iter().any(|allowed| {
        let allowed = allowed.trim_end_matches('/');
        path == allowed || path.starts_with(&format!("{allowed}/"))
    })
}

fn validate_package_hash(package_hash: &str) -> Result<()> {
    let Some(hex) = package_hash.strip_prefix(SHA256_PREFIX) else {
        bail!("plugin package_hash must start with {SHA256_PREFIX}");
    };

    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("plugin package_hash must contain a 64 character sha256 hex digest");
    }

    Ok(())
}

fn validate_url_allowlist(label: &str, urls: &[String]) -> Result<()> {
    for url in urls {
        if !url.starts_with("https://") {
            bail!("{label} entries must use https:// URLs");
        }
        if url.contains(char::is_whitespace) {
            bail!("{label} entries must not contain whitespace");
        }
    }
    Ok(())
}

fn validate_package_allowlist(label: &str, paths: &[String]) -> Result<()> {
    for path in paths {
        validate_package_path(path, true).with_context(|| format!("validate {label} entry"))?;
    }
    Ok(())
}

fn pending_backup_dir_for(root_dir: &Path) -> Option<PathBuf> {
    let marker_path = root_dir.join(PENDING_BACKUP_FILE);
    let backup = fs::read_to_string(marker_path).ok()?;
    let backup = backup.trim();
    if backup.is_empty() {
        return None;
    }
    Some(PathBuf::from(backup))
}

fn default_entry() -> String {
    DEFAULT_ENTRY.to_string()
}

pub fn validate_plugin_id(id: &str) -> Result<()> {
    let len = id.len();
    if !(3..=64).contains(&len) {
        bail!("plugin id must be 3..=64 bytes");
    }

    let mut previous_dash = false;
    for (index, byte) in id.bytes().enumerate() {
        let allowed = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !allowed {
            bail!("plugin id may only contain lowercase ascii letters, digits, and '-'");
        }

        if index == 0 && !byte.is_ascii_lowercase() {
            bail!("plugin id must start with a lowercase ascii letter");
        }

        if byte == b'-' && previous_dash {
            bail!("plugin id must not contain consecutive '-' characters");
        }

        previous_dash = byte == b'-';
    }

    if id.ends_with('-') {
        bail!("plugin id must not end with '-'");
    }

    Ok(())
}

pub fn load_manifests_sorted(plugins_dir: impl AsRef<Path>) -> Result<Vec<PluginManifest>> {
    load_manifests_from_sources(
        plugins_dir,
        std::env::temp_dir().join("bmcbl-plugin-packages"),
    )
}

pub fn load_manifests_from_sources(
    plugins_dir: impl AsRef<Path>,
    _package_cache_dir: impl AsRef<Path>,
) -> Result<Vec<PluginManifest>> {
    let plugins_dir = plugins_dir.as_ref();
    if !plugins_dir.exists() {
        return Ok(Vec::new());
    }

    install_packages_in_plugins_dir(plugins_dir)?;

    let mut manifests = Vec::new();
    for entry in fs::read_dir(plugins_dir)
        .with_context(|| format!("read plugins directory {}", plugins_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && !is_internal_install_dir(&path) {
            match PluginManifest::load_from_dir(&path).and_then(|manifest| {
                ensure_user_config(&manifest)?;
                Ok(manifest)
            }) {
                Ok(manifest) => manifests.push(manifest),
                Err(error) => {
                    warn!(
                        path = %path.display(),
                        error = %format_error_chain(&error),
                        "plugin manifest skipped"
                    );
                }
            }
        }
    }

    manifests.sort_by(|left, right| left.id.cmp(&right.id));
    let mut seen = BTreeSet::new();
    for manifest in &manifests {
        if !seen.insert(manifest.id.clone()) {
            bail!("duplicate plugin id {}", manifest.id);
        }
    }

    Ok(manifests)
}

pub fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

pub fn commit_installed_package(manifest: &PluginManifest) -> Result<()> {
    let Some(backup_dir) = manifest.pending_backup_dir.as_ref() else {
        return Ok(());
    };
    let marker_path = manifest.root_dir.join(PENDING_BACKUP_FILE);
    if marker_path.exists() {
        fs::remove_file(&marker_path)
            .with_context(|| format!("remove {}", marker_path.display()))?;
    }
    remove_dir_if_exists(backup_dir)
}

pub fn rollback_installed_package(manifest: &PluginManifest) -> Result<()> {
    let Some(backup_dir) = manifest.pending_backup_dir.as_ref() else {
        return Ok(());
    };
    let install_dir = &manifest.root_dir;
    remove_dir_if_exists(install_dir)?;
    if backup_dir.exists() {
        fs::rename(backup_dir, install_dir).with_context(|| {
            format!(
                "restore plugin {} from {}",
                install_dir.display(),
                backup_dir.display()
            )
        })?;
    }
    Ok(())
}

fn install_packages_in_plugins_dir(plugins_dir: &Path) -> Result<()> {
    let mut packages = Vec::new();
    for entry in fs::read_dir(plugins_dir)
        .with_context(|| format!("read plugins directory {}", plugins_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if is_plugin_package_path(&path) {
            packages.push(path);
        }
    }

    packages.sort();
    for package_path in packages {
        if let Err(error) = install_manifest_from_package(&package_path, plugins_dir)
            .with_context(|| format!("install plugin package {}", package_path.display()))
        {
            warn!(error = %format_error_chain(&error), "plugin package install failed");
            if let Err(move_error) = move_failed_package(&package_path, plugins_dir, &error) {
                warn!(
                    error = %format_error_chain(&move_error),
                    "failed to isolate invalid plugin package"
                );
            }
        }
    }

    Ok(())
}

fn is_internal_install_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".bmcbl-plugin-"))
}

fn is_plugin_package_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PLUGIN_PACKAGE_EXTENSION))
}

pub fn load_manifest_from_package(
    package_path: &Path,
    package_cache_dir: &Path,
) -> Result<PluginManifest> {
    if !is_plugin_package_path(package_path) {
        bail!("plugin package must use .{PLUGIN_PACKAGE_EXTENSION} extension");
    }

    let package_file = fs::File::open(package_path)
        .with_context(|| format!("open plugin package {}", package_path.display()))?;
    let mut archive = zip::ZipArchive::new(package_file)
        .with_context(|| format!("read plugin package {}", package_path.display()))?;
    let manifest_text = read_package_entry_to_string(&mut archive, PLUGIN_MANIFEST_FILE)
        .with_context(|| {
            format!(
                "read {PLUGIN_MANIFEST_FILE} from {}",
                package_path.display()
            )
        })?;
    let manifest_for_metadata = PluginManifest::parse(package_path, &manifest_text)?;
    let wasm = read_package_entry_to_bytes(&mut archive, &manifest_for_metadata.entry)
        .with_context(|| {
            format!(
                "read {} from {}",
                manifest_for_metadata.entry,
                package_path.display()
            )
        })?;

    if let Some(expected_hash) = &manifest_for_metadata.package_hash {
        let actual_hash = format!("{SHA256_PREFIX}{}", sha256_hex(&wasm));
        if expected_hash != &actual_hash {
            bail!(
                "plugin package wasm hash mismatch for {}: expected {}, got {}",
                manifest_for_metadata.id,
                expected_hash,
                actual_hash
            );
        }
    }

    let cache_key = package_cache_key(&manifest_for_metadata, &wasm);
    let unpack_dir = package_cache_dir
        .join(&manifest_for_metadata.id)
        .join(cache_key);
    fs::create_dir_all(&unpack_dir)
        .with_context(|| format!("create plugin cache {}", unpack_dir.display()))?;
    write_atomic(
        unpack_dir.join(PLUGIN_MANIFEST_FILE),
        manifest_text.as_bytes(),
    )?;
    write_atomic(unpack_dir.join(&manifest_for_metadata.entry), &wasm)?;
    unpack_package_resources(&mut archive, &unpack_dir, &manifest_for_metadata)?;
    let cached_manifest = PluginManifest::load_from_dir(unpack_dir)?;
    ensure_user_config(&cached_manifest)?;

    Ok(cached_manifest)
}

pub fn install_manifest_from_package(
    package_path: &Path,
    plugins_dir: &Path,
) -> Result<PluginManifest> {
    if !is_plugin_package_path(package_path) {
        bail!("plugin package must use .{PLUGIN_PACKAGE_EXTENSION} extension");
    }

    let package_file = fs::File::open(package_path)
        .with_context(|| format!("open plugin package {}", package_path.display()))?;
    let mut archive = zip::ZipArchive::new(package_file)
        .with_context(|| format!("read plugin package {}", package_path.display()))?;
    let manifest_text = read_package_entry_to_string(&mut archive, PLUGIN_MANIFEST_FILE)
        .with_context(|| {
            format!(
                "read {PLUGIN_MANIFEST_FILE} from {}",
                package_path.display()
            )
        })?;
    let manifest_for_metadata = PluginManifest::parse(package_path, &manifest_text)?;
    let wasm = read_package_entry_to_bytes(&mut archive, &manifest_for_metadata.entry)
        .with_context(|| {
            format!(
                "read {} from {}",
                manifest_for_metadata.entry,
                package_path.display()
            )
        })?;

    if let Some(expected_hash) = &manifest_for_metadata.package_hash {
        let actual_hash = format!("{SHA256_PREFIX}{}", sha256_hex(&wasm));
        if expected_hash != &actual_hash {
            bail!(
                "plugin package wasm hash mismatch for {}: expected {}, got {}",
                manifest_for_metadata.id,
                expected_hash,
                actual_hash
            );
        }
    }

    let install_dir = plugins_dir.join(&manifest_for_metadata.id);
    if installed_package_matches(&install_dir, &manifest_for_metadata)? {
        remove_installed_package_file(package_path)?;
        return PluginManifest::load_from_dir(install_dir);
    }

    validate_replace_target(&install_dir, &manifest_for_metadata.id)?;

    let install_key = package_cache_key(&manifest_for_metadata, &wasm);
    let temp_dir = plugins_dir.join(format!(
        ".bmcbl-plugin-install-{}-{install_key}",
        manifest_for_metadata.id
    ));
    let backup_dir = plugins_dir.join(format!(
        ".bmcbl-plugin-previous-{}-{install_key}",
        manifest_for_metadata.id
    ));
    remove_dir_if_exists(&temp_dir)?;
    remove_dir_if_exists(&backup_dir)?;
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("create temporary plugin install {}", temp_dir.display()))?;
    write_atomic(
        temp_dir.join(PLUGIN_MANIFEST_FILE),
        manifest_text.as_bytes(),
    )?;
    write_atomic(temp_dir.join(&manifest_for_metadata.entry), &wasm)?;
    unpack_package_resources(&mut archive, &temp_dir, &manifest_for_metadata)?;

    let installed_manifest = PluginManifest::load_from_dir(&temp_dir)
        .with_context(|| format!("validate installed plugin {}", temp_dir.display()))?;
    if installed_manifest.wasm_path().exists() {
        prepare_user_config_for_install(&install_dir, &temp_dir, &installed_manifest)?;
        replace_plugin_dir(&temp_dir, &install_dir, &backup_dir)?;
    } else {
        bail!(
            "plugin package {} did not install {}",
            manifest_for_metadata.id,
            manifest_for_metadata.entry
        );
    }

    remove_installed_package_file(package_path)?;
    let manifest = PluginManifest::load_from_dir(install_dir)?;
    ensure_user_config(&manifest)?;
    Ok(manifest)
}

fn installed_package_matches(
    install_dir: &Path,
    package_manifest: &PluginManifest,
) -> Result<bool> {
    if !install_dir.exists() {
        return Ok(false);
    }

    let installed = PluginManifest::load_from_dir(install_dir)
        .with_context(|| format!("read installed plugin {}", install_dir.display()))?;
    Ok(installed.id == package_manifest.id
        && installed.version == package_manifest.version
        && installed.package_hash == package_manifest.package_hash
        && package_manifest.package_hash.is_some())
}

fn validate_replace_target(install_dir: &Path, plugin_id: &str) -> Result<()> {
    if !install_dir.exists() {
        return Ok(());
    }

    let installed = PluginManifest::load_from_dir(install_dir)
        .with_context(|| format!("read installed plugin {}", install_dir.display()))?;
    if installed.id != plugin_id {
        bail!(
            "refusing to replace plugin directory {} because it contains plugin {}",
            install_dir.display(),
            installed.id
        );
    }
    if installed.package_hash.is_none() {
        bail!(
            "refusing to replace development plugin directory {} with package {}",
            install_dir.display(),
            plugin_id
        );
    }
    Ok(())
}

fn replace_plugin_dir(temp_dir: &Path, install_dir: &Path, backup_dir: &Path) -> Result<()> {
    if install_dir.exists() {
        fs::rename(install_dir, backup_dir).with_context(|| {
            format!(
                "move existing plugin {} to {}",
                install_dir.display(),
                backup_dir.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(temp_dir, install_dir) {
        if backup_dir.exists() {
            fs::rename(backup_dir, install_dir).with_context(|| {
                format!(
                    "restore plugin {} after failed install",
                    install_dir.display()
                )
            })?;
        }
        return Err(error).with_context(|| {
            format!(
                "move installed plugin {} to {}",
                temp_dir.display(),
                install_dir.display()
            )
        });
    }

    if backup_dir.exists() {
        write_atomic(
            install_dir.join(PENDING_BACKUP_FILE),
            backup_dir.to_string_lossy().as_bytes(),
        )?;
    }
    Ok(())
}

fn remove_installed_package_file(package_path: &Path) -> Result<()> {
    fs::remove_file(package_path)
        .with_context(|| format!("remove installed plugin package {}", package_path.display()))
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn move_failed_package(
    package_path: &Path,
    plugins_dir: &Path,
    error: &anyhow::Error,
) -> Result<()> {
    if !package_path.exists() {
        return Ok(());
    }
    let failed_dir = plugins_dir.join(".bmcbl-plugin-failed");
    fs::create_dir_all(&failed_dir)
        .with_context(|| format!("create failed plugin package dir {}", failed_dir.display()))?;
    let file_name = package_path
        .file_name()
        .ok_or_else(|| anyhow!("package path has no file name: {}", package_path.display()))?;
    let failed_package_path = unique_failed_path(&failed_dir.join(file_name));
    fs::rename(package_path, &failed_package_path).with_context(|| {
        format!(
            "move invalid plugin package {} to {}",
            package_path.display(),
            failed_package_path.display()
        )
    })?;
    let error_path = failed_package_path.with_extension("error.txt");
    fs::write(&error_path, format!("{}\n", format_error_chain(error)))
        .with_context(|| format!("write {}", error_path.display()))?;
    Ok(())
}

fn unique_failed_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("plugin");
    let extension = path.extension().and_then(|extension| extension.to_str());
    for index in 1..1000 {
        let file_name = if let Some(extension) = extension {
            format!("{stem}-{index}.{extension}")
        } else {
            format!("{stem}-{index}")
        };
        let candidate = path.with_file_name(file_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.with_file_name(format!("{stem}-failed"))
}

fn package_cache_key(manifest: &PluginManifest, wasm: &[u8]) -> String {
    let hash = sha256_hex(wasm);
    let short_hash = hash.get(..16).unwrap_or(&hash);
    format!("{}-{short_hash}", sanitize_cache_segment(&manifest.version))
}

fn sanitize_cache_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn read_package_entry_to_string<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String> {
    let bytes = read_package_entry_to_bytes(archive, name)?;
    String::from_utf8(bytes).with_context(|| format!("plugin package entry {name} is not utf-8"))
}

fn read_package_entry_to_bytes<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("plugin package missing {name}"))?;
    let mut bytes = Vec::with_capacity(entry.size().try_into().unwrap_or(0));
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn unpack_package_resources<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    unpack_dir: &Path,
    manifest: &PluginManifest,
) -> Result<()> {
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.replace('\\', "/");
        if name.ends_with('/') || is_core_package_entry(&name) {
            continue;
        }
        if !is_allowed_package_entry(&name, manifest) {
            continue;
        }

        let relative_path = validate_package_path(&name, name.ends_with('/'))?;
        let output_path = unpack_dir.join(relative_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::with_capacity(entry.size().try_into().unwrap_or(0));
        entry.read_to_end(&mut bytes)?;
        write_atomic(output_path, &bytes)?;
    }

    Ok(())
}

fn is_core_package_entry(name: &str) -> bool {
    name == PLUGIN_MANIFEST_FILE || name == DEFAULT_ENTRY
}

fn is_allowed_package_entry(name: &str, manifest: &PluginManifest) -> bool {
    is_manifest_declared_resource_entry(name, manifest)
        || allowlist_contains_path(&manifest.permissions.resource_allow, name)
}

fn is_manifest_declared_resource_entry(name: &str, manifest: &PluginManifest) -> bool {
    manifest.readme.as_deref() == Some(name)
        || manifest
            .readme_locales
            .values()
            .any(|path| path.as_str() == name)
        || manifest.icon.as_deref() == Some(name)
        || manifest.config_default.as_deref() == Some(name)
        || manifest.config_schema.as_deref() == Some(name)
        || manifest
            .lang_dir
            .as_deref()
            .is_some_and(|path| name == path || name.starts_with(&format!("{path}/")))
}

pub fn ensure_user_config(manifest: &PluginManifest) -> Result<()> {
    let user_config_path = manifest.user_config_path();
    let Some(default_path) = manifest.config_default_path() else {
        return Ok(());
    };
    if !default_path.exists() {
        return Ok(());
    }
    if !user_config_path.exists() {
        let default_text = fs::read_to_string(&default_path)
            .with_context(|| format!("read default plugin config {}", default_path.display()))?;
        write_atomic(user_config_path, default_text.as_bytes())?;
        return Ok(());
    }
    merge_config_file(&default_path, &user_config_path)
}

fn prepare_user_config_for_install(
    install_dir: &Path,
    temp_dir: &Path,
    manifest: &PluginManifest,
) -> Result<()> {
    let new_user_config = temp_dir.join(PLUGIN_USER_CONFIG_FILE);
    let old_user_config = install_dir.join(PLUGIN_USER_CONFIG_FILE);
    if old_user_config.exists() {
        fs::copy(&old_user_config, &new_user_config).with_context(|| {
            format!(
                "copy plugin config {} to {}",
                old_user_config.display(),
                new_user_config.display()
            )
        })?;
    }
    ensure_user_config(&PluginManifest {
        root_dir: temp_dir.to_path_buf(),
        ..manifest.clone()
    })
}

fn merge_config_file(default_path: &Path, user_path: &Path) -> Result<()> {
    let default_text = fs::read_to_string(default_path)
        .with_context(|| format!("read default plugin config {}", default_path.display()))?;
    let user_text = fs::read_to_string(user_path)
        .with_context(|| format!("read plugin config {}", user_path.display()))?;
    let default_doc = default_text
        .parse::<DocumentMut>()
        .with_context(|| format!("parse default plugin config {}", default_path.display()))?;
    let mut user_doc = user_text
        .parse::<DocumentMut>()
        .with_context(|| format!("parse plugin config {}", user_path.display()))?;

    if merge_toml_tables(default_doc.as_table(), user_doc.as_table_mut()) {
        write_atomic(user_path.to_path_buf(), user_doc.to_string().as_bytes())?;
    }
    Ok(())
}

fn merge_toml_tables(default: &toml_edit::Table, user: &mut toml_edit::Table) -> bool {
    let mut changed = false;
    for (key, default_item) in default.iter() {
        match user.entry(key) {
            Entry::Occupied(mut occupied) => {
                if let (Some(default_table), Some(user_table)) =
                    (default_item.as_table(), occupied.get_mut().as_table_mut())
                {
                    changed |= merge_toml_tables(default_table, user_table);
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(default_item.clone());
                changed = true;
            }
        }
    }
    changed
}

pub fn read_user_config(manifest: &PluginManifest) -> Result<String> {
    ensure_user_config(manifest)?;
    let path = manifest.user_config_path();
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).with_context(|| format!("read plugin config {}", path.display()))
}

pub fn write_user_config(manifest: &PluginManifest, content: &str) -> Result<()> {
    content
        .parse::<DocumentMut>()
        .with_context(|| format!("parse plugin config for {}", manifest.id))?;
    write_atomic(manifest.user_config_path(), content.as_bytes())
}

fn write_atomic(path: PathBuf, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&temp_path)
            .with_context(|| format!("create temporary file {}", temp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write temporary file {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary file {}", temp_path.display()))?;
    }
    fs::rename(&temp_path, &path).with_context(|| {
        format!(
            "replace {} with temporary file {}",
            path.display(),
            temp_path.display()
        )
    })?;
    Ok(())
}

fn validate_optional_package_path(path: Option<&str>, allow_directory: bool) -> Result<()> {
    if let Some(path) = path {
        validate_package_path(path, allow_directory)?;
    }
    Ok(())
}

fn validate_optional_package_paths<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    allow_directory: bool,
) -> Result<()> {
    for path in paths {
        validate_package_path(path, allow_directory)?;
    }
    Ok(())
}

fn validate_package_path(path: &str, allow_directory: bool) -> Result<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        bail!("plugin package path must not be empty");
    }
    if path.contains('\\') {
        bail!("plugin package path must use / separators");
    }
    if !allow_directory && path.ends_with('/') {
        bail!("plugin package file path must not end with /");
    }
    let relative_path = Path::new(path);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("plugin package path must stay inside the plugin directory");
    }
    Ok(relative_path.to_path_buf())
}

pub fn readme_locale_candidates(locale: &str) -> Vec<String> {
    let normalized = locale.trim().replace('_', "-");
    let mut candidates = Vec::new();
    push_unique_candidate(&mut candidates, &normalized);
    if let Some((language, _region)) = normalized.split_once('-') {
        push_unique_candidate(&mut candidates, language);
    }
    push_unique_candidate(&mut candidates, "en-US");
    candidates
}

fn push_unique_candidate(candidates: &mut Vec<String>, value: &str) {
    if value.is_empty() || candidates.iter().any(|candidate| candidate == value) {
        return;
    }
    candidates.push(value.to_string());
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;

    let digest = sha2::Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest(id: &str) -> String {
        format!(
            r#"
schema_version = 2
id = "{id}"
name = "Hello"
version = "0.1.0"
api_version = "{CURRENT_API_VERSION}"
entry = "plugin.wasm"
authors = ["BMCBL"]
capabilities = ["ui.page", "ui.window", "ui.hook", "event.global", "toast"]
"#
        )
    }

    #[test]
    fn manifest_parses_supported_capabilities() {
        let manifest = PluginManifest::parse("hello", &sample_manifest("hello-plugin"))
            .expect("manifest should parse");

        assert_eq!(manifest.id, "hello-plugin");
        assert_eq!(manifest.entry, "plugin.wasm");
        assert!(manifest.has_capability(&PluginCapability::UiPage));
        assert!(manifest.has_capability(&PluginCapability::UiWindow));
        assert!(manifest.has_capability(&PluginCapability::Toast));
    }

    #[test]
    fn manifest_rejects_invalid_id() {
        let error = PluginManifest::parse("hello", &sample_manifest("Hello_Plugin"))
            .expect_err("invalid id should fail");

        assert!(error.to_string().contains("lowercase ascii"));
    }

    #[test]
    fn manifest_rejects_unknown_capability() {
        let content = sample_manifest("hello-plugin").replace("\"toast\"", "\"filesystem\"");
        let error =
            PluginManifest::parse("hello", &content).expect_err("unknown capability should fail");

        assert!(error.to_string().contains("unsupported plugin capability"));
    }

    #[test]
    fn manifest_rejects_v03_schema() {
        let content =
            sample_manifest("hello-plugin").replace("schema_version = 2", "schema_version = 1");
        let error =
            PluginManifest::parse("hello", &content).expect_err("schema v1 should be rejected");

        assert!(
            error
                .to_string()
                .contains("unsupported plugin schema_version 1")
        );
    }

    #[test]
    fn manifest_parses_v2_permissions_and_limits() {
        let content = format!(
            r#"
schema_version = 2
id = "hello-plugin"
name = "Hello"
version = "0.1.0"
api_version = "{CURRENT_API_VERSION}"
entry = "plugin.wasm"
capabilities = ["network.http", "resource.read", "external.open", "storage.kv"]

[permissions.network]
allow = ["https://example.com/api/"]

[permissions.resource]
allow = ["assets/", "README.md"]

[permissions.external]
allow = ["https://example.com/docs/"]

[limits]
memory_mb = 256
max_http_bytes = 99999999
max_resource_bytes = 2048
max_storage_bytes = 4096
"#
        );

        let manifest = PluginManifest::parse("hello", &content)
            .expect("manifest with v2 permissions should parse");

        assert!(manifest.has_capability(&PluginCapability::StorageKv));
        assert!(manifest.allows_network_url("https://example.com/api/list"));
        assert!(manifest.allows_external_url("https://example.com/docs/page"));
        assert_eq!(
            manifest
                .resource_path("assets/item.txt")
                .expect("resource should be allowed"),
            PathBuf::from("hello").join("assets/item.txt")
        );
        assert_eq!(manifest.limits.memory_mb, HARD_MEMORY_LIMIT_MB);
        assert_eq!(manifest.limits.max_http_bytes, HARD_HTTP_LIMIT_BYTES);
        assert_eq!(manifest.limits.max_resource_bytes, 2048);
        assert_eq!(manifest.limits.max_storage_bytes, 4096);
    }

    #[test]
    fn manifest_rejects_resource_path_traversal() {
        let content = format!(
            r#"
schema_version = 2
id = "hello-plugin"
name = "Hello"
version = "0.1.0"
api_version = "{CURRENT_API_VERSION}"
entry = "plugin.wasm"
capabilities = ["resource.read"]

[permissions.resource]
allow = ["../secrets"]
"#
        );
        let error =
            PluginManifest::parse("hello", &content).expect_err("resource traversal should fail");

        drop(error);
    }

    #[test]
    fn manifest_requires_declared_capabilities() {
        let manifest = PluginManifest::parse("hello", &sample_manifest("hello-plugin"))
            .expect("manifest should parse");

        assert!(
            manifest
                .require_capability(PluginCapability::UiPage)
                .is_ok()
        );

        let limited = PluginManifest::parse(
            "hello",
            &sample_manifest("hello-plugin")
                .replace(r#", "ui.window", "ui.hook", "event.global", "toast""#, ""),
        )
        .expect("manifest should parse");
        let error = limited
            .require_capability(PluginCapability::UiWindow)
            .expect_err("missing capability should fail");
        assert!(error.to_string().contains("ui.window"));
    }

    #[test]
    fn manifest_parses_professional_metadata() {
        let content = format!(
            r#"
schema_version = 2
id = "bmcbl-essentials"
name = "BMCBL Essentials"
version = "0.1.0"
api_version = "{CURRENT_API_VERSION}"
entry = "plugin.wasm"
authors = ["BMCBL"]
description = "Core example plugin for BMCBL."
website = "https://bmcbl.com"
license = "GPL-3.0-only"
load_order = "startup"
tags = ["essentials", "example", "wasm"]
readme = "README.md"
readme_locales = {{ "en-US" = "README.md", "zh-CN" = "README.zh-CN.md" }}
package_hash = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
capabilities = ["ui.page", "ui.window", "ui.hook", "event.global", "toast"]
"#
        );

        let manifest = PluginManifest::parse("bmcbl-essentials", &content)
            .expect("professional metadata should parse");

        assert_eq!(manifest.website.as_deref(), Some("https://bmcbl.com"));
        assert_eq!(manifest.license.as_deref(), Some("GPL-3.0-only"));
        assert_eq!(manifest.load_order, PluginLoadOrder::Startup);
        assert_eq!(
            manifest.tags,
            vec![
                "essentials".to_string(),
                "example".to_string(),
                "wasm".to_string()
            ]
        );
        assert_eq!(
            manifest.package_hash.as_deref(),
            Some("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            manifest.readme_path_for_locale("zh-CN"),
            Some(PathBuf::from("bmcbl-essentials").join("README.zh-CN.md"))
        );
        assert_eq!(
            manifest.readme_path_for_locale("zh-HK"),
            Some(PathBuf::from("bmcbl-essentials").join("README.md"))
        );
    }

    #[test]
    fn readme_locale_candidates_follow_plugin_i18n_fallback_order() {
        assert_eq!(
            readme_locale_candidates("zh-CN"),
            vec!["zh-CN".to_string(), "zh".to_string(), "en-US".to_string()]
        );
        assert_eq!(
            readme_locale_candidates("en-US"),
            vec!["en-US".to_string(), "en".to_string()]
        );
    }

    #[test]
    fn bmcblx_package_is_installed_to_plugins_dir_and_removed() {
        let root = unique_temp_dir("bmcbl-plugin-package");
        let plugins_dir = root.join("plugins");
        let package_cache_dir = root.join("cache");
        std::fs::create_dir_all(&plugins_dir).expect("plugins dir should be created");

        let wasm = b"component bytes";
        let hash = sha256_hex(wasm);
        let manifest = format!(
            r#"
schema_version = 2
id = "bmcbl-essentials"
name = "BMCBL Essentials"
version = "0.1.0"
api_version = "{CURRENT_API_VERSION}"
entry = "plugin.wasm"
authors = ["BMCBL"]
description = "Core example plugin for BMCBL."
website = "https://bmcbl.com"
license = "GPL-3.0-only"
load_order = "startup"
tags = ["essentials", "example", "wasm"]
package_hash = "sha256:{hash}"
capabilities = ["ui.page", "ui.window", "ui.hook", "event.global", "toast"]
"#
        );
        let package_path = plugins_dir.join("bmcbl-essentials-0.1.0.bmcblx");
        write_test_package(&package_path, &manifest, wasm);

        let manifests = load_manifests_from_sources(&plugins_dir, &package_cache_dir)
            .expect("package should be loaded");

        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].id, "bmcbl-essentials");
        assert_eq!(manifests[0].root_dir, plugins_dir.join("bmcbl-essentials"));
        assert_eq!(
            std::fs::read(manifests[0].wasm_path()).expect("wasm should be unpacked"),
            wasm
        );
        assert!(
            !package_path.exists(),
            "installed package file should be removed"
        );
    }

    #[test]
    fn installed_package_update_preserves_user_config_and_marks_pending_backup() {
        let root = unique_temp_dir("bmcbl-plugin-package-update");
        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("plugins dir should be created");

        let old_wasm = b"old component";
        let old_hash = sha256_hex(old_wasm);
        let old_manifest = test_manifest("0.1.0", &old_hash);
        let old_package_path = plugins_dir.join("bmcbl-essentials-0.1.0.bmcblx");
        write_test_package_with_config(
            &old_package_path,
            &old_manifest,
            old_wasm,
            "enabled = true\n",
        );
        let old_manifest = install_manifest_from_package(&old_package_path, &plugins_dir)
            .expect("old package should install");
        commit_installed_package(&old_manifest).expect("old install should finalize");
        let user_config_path = old_manifest.user_config_path();
        std::fs::write(&user_config_path, "enabled = false\n")
            .expect("user config should be customized");

        let new_wasm = b"new component";
        let new_hash = sha256_hex(new_wasm);
        let new_manifest = test_manifest("0.2.0", &new_hash);
        let new_package_path = plugins_dir.join("bmcbl-essentials-0.2.0.bmcblx");
        write_test_package_with_config(
            &new_package_path,
            &new_manifest,
            new_wasm,
            "enabled = true\nnew_value = 7\n",
        );

        let updated_manifest = install_manifest_from_package(&new_package_path, &plugins_dir)
            .expect("new package should install");
        let config = std::fs::read_to_string(updated_manifest.user_config_path())
            .expect("merged config should read");

        assert!(config.contains("enabled = false"));
        assert!(config.contains("new_value = 7"));
        assert!(updated_manifest.pending_backup_dir.is_some());
        assert!(updated_manifest.root_dir.join(PENDING_BACKUP_FILE).exists());
        commit_installed_package(&updated_manifest).expect("new install should finalize");
        assert!(
            updated_manifest
                .pending_backup_dir
                .as_ref()
                .is_some_and(|path| !path.exists())
        );
        assert!(!updated_manifest.root_dir.join(PENDING_BACKUP_FILE).exists());
    }

    #[test]
    fn ensure_user_config_does_not_rewrite_when_defaults_are_already_present() {
        let root = unique_temp_dir("bmcbl-plugin-config-noop-merge");
        let plugin_dir = root.join("bmcbl-essentials");
        std::fs::create_dir_all(plugin_dir.join("config")).expect("plugin config dir should exist");
        std::fs::write(
            plugin_dir.join("config/default.toml"),
            "# default comment\nenabled = true\n",
        )
        .expect("default config should be written");
        let user_config_path = plugin_dir.join(PLUGIN_USER_CONFIG_FILE);
        std::fs::write(&user_config_path, "# user comment\nenabled = false\n")
            .expect("user config should be written");
        let before = std::fs::read_to_string(&user_config_path)
            .expect("user config should read before merge");
        let manifest_text = test_manifest("0.1.0", &sha256_hex(b"component"));
        let parsed_manifest = PluginManifest::parse("bmcbl-essentials", &manifest_text)
            .expect("test manifest should parse");
        let manifest = PluginManifest {
            root_dir: plugin_dir,
            config_default: Some("config/default.toml".to_string()),
            ..parsed_manifest
        };

        ensure_user_config(&manifest).expect("noop merge should succeed");

        let after = std::fs::read_to_string(&user_config_path)
            .expect("user config should read after merge");
        assert_eq!(before, after);
    }

    #[test]
    fn installed_package_update_can_roll_back_to_previous_directory() {
        let root = unique_temp_dir("bmcbl-plugin-package-rollback");
        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("plugins dir should be created");

        let old_wasm = b"old component";
        let old_hash = sha256_hex(old_wasm);
        let old_manifest = test_manifest("0.1.0", &old_hash);
        let old_package_path = plugins_dir.join("bmcbl-essentials-0.1.0.bmcblx");
        write_test_package(&old_package_path, &old_manifest, old_wasm);
        let old_manifest = install_manifest_from_package(&old_package_path, &plugins_dir)
            .expect("old package should install");
        commit_installed_package(&old_manifest).expect("old install should finalize");

        let new_wasm = b"new component";
        let new_hash = sha256_hex(new_wasm);
        let new_manifest = test_manifest("0.2.0", &new_hash);
        let new_package_path = plugins_dir.join("bmcbl-essentials-0.2.0.bmcblx");
        write_test_package(&new_package_path, &new_manifest, new_wasm);
        let updated_manifest = install_manifest_from_package(&new_package_path, &plugins_dir)
            .expect("new package should install");

        rollback_installed_package(&updated_manifest).expect("rollback should succeed");
        let restored = PluginManifest::load_from_dir(plugins_dir.join("bmcbl-essentials"))
            .expect("restored manifest should load");

        assert_eq!(restored.version, "0.1.0");
        assert_eq!(
            std::fs::read(restored.wasm_path()).expect("restored wasm should read"),
            old_wasm
        );
    }

    #[test]
    fn package_resources_are_unpacked_and_user_config_is_created() {
        let root = unique_temp_dir("bmcbl-plugin-resources");
        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("plugins dir should be created");

        let wasm = b"component bytes";
        let hash = sha256_hex(wasm);
        let manifest = test_manifest("0.1.0", &hash);
        let package_path = plugins_dir.join("bmcbl-essentials-0.1.0.bmcblx");
        write_test_package_with_resources(&package_path, &manifest, wasm);
        let manifest = install_manifest_from_package(&package_path, &plugins_dir)
            .expect("package should install");

        assert_eq!(
            std::fs::read_to_string(manifest.root_dir.join("README.md"))
                .expect("readme should read"),
            "# Essentials\n"
        );
        assert_eq!(
            std::fs::read_to_string(manifest.root_dir.join("README.zh-CN.md"))
                .expect("localized readme should read"),
            "# Essentials CN\n"
        );
        assert_eq!(
            std::fs::read_to_string(manifest.root_dir.join("lang/en-US.lang"))
                .expect("lang should read"),
            "hello=Hello\n"
        );
        assert_eq!(
            std::fs::read_to_string(manifest.user_config_path()).expect("user config should read"),
            "# default\nvalue = 1\n"
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    fn write_test_package(package_path: &Path, manifest: &str, wasm: &[u8]) {
        let file = std::fs::File::create(package_path).expect("package should be created");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(PLUGIN_MANIFEST_FILE, options)
            .expect("manifest entry should start");
        std::io::Write::write_all(&mut zip, manifest.as_bytes())
            .expect("manifest should be written");
        zip.start_file(DEFAULT_ENTRY, options)
            .expect("wasm entry should start");
        std::io::Write::write_all(&mut zip, wasm).expect("wasm should be written");
        zip.finish().expect("package should finish");
    }

    fn write_test_package_with_config(
        package_path: &Path,
        manifest: &str,
        wasm: &[u8],
        default_config: &str,
    ) {
        let file = std::fs::File::create(package_path).expect("package should be created");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(PLUGIN_MANIFEST_FILE, options)
            .expect("manifest entry should start");
        std::io::Write::write_all(&mut zip, manifest.as_bytes())
            .expect("manifest should be written");
        zip.start_file(DEFAULT_ENTRY, options)
            .expect("wasm entry should start");
        std::io::Write::write_all(&mut zip, wasm).expect("wasm should be written");
        zip.start_file("config/default.toml", options)
            .expect("default config entry should start");
        std::io::Write::write_all(&mut zip, default_config.as_bytes())
            .expect("default config should be written");
        zip.finish().expect("package should finish");
    }

    fn write_test_package_with_resources(package_path: &Path, manifest: &str, wasm: &[u8]) {
        let file = std::fs::File::create(package_path).expect("package should be created");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(PLUGIN_MANIFEST_FILE, options)
            .expect("manifest entry should start");
        std::io::Write::write_all(&mut zip, manifest.as_bytes())
            .expect("manifest should be written");
        zip.start_file(DEFAULT_ENTRY, options)
            .expect("wasm entry should start");
        std::io::Write::write_all(&mut zip, wasm).expect("wasm should be written");
        zip.start_file("README.md", options)
            .expect("readme entry should start");
        std::io::Write::write_all(&mut zip, b"# Essentials\n").expect("readme should be written");
        zip.start_file("README.zh-CN.md", options)
            .expect("localized readme entry should start");
        std::io::Write::write_all(&mut zip, b"# Essentials CN\n")
            .expect("localized readme should be written");
        zip.start_file("lang/en-US.lang", options)
            .expect("lang entry should start");
        std::io::Write::write_all(&mut zip, b"hello=Hello\n").expect("lang should be written");
        zip.start_file("config/default.toml", options)
            .expect("default config entry should start");
        std::io::Write::write_all(&mut zip, b"# default\nvalue = 1\n")
            .expect("default config should be written");
        zip.start_file("config/schema.toml", options)
            .expect("schema entry should start");
        std::io::Write::write_all(&mut zip, b"fields = []\n").expect("schema should be written");
        zip.finish().expect("package should finish");
    }

    fn test_manifest(version: &str, wasm_hash: &str) -> String {
        format!(
            r#"
schema_version = 2
id = "bmcbl-essentials"
name = "BMCBL Essentials"
version = "{version}"
api_version = "{CURRENT_API_VERSION}"
entry = "plugin.wasm"
authors = ["BMCBL"]
description = "Core example plugin for BMCBL."
website = "https://bmcbl.com"
license = "GPL-3.0-only"
load_order = "startup"
tags = ["essentials", "example", "wasm"]
readme = "README.md"
readme_locales = {{ "en-US" = "README.md", "zh-CN" = "README.zh-CN.md" }}
lang_dir = "lang"
config_default = "config/default.toml"
config_schema = "config/schema.toml"
package_hash = "sha256:{wasm_hash}"
capabilities = ["ui.page", "ui.window", "ui.hook", "event.global", "toast"]
"#
        )
    }
}
