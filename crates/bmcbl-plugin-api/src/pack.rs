use anyhow::{Context, Result, anyhow, bail};
use semver::Version;
use serde::Deserialize;
use sha2::Digest as _;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

pub const API_VERSION: &str = "0.4";
pub const PLUGIN_MANIFEST_FILE: &str = "plugin.toml";
pub const PLUGIN_WASM_FILE: &str = "plugin.wasm";
pub const PLUGIN_PACKAGE_EXTENSION: &str = "bmcblx";
const WASM_TARGET: &str = "wasm32-unknown-unknown";

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub manifest_path: PathBuf,
    pub release: bool,
    pub out_dir: Option<PathBuf>,
    pub run_cargo_build: bool,
    pub target_dir: Option<PathBuf>,
    pub wasm_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackResult {
    pub package_path: PathBuf,
    pub manifest_text: String,
    pub wasm_hash: String,
}

#[derive(Debug, Clone)]
pub struct AutoPackRequest {
    pub manifest_path: PathBuf,
    pub target_dir: PathBuf,
    pub wasm_path: PathBuf,
    pub release: bool,
    pub out_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CargoManifest {
    package: CargoPackage,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    metadata: CargoPackageMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackageMetadata {
    #[serde(rename = "bmcbl-plugin")]
    bmcbl_plugin: Option<BmcblPluginMetadata>,
}

#[derive(Debug, Deserialize)]
struct BmcblPluginMetadata {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    website: Option<String>,
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
    permissions: BmcblPluginPermissions,
    #[serde(default)]
    limits: BmcblPluginLimits,
}

#[derive(Debug, Default, Deserialize)]
struct BmcblPluginPermissions {
    #[serde(default)]
    network: BmcblPluginAllowList,
    #[serde(default)]
    resource: BmcblPluginAllowList,
    #[serde(default)]
    external: BmcblPluginAllowList,
}

#[derive(Debug, Default, Deserialize)]
struct BmcblPluginAllowList {
    #[serde(default)]
    allow: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BmcblPluginLimits {
    #[serde(default)]
    memory_mb: Option<u32>,
    #[serde(default)]
    max_http_bytes: Option<u64>,
    #[serde(default)]
    max_resource_bytes: Option<u64>,
    #[serde(default)]
    max_storage_bytes: Option<u64>,
}

pub fn pack_plugin(options: PackOptions) -> Result<PackResult> {
    let manifest_path = fs::canonicalize(&options.manifest_path)
        .with_context(|| format!("resolve {}", options.manifest_path.display()))?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("manifest path has no parent: {}", manifest_path.display()))?
        .to_path_buf();
    let cargo_manifest = read_cargo_manifest(&manifest_path)?;
    let metadata = cargo_manifest
        .package
        .metadata
        .bmcbl_plugin
        .as_ref()
        .ok_or_else(|| anyhow!("missing [package.metadata.bmcbl-plugin]"))?;

    validate_plugin_id(&metadata.id)?;
    let version = Version::parse(&cargo_manifest.package.version)
        .with_context(|| format!("invalid plugin version {}", cargo_manifest.package.version))?;
    if metadata.capabilities.is_empty() {
        bail!("metadata.bmcbl-plugin.capabilities must not be empty");
    }
    validate_optional_package_path(metadata.readme.as_deref(), false)?;
    validate_package_path_map(&metadata.readme_locales, false)?;
    validate_optional_package_path(metadata.icon.as_deref(), false)?;
    validate_optional_package_path(metadata.lang_dir.as_deref(), true)?;
    validate_optional_package_path(metadata.config_default.as_deref(), false)?;
    validate_optional_package_path(metadata.config_schema.as_deref(), false)?;
    validate_url_allowlist(
        "permissions.network.allow",
        &metadata.permissions.network.allow,
    )?;
    validate_url_allowlist(
        "permissions.external.allow",
        &metadata.permissions.external.allow,
    )?;
    validate_package_allowlist(
        "permissions.resource.allow",
        &metadata.permissions.resource.allow,
    )?;

    if options.run_cargo_build {
        build_wasm(
            &manifest_path,
            options.release,
            options.target_dir.as_deref(),
        )?;
    }

    let wasm_path = options.wasm_path.clone().unwrap_or_else(|| {
        compiled_wasm_path(
            &manifest_dir,
            &cargo_manifest.package.name,
            options.release,
            options.target_dir.as_deref(),
        )
    });
    let wasm = fs::read(&wasm_path).with_context(|| format!("read {}", wasm_path.display()))?;
    let wasm_hash = sha256_hex(&wasm);
    let manifest_text = generate_plugin_manifest(&cargo_manifest, metadata, &wasm_hash);
    let package_path = package_output_path(
        &manifest_dir,
        options.out_dir.as_deref(),
        &metadata.id,
        &version,
    )?;

    write_package(&package_path, &manifest_text, &wasm, &manifest_dir)?;
    sync_development_package_copy(&manifest_dir, &package_path, metadata)?;
    Ok(PackResult {
        package_path,
        manifest_text,
        wasm_hash,
    })
}

fn read_cargo_manifest(path: &Path) -> Result<CargoManifest> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn build_wasm(manifest_path: &Path, release: bool, target_dir: Option<&Path>) -> Result<()> {
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest_path)
        .arg("--target")
        .arg(WASM_TARGET);
    if release {
        command.arg("--release");
    }
    if let Some(target_dir) = target_dir {
        command.arg("--target-dir").arg(target_dir);
    }

    command
        .env("BMCBL_PLUGIN_SKIP_AUTO_PACK", "1")
        .env_remove("CARGO_MAKEFLAGS")
        .env("CARGO_TERM_PROGRESS_WHEN", "never");

    let status = command.status().context("run cargo build for plugin")?;
    if !status.success() {
        bail!("cargo build failed with status {status}");
    }
    Ok(())
}

fn compiled_wasm_path(
    manifest_dir: &Path,
    package_name: &str,
    release: bool,
    target_dir: Option<&Path>,
) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    let file_name = format!("{}.wasm", package_name.replace('-', "_"));
    target_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.join("target"))
        .join(WASM_TARGET)
        .join(profile)
        .join(file_name)
}

fn generate_plugin_manifest(
    cargo_manifest: &CargoManifest,
    metadata: &BmcblPluginMetadata,
    wasm_hash: &str,
) -> String {
    let package = &cargo_manifest.package;
    let name = metadata.name.as_deref().unwrap_or(&package.name);
    let description = package.description.as_deref().unwrap_or_default();
    let license = package.license.as_deref().unwrap_or_default();
    let website = metadata.website.as_deref().unwrap_or_default();

    [
        "schema_version = 2".to_string(),
        format!("id = {}", toml_string(&metadata.id)),
        format!("name = {}", toml_string(name)),
        format!("version = {}", toml_string(&package.version)),
        format!("api_version = {}", toml_string(API_VERSION)),
        format!("entry = {}", toml_string(PLUGIN_WASM_FILE)),
        format!("authors = {}", toml_array(&package.authors)),
        format!("description = {}", toml_string(description)),
        format!("website = {}", toml_string(website)),
        format!("license = {}", toml_string(license)),
        "load_order = \"startup\"".to_string(),
        format!("tags = {}", toml_array(&metadata.tags)),
        optional_manifest_path("readme", metadata.readme.as_deref()),
        optional_manifest_path_map("readme_locales", &metadata.readme_locales),
        optional_manifest_path("icon", metadata.icon.as_deref()),
        optional_manifest_path("lang_dir", metadata.lang_dir.as_deref()),
        optional_manifest_path("config_default", metadata.config_default.as_deref()),
        optional_manifest_path("config_schema", metadata.config_schema.as_deref()),
        format!(
            "package_hash = {}",
            toml_string(&format!("sha256:{wasm_hash}"))
        ),
        format!("capabilities = {}", toml_array(&metadata.capabilities)),
        manifest_allowlist_section("permissions.network", &metadata.permissions.network.allow),
        manifest_allowlist_section("permissions.resource", &metadata.permissions.resource.allow),
        manifest_allowlist_section("permissions.external", &metadata.permissions.external.allow),
        manifest_limits_section(&metadata.limits),
        String::new(),
    ]
    .join("\n")
}

fn package_output_path(
    manifest_dir: &Path,
    out_dir: Option<&Path>,
    plugin_id: &str,
    version: &Version,
) -> Result<PathBuf> {
    let out_dir = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_target_dir(manifest_dir).join("bmcbl-plugins"));
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    Ok(out_dir.join(format!("{plugin_id}-{version}.{PLUGIN_PACKAGE_EXTENSION}")))
}

fn workspace_target_dir(manifest_dir: &Path) -> PathBuf {
    let mut current = manifest_dir;
    while let Some(parent) = current.parent() {
        if parent.join("Cargo.toml").exists() && parent.join("crates").exists() {
            return parent.join("target");
        }
        current = parent;
    }
    manifest_dir.join("target")
}

fn sync_development_package_copy(
    manifest_dir: &Path,
    package_path: &Path,
    metadata: &BmcblPluginMetadata,
) -> Result<()> {
    if env::var_os("BMCBL_PLUGIN_SKIP_DEV_INSTALL").is_some() {
        return Ok(());
    }

    let target_dir = workspace_target_dir(manifest_dir);
    let plugins_dir = target_dir.join("debug").join("BMCBL").join("plugins");
    let Some(package_name) = package_path.file_name() else {
        return Ok(());
    };
    fs::create_dir_all(&plugins_dir)
        .with_context(|| format!("create {}", plugins_dir.display()))?;
    let destination = plugins_dir.join(package_name);
    if destination == package_path {
        return Ok(());
    }

    let temp_destination = destination.with_file_name(format!(
        "{}.tmp",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plugin.bmcblx")
    ));
    fs::copy(package_path, &temp_destination).with_context(|| {
        format!(
            "copy {} to {}",
            package_path.display(),
            temp_destination.display()
        )
    })?;
    if destination.exists() {
        fs::remove_file(&destination)
            .with_context(|| format!("replace {}", destination.display()))?;
    }
    fs::rename(&temp_destination, &destination).with_context(|| {
        format!(
            "move {} to {}",
            temp_destination.display(),
            destination.display()
        )
    })?;
    sync_development_installed_package_dir(&plugins_dir, &destination, metadata)?;
    Ok(())
}

fn sync_development_installed_package_dir(
    plugins_dir: &Path,
    package_path: &Path,
    metadata: &BmcblPluginMetadata,
) -> Result<()> {
    let install_dir = plugins_dir.join(&metadata.id);
    if !install_dir.exists() {
        return Ok(());
    }

    let installed_manifest = install_dir.join(PLUGIN_MANIFEST_FILE);
    if !installed_manifest.exists() {
        return Ok(());
    }

    let installed_text = fs::read_to_string(&installed_manifest)
        .with_context(|| format!("read {}", installed_manifest.display()))?;
    if !installed_text.contains("package_hash = ") {
        return Ok(());
    }

    install_package_to_development_dir(package_path, plugins_dir, &metadata.id)
}

fn install_package_to_development_dir(
    package_path: &Path,
    plugins_dir: &Path,
    plugin_id: &str,
) -> Result<()> {
    let file = fs::File::open(package_path)
        .with_context(|| format!("open plugin package {}", package_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("read plugin package {}", package_path.display()))?;
    let temp_dir = plugins_dir.join(format!(".bmcbl-plugin-dev-install-{plugin_id}"));
    let backup_dir = plugins_dir.join(format!(".bmcbl-plugin-dev-previous-{plugin_id}"));
    let install_dir = plugins_dir.join(plugin_id);

    remove_dir_if_exists(&temp_dir)?;
    remove_dir_if_exists(&backup_dir)?;
    fs::create_dir_all(&temp_dir).with_context(|| format!("create {}", temp_dir.display()))?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().replace('\\', "/");
        if name.is_empty() || name.contains("..") || name.starts_with('/') || name.starts_with('.')
        {
            continue;
        }

        let output_path = temp_dir.join(&name);
        if entry.is_dir() || name.ends_with('/') {
            fs::create_dir_all(&output_path)
                .with_context(|| format!("create {}", output_path.display()))?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut output = fs::File::create(&output_path)
            .with_context(|| format!("create {}", output_path.display()))?;
        std::io::copy(&mut entry, &mut output)
            .with_context(|| format!("extract {}", output_path.display()))?;
    }

    if install_dir.exists() {
        fs::rename(&install_dir, &backup_dir).with_context(|| {
            format!(
                "move existing plugin {} to {}",
                install_dir.display(),
                backup_dir.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(&temp_dir, &install_dir) {
        if backup_dir.exists() {
            fs::rename(&backup_dir, &install_dir)
                .with_context(|| format!("restore plugin install {}", install_dir.display()))?;
        }
        return Err(error).with_context(|| {
            format!(
                "move installed plugin {} to {}",
                temp_dir.display(),
                install_dir.display()
            )
        });
    }

    remove_dir_if_exists(&backup_dir)
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn write_package(
    package_path: &Path,
    manifest_text: &str,
    wasm: &[u8],
    plugin_dir: &Path,
) -> Result<()> {
    if let Some(parent) = package_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let temp_path = package_path.with_file_name(format!(
        "{}.tmp",
        package_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plugin.bmcblx")
    ));
    let file =
        fs::File::create(&temp_path).with_context(|| format!("create {}", temp_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(PLUGIN_MANIFEST_FILE, options)?;
    zip.write_all(manifest_text.as_bytes())?;
    zip.start_file(PLUGIN_WASM_FILE, options)?;
    zip.write_all(wasm)?;

    let mut written_files = BTreeSet::new();
    write_optional_file_from_manifest(
        plugin_dir,
        &mut zip,
        manifest_text,
        "readme",
        &mut written_files,
    )?;
    write_optional_file_map_from_manifest(
        plugin_dir,
        &mut zip,
        manifest_text,
        "readme_locales",
        &mut written_files,
    )?;
    write_optional_file_from_manifest(
        plugin_dir,
        &mut zip,
        manifest_text,
        "icon",
        &mut written_files,
    )?;
    write_optional_file_from_manifest(
        plugin_dir,
        &mut zip,
        manifest_text,
        "config_default",
        &mut written_files,
    )?;
    write_optional_file_from_manifest(
        plugin_dir,
        &mut zip,
        manifest_text,
        "config_schema",
        &mut written_files,
    )?;
    write_optional_directory_from_manifest(
        plugin_dir,
        &mut zip,
        manifest_text,
        "lang_dir",
        &mut written_files,
    )?;

    write_allowed_resource_entries(plugin_dir, &mut zip, &mut written_files, manifest_text)?;

    zip.finish()?;
    if package_path.exists() {
        fs::remove_file(package_path)
            .with_context(|| format!("replace {}", package_path.display()))?;
    }
    fs::rename(&temp_path, package_path)
        .with_context(|| format!("move {} to {}", temp_path.display(), package_path.display()))?;
    Ok(())
}

pub fn auto_pack_from_build_script() -> Result<()> {
    if env::var_os("BMCBL_PLUGIN_POST_PACK").is_some() {
        return run_post_pack_worker_from_env();
    }
    if env::var_os("BMCBL_PLUGIN_SKIP_AUTO_PACK").is_some() {
        return Ok(());
    }

    println!("cargo:rustc-env=BMCBL_PLUGIN_BUILD_SCRIPT_PACK=1");

    let target = env::var("TARGET").context("read TARGET")?;
    if target != WASM_TARGET {
        return Ok(());
    }

    let manifest_path = PathBuf::from(env::var_os("CARGO_MANIFEST_PATH").ok_or_else(|| {
        anyhow!("CARGO_MANIFEST_PATH is required for BMCBL plugin auto packaging")
    })?);
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        anyhow!("CARGO_MANIFEST_DIR is required for BMCBL plugin auto packaging")
    })?);
    let package_name = env::var("CARGO_PKG_NAME").context("read CARGO_PKG_NAME")?;
    let profile = env::var("PROFILE").context("read PROFILE")?;
    let release = profile == "release";
    emit_build_script_rerun_paths(&manifest_dir)?;
    emit_package_rerun_path(&manifest_path)?;

    let target_dir = env::var_os("BMCBL_PLUGIN_AUTO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_target_dir(&manifest_dir)
                .join("bmcbl-plugin-auto-build")
                .join(&package_name)
        });
    let result = pack_plugin(PackOptions {
        manifest_path: manifest_path.clone(),
        target_dir: Some(target_dir),
        wasm_path: None,
        release,
        out_dir: env::var_os("BMCBL_PLUGIN_PACKAGE_OUT_DIR").map(PathBuf::from),
        run_cargo_build: true,
    });
    write_auto_pack_log(&manifest_path, &result)?;
    result.map(|_| ())
}

pub fn spawn_post_pack_worker(request: AutoPackRequest) -> Result<()> {
    let manifest_dir = request.manifest_path.parent().ok_or_else(|| {
        anyhow!(
            "manifest path has no parent: {}",
            request.manifest_path.display()
        )
    })?;
    let initial_wasm_signature = file_signature(&request.wasm_path).ok();
    let require_fresh_wasm = source_requires_fresh_wasm(manifest_dir, &request.wasm_path);
    let mut command = post_pack_worker_command()?;
    command
        .env("BMCBL_PLUGIN_POST_PACK", "1")
        .env("BMCBL_PLUGIN_MANIFEST_PATH", &request.manifest_path)
        .env("BMCBL_PLUGIN_TARGET_DIR", &request.target_dir)
        .env("BMCBL_PLUGIN_WASM_PATH", &request.wasm_path)
        .env_remove("CARGO")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .env(
            "BMCBL_PLUGIN_RELEASE",
            if request.release { "1" } else { "0" },
        )
        .env(
            "BMCBL_PLUGIN_INITIAL_WASM_SIGNATURE",
            initial_wasm_signature.unwrap_or_default(),
        )
        .env(
            "BMCBL_PLUGIN_REQUIRE_FRESH_WASM",
            if require_fresh_wasm { "1" } else { "0" },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(out_dir) = request.out_dir {
        command.env("BMCBL_PLUGIN_PACKAGE_OUT_DIR", out_dir);
    }

    configure_post_pack_worker_command(&mut command);

    command
        .spawn()
        .context("spawn BMCBL plugin post-pack worker")?;
    Ok(())
}

fn configure_post_pack_worker_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

fn post_pack_worker_command() -> Result<Command> {
    if let Some(path) = env::var_os("BMCBL_PLUGIN_POST_PACK_EXE") {
        return Ok(Command::new(path));
    }
    let current_exe = env::current_exe()
        .context("resolve current executable for BMCBL plugin post-pack worker")?;
    if current_exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.starts_with("build-script"))
    {
        return Ok(Command::new(current_exe));
    }
    if let Some(path) = find_post_pack_worker_near_current_exe(&current_exe) {
        return Ok(Command::new(path));
    }

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .arg("--target-dir")
        .arg(
            workspace_target_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
                .join("bmcbl-plugin-pack-worker"),
        )
        .arg("--bin")
        .arg("bmcbl-plugin-post-pack");
    Ok(command)
}

pub fn run_post_pack_worker_from_env() -> Result<()> {
    let manifest_path = env_path("BMCBL_PLUGIN_MANIFEST_PATH")?;
    let target_dir = env_path("BMCBL_PLUGIN_TARGET_DIR")?;
    let wasm_path = env_path("BMCBL_PLUGIN_WASM_PATH")?;
    let dep_path = env::var_os("BMCBL_PLUGIN_DEP_PATH").map(PathBuf::from);
    let release = env::var("BMCBL_PLUGIN_RELEASE").unwrap_or_default() == "1";
    let out_dir = env::var_os("BMCBL_PLUGIN_PACKAGE_OUT_DIR").map(PathBuf::from);
    let initial_signature = env::var("BMCBL_PLUGIN_INITIAL_WASM_SIGNATURE").ok();
    let initial_dep_signature = env::var("BMCBL_PLUGIN_INITIAL_DEP_SIGNATURE").ok();
    let require_fresh_wasm = env::var("BMCBL_PLUGIN_REQUIRE_FRESH_WASM").unwrap_or_default() == "1";

    wait_for_wasm_artifact(
        &wasm_path,
        dep_path.as_deref(),
        initial_signature.as_deref(),
        initial_dep_signature.as_deref(),
        require_fresh_wasm,
    )?;
    let result = pack_plugin(PackOptions {
        manifest_path,
        release,
        out_dir,
        run_cargo_build: false,
        target_dir: Some(target_dir),
        wasm_path: Some(wasm_path),
    });
    write_post_pack_log(&result)?;
    result.map(|_| ())
}

fn find_post_pack_worker_near_current_exe(current_exe: &Path) -> Option<PathBuf> {
    let deps_dir = current_exe.parent()?;
    let target_dir = deps_dir.parent().unwrap_or(deps_dir);
    let candidates = [
        target_dir.join(exe_name("bmcbl-plugin-post-pack")),
        deps_dir.join(exe_name("bmcbl-plugin-post-pack")),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn exe_name(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

fn env_path(name: &str) -> Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("{name} is required for BMCBL plugin post-pack"))
}

fn wait_for_wasm_artifact(
    path: &Path,
    dep_path: Option<&Path>,
    initial_signature: Option<&str>,
    initial_dep_signature: Option<&str>,
    require_fresh_wasm: bool,
) -> Result<()> {
    let timeout = Duration::from_millis(
        env::var("BMCBL_PLUGIN_PACK_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(300_000),
    );
    let existing_artifact_after = Duration::from_millis(
        env::var("BMCBL_PLUGIN_PACK_EXISTING_AFTER_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(2_000),
    );
    let started_at = Instant::now();

    loop {
        if let Ok(signature) = file_signature(path) {
            let changed = initial_signature.is_none_or(|initial| initial != signature);
            let dep_changed = dep_path
                .and_then(|path| file_signature(path).ok())
                .is_some_and(|signature| {
                    initial_dep_signature.is_none_or(|initial| initial != signature)
                });
            if changed
                || dep_changed
                || (!require_fresh_wasm && started_at.elapsed() >= existing_artifact_after)
            {
                wait_until_stable(path)?;
                return Ok(());
            }

            if started_at.elapsed() >= existing_artifact_after && !source_newer_than_artifact(path)?
            {
                wait_until_stable(path)?;
                return Ok(());
            }

            if started_at.elapsed() >= timeout && path.exists() {
                wait_until_stable(path)?;
                return Ok(());
            }
        }

        if started_at.elapsed() >= timeout {
            bail!("timed out waiting for {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn source_newer_than_artifact(path: &Path) -> Result<bool> {
    let manifest_path = env_path("BMCBL_PLUGIN_MANIFEST_PATH")?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("manifest path has no parent: {}", manifest_path.display()))?;
    Ok(source_requires_fresh_wasm(manifest_dir, path))
}

fn emit_build_script_rerun_paths(manifest_dir: &Path) -> Result<()> {
    emit_rerun_if_exists(&manifest_dir.join("Cargo.toml"));
    emit_rerun_if_exists(&manifest_dir.join("build.rs"));
    emit_rerun_for_directory(&manifest_dir.join("src"))?;
    emit_rerun_for_directory(&manifest_dir.join("assets"))?;
    emit_rerun_for_directory(&manifest_dir.join("lang"))?;
    emit_rerun_for_directory(&manifest_dir.join("config"))?;
    emit_rerun_if_exists(&manifest_dir.join("README.md"));
    emit_rerun_for_readme_locales(&manifest_dir)?;
    Ok(())
}

fn emit_package_rerun_path(manifest_path: &Path) -> Result<()> {
    let manifest_path = fs::canonicalize(manifest_path)
        .with_context(|| format!("resolve {}", manifest_path.display()))?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("manifest path has no parent: {}", manifest_path.display()))?;
    let cargo_manifest = read_cargo_manifest(&manifest_path)?;
    let metadata = cargo_manifest
        .package
        .metadata
        .bmcbl_plugin
        .as_ref()
        .ok_or_else(|| anyhow!("missing [package.metadata.bmcbl-plugin]"))?;
    validate_plugin_id(&metadata.id)?;
    let version = Version::parse(&cargo_manifest.package.version)
        .with_context(|| format!("invalid plugin version {}", cargo_manifest.package.version))?;
    let package_path = package_output_path(
        manifest_dir,
        env::var_os("BMCBL_PLUGIN_PACKAGE_OUT_DIR")
            .map(PathBuf::from)
            .as_deref(),
        &metadata.id,
        &version,
    )?;
    println!("cargo:rerun-if-changed={}", package_path.display());
    Ok(())
}

fn emit_rerun_for_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut entries = fs::read_dir(path)
        .with_context(|| format!("read {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            emit_rerun_for_directory(&path)?;
        } else {
            emit_rerun_if_exists(&path);
        }
    }
    Ok(())
}

fn emit_rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn emit_rerun_for_readme_locales(manifest_dir: &Path) -> Result<()> {
    let mut entries = fs::read_dir(manifest_dir)
        .with_context(|| format!("read {}", manifest_dir.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.starts_with("README.") && file_name.ends_with(".md") {
            emit_rerun_if_exists(&path);
        }
    }
    Ok(())
}

fn source_requires_fresh_wasm(manifest_dir: &Path, wasm_path: &Path) -> bool {
    let Ok(wasm_modified) = fs::metadata(wasm_path).and_then(|metadata| metadata.modified()) else {
        return true;
    };
    let source_paths = [
        manifest_dir.join("Cargo.toml"),
        manifest_dir.join("build.rs"),
        manifest_dir.join("src"),
    ];
    source_paths
        .iter()
        .any(|path| path_modified_after(path, wasm_modified).unwrap_or(false))
}

fn path_modified_after(path: &Path, baseline: SystemTime) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if path.is_dir() {
        for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
            let entry = entry?;
            if path_modified_after(&entry.path(), baseline)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    let modified = fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .modified()
        .with_context(|| format!("read modified time for {}", path.display()))?;
    Ok(modified > baseline)
}

fn wait_until_stable(path: &Path) -> Result<()> {
    let timeout = Duration::from_secs(10);
    let started_at = Instant::now();
    let mut previous = file_signature(path)?;

    loop {
        std::thread::sleep(Duration::from_millis(300));
        let current = file_signature(path)?;
        if current == previous {
            return Ok(());
        }
        if started_at.elapsed() >= timeout {
            bail!("{} did not become stable", path.display());
        }
        previous = current;
    }
}

fn file_signature(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let modified = metadata
        .modified()
        .with_context(|| format!("read modified time for {}", path.display()))?
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(format!("{}:{modified}", metadata.len()))
}

fn write_post_pack_log(result: &Result<PackResult>) -> Result<()> {
    let manifest_path = env_path("BMCBL_PLUGIN_MANIFEST_PATH")?;
    write_auto_pack_log(&manifest_path, result)
}

fn write_auto_pack_log(manifest_path: &Path, result: &Result<PackResult>) -> Result<()> {
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("manifest path has no parent: {}", manifest_path.display()))?;
    let log_dir = workspace_target_dir(manifest_dir).join("bmcbl-plugins");
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;
    let log_path = log_dir.join("auto-pack.log");
    let line = match result {
        Ok(result) => format!("packed {}\n", result.package_path.display()),
        Err(error) => format!("failed: {error:#}\n"),
    };
    let max_bytes = 32 * 1024;
    let mut existing = if log_path.exists() {
        fs::read_to_string(&log_path).unwrap_or_default()
    } else {
        String::new()
    };
    existing.push_str(&line);
    if existing.len() > max_bytes {
        let keep_from = existing.len().saturating_sub(max_bytes);
        existing = existing[keep_from..].to_string();
    }
    fs::write(&log_path, existing).with_context(|| format!("write {}", log_path.display()))
}

fn write_allowed_resource_entries<W: Write + std::io::Seek>(
    plugin_dir: &Path,
    zip: &mut zip::ZipWriter<W>,
    written_files: &mut BTreeSet<String>,
    manifest_text: &str,
) -> Result<()> {
    let manifest: toml::Value =
        toml::from_str(manifest_text).context("parse generated manifest")?;
    let allowed = manifest
        .get("permissions")
        .and_then(|permissions| permissions.get("resource"))
        .and_then(|resource| resource.get("allow"))
        .and_then(toml::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for entry in allowed {
        let normalized = normalize_package_path(&entry);
        let allow_directory = normalized.ends_with('/');
        let relative_path = validate_package_path(Path::new(&normalized), allow_directory)
            .with_context(|| format!("validate resource allowlist entry {normalized}"))?;
        let path = plugin_dir.join(&relative_path);
        if allow_directory {
            if path.exists() {
                write_directory(zip, plugin_dir, &path, written_files)?;
            }
        } else if path.exists() {
            let normalized = normalize_package_path(&relative_path.to_string_lossy());
            if written_files.insert(normalized) {
                write_package_file(zip, plugin_dir, &relative_path, &relative_path)?;
            }
        }
    }
    Ok(())
}

fn write_optional_file_from_manifest<W: Write + std::io::Seek>(
    plugin_dir: &Path,
    zip: &mut zip::ZipWriter<W>,
    manifest_text: &str,
    key: &str,
    written_files: &mut BTreeSet<String>,
) -> Result<()> {
    let Some(relative_path) = manifest_path_value(manifest_text, key)? else {
        return Ok(());
    };
    if !written_files.insert(normalize_package_path(&relative_path.to_string_lossy())) {
        return Ok(());
    }
    write_package_file(zip, plugin_dir, &relative_path, &relative_path)
}

fn write_optional_file_map_from_manifest<W: Write + std::io::Seek>(
    plugin_dir: &Path,
    zip: &mut zip::ZipWriter<W>,
    manifest_text: &str,
    key: &str,
    written_files: &mut BTreeSet<String>,
) -> Result<()> {
    for relative_path in manifest_path_map_values(manifest_text, key)? {
        if !written_files.insert(normalize_package_path(&relative_path.to_string_lossy())) {
            continue;
        }
        write_package_file(zip, plugin_dir, &relative_path, &relative_path)?;
    }
    Ok(())
}

fn write_optional_directory_from_manifest<W: Write + std::io::Seek>(
    plugin_dir: &Path,
    zip: &mut zip::ZipWriter<W>,
    manifest_text: &str,
    key: &str,
    written_files: &mut BTreeSet<String>,
) -> Result<()> {
    let Some(relative_path) = manifest_path_value(manifest_text, key)? else {
        return Ok(());
    };
    let root = plugin_dir.join(&relative_path);
    if !root.exists() {
        return Ok(());
    }
    write_directory(zip, plugin_dir, &root, written_files)
}

fn manifest_path_value(manifest_text: &str, key: &str) -> Result<Option<PathBuf>> {
    let value: toml::Value = toml::from_str(manifest_text).context("parse generated manifest")?;
    Ok(value
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from))
}

fn manifest_path_map_values(manifest_text: &str, key: &str) -> Result<Vec<PathBuf>> {
    let value: toml::Value = toml::from_str(manifest_text).context("parse generated manifest")?;
    let Some(table) = value.get(key).and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };

    let mut paths = Vec::with_capacity(table.len());
    for path in table.values().filter_map(toml::Value::as_str) {
        if !path.trim().is_empty() {
            paths.push(PathBuf::from(path));
        }
    }
    Ok(paths)
}

fn write_directory<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    plugin_dir: &Path,
    current_dir: &Path,
    written_files: &mut BTreeSet<String>,
) -> Result<()> {
    let mut entries = fs::read_dir(current_dir)
        .with_context(|| format!("read package directory {}", current_dir.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            write_directory(zip, plugin_dir, &path, written_files)?;
            continue;
        }
        let relative_path = path
            .strip_prefix(plugin_dir)
            .with_context(|| format!("strip plugin prefix {}", path.display()))?;
        if !written_files.insert(normalize_package_path(&relative_path.to_string_lossy())) {
            continue;
        }
        write_package_file(zip, plugin_dir, relative_path, relative_path)?;
    }
    Ok(())
}

fn write_package_file<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    plugin_dir: &Path,
    source_relative: &Path,
    package_relative: &Path,
) -> Result<()> {
    let source_relative = validate_package_path(source_relative, false)?;
    let package_relative = validate_package_path(package_relative, false)?;
    let source = plugin_dir.join(source_relative);
    if !source.exists() {
        return Ok(());
    }

    let mut bytes = Vec::new();
    fs::File::open(&source)
        .with_context(|| format!("open {}", source.display()))?
        .read_to_end(&mut bytes)?;
    let zip_path = package_relative.to_string_lossy().replace('\\', "/");
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(zip_path, options)?;
    zip.write_all(&bytes)?;
    Ok(())
}

fn validate_plugin_id(id: &str) -> Result<()> {
    if !(3..=64).contains(&id.len()) {
        bail!("plugin id must be 3..=64 bytes");
    }
    let mut previous_dash = false;
    for (index, byte) in id.bytes().enumerate() {
        if !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
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

fn optional_manifest_path(key: &str, value: Option<&str>) -> String {
    value.map_or_else(String::new, |value| {
        format!("{key} = {}", toml_string(&normalize_package_path(value)))
    })
}

fn optional_manifest_path_map(key: &str, values: &BTreeMap<String, String>) -> String {
    if values.is_empty() {
        return String::new();
    }

    let entries = values
        .iter()
        .map(|(locale, path)| {
            format!(
                "{} = {}",
                toml_string(locale),
                toml_string(&normalize_package_path(path))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{key} = {{ {entries} }}")
}

fn manifest_allowlist_section(section: &str, values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    format!("[{section}]\nallow = {}", toml_array(values))
}

fn manifest_limits_section(limits: &BmcblPluginLimits) -> String {
    let mut lines = Vec::new();
    if let Some(value) = limits.memory_mb {
        lines.push(format!("memory_mb = {value}"));
    }
    if let Some(value) = limits.max_http_bytes {
        lines.push(format!("max_http_bytes = {value}"));
    }
    if let Some(value) = limits.max_resource_bytes {
        lines.push(format!("max_resource_bytes = {value}"));
    }
    if let Some(value) = limits.max_storage_bytes {
        lines.push(format!("max_storage_bytes = {value}"));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("[limits]\n{}", lines.join("\n"))
    }
}

fn validate_optional_package_path(path: Option<&str>, allow_directory: bool) -> Result<()> {
    if let Some(path) = path {
        validate_package_path(Path::new(path), allow_directory)?;
    }
    Ok(())
}

fn validate_package_path_map(
    paths: &BTreeMap<String, String>,
    allow_directory: bool,
) -> Result<()> {
    for (locale, path) in paths {
        if locale.trim().is_empty() {
            bail!("plugin locale keys must not be empty");
        }
        validate_package_path(Path::new(path), allow_directory)
            .with_context(|| format!("validate plugin package path for locale {locale}"))?;
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
        validate_package_path(Path::new(path), true)
            .with_context(|| format!("validate plugin package path for {label}"))?;
    }
    Ok(())
}

fn validate_package_path(path: &Path, allow_directory: bool) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("plugin package path must not be empty");
    }
    if path.is_absolute() {
        bail!("plugin package path must be relative");
    }
    if path
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("plugin package path must stay inside the plugin directory");
    }
    let normalized = normalize_package_path(&path.to_string_lossy());
    if !allow_directory && normalized.ends_with('/') {
        bail!("plugin package file path must not end with /");
    }
    Ok(PathBuf::from(normalized))
}

fn normalize_package_path(path: &str) -> String {
    path.trim().replace('\\', "/")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn toml_array(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| toml_string(value))
        .collect::<Vec<_>>();
    format!("[{}]", values.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_manifest_contains_professional_metadata_and_hash() {
        let cargo_manifest = CargoManifest {
            package: CargoPackage {
                name: "hello-wasm".to_string(),
                version: "0.1.0".to_string(),
                authors: vec!["BMCBL".to_string()],
                description: Some("Core example plugin for BMCBL.".to_string()),
                license: Some("GPL-3.0-only".to_string()),
                metadata: CargoPackageMetadata {
                    bmcbl_plugin: Some(BmcblPluginMetadata {
                        id: "bmcbl-essentials".to_string(),
                        name: Some("BMCBL Essentials".to_string()),
                        capabilities: vec!["ui.page".to_string(), "toast".to_string()],
                        website: Some("https://bmcbl.com".to_string()),
                        tags: vec!["essentials".to_string(), "wasm".to_string()],
                        readme: Some("README.md".to_string()),
                        readme_locales: BTreeMap::from([
                            ("en-US".to_string(), "README.md".to_string()),
                            ("zh-CN".to_string(), "README.zh-CN.md".to_string()),
                        ]),
                        icon: Some("assets/icon.svg".to_string()),
                        lang_dir: Some("lang".to_string()),
                        config_default: Some("config/default.toml".to_string()),
                        config_schema: Some("config/schema.toml".to_string()),
                        permissions: BmcblPluginPermissions {
                            network: BmcblPluginAllowList {
                                allow: vec![
                                    "https://launchercontent.mojang.com/v2/bedrockPatchNotes.json"
                                        .to_string(),
                                ],
                            },
                            resource: BmcblPluginAllowList {
                                allow: vec!["assets/".to_string()],
                            },
                            external: BmcblPluginAllowList::default(),
                        },
                        limits: BmcblPluginLimits {
                            memory_mb: Some(32),
                            max_http_bytes: Some(524288),
                            max_resource_bytes: Some(4096),
                            max_storage_bytes: None,
                        },
                    }),
                },
            },
        };
        let metadata = cargo_manifest
            .package
            .metadata
            .bmcbl_plugin
            .as_ref()
            .expect("metadata should exist");

        let manifest = generate_plugin_manifest(&cargo_manifest, metadata, "abc123");

        assert!(manifest.contains("id = \"bmcbl-essentials\""));
        assert!(manifest.contains("name = \"BMCBL Essentials\""));
        assert!(manifest.contains("entry = \"plugin.wasm\""));
        assert!(manifest.contains("load_order = \"startup\""));
        assert!(manifest.contains("readme = \"README.md\""));
        assert!(manifest.contains(
            "readme_locales = { \"en-US\" = \"README.md\", \"zh-CN\" = \"README.zh-CN.md\" }"
        ));
        assert!(manifest.contains("icon = \"assets/icon.svg\""));
        assert!(manifest.contains("lang_dir = \"lang\""));
        assert!(manifest.contains("config_default = \"config/default.toml\""));
        assert!(manifest.contains("config_schema = \"config/schema.toml\""));
        assert!(manifest.contains("[permissions.network]"));
        assert!(manifest.contains(
            "allow = [\"https://launchercontent.mojang.com/v2/bedrockPatchNotes.json\"]"
        ));
        assert!(manifest.contains("[permissions.resource]"));
        assert!(manifest.contains("allow = [\"assets/\"]"));
        assert!(manifest.contains("[limits]"));
        assert!(manifest.contains("memory_mb = 32"));
        assert!(manifest.contains("max_http_bytes = 524288"));
        assert!(manifest.contains("package_hash = \"sha256:abc123\""));
    }

    #[test]
    fn package_contains_manifest_wasm_assets_and_declared_resources() {
        let root = unique_temp_dir("bmcbl-plugin-tools-package");
        let plugin_dir = root.join("plugin");
        let assets_dir = plugin_dir.join("assets");
        let lang_dir = plugin_dir.join("lang");
        let config_dir = plugin_dir.join("config");
        fs::create_dir_all(&assets_dir).expect("assets dir should be created");
        fs::create_dir_all(&lang_dir).expect("lang dir should be created");
        fs::create_dir_all(&config_dir).expect("config dir should be created");
        fs::write(assets_dir.join("readme.txt"), b"asset").expect("asset should be written");
        fs::write(assets_dir.join("icon.svg"), b"<svg/>").expect("icon should be written");
        fs::write(plugin_dir.join("README.md"), b"readme").expect("readme should be written");
        fs::write(plugin_dir.join("README.zh-CN.md"), b"zh readme")
            .expect("localized readme should be written");
        fs::write(lang_dir.join("en-US.lang"), b"hello=Hello").expect("lang should be written");
        fs::write(config_dir.join("default.toml"), b"enabled = true")
            .expect("default config should be written");
        fs::write(config_dir.join("schema.toml"), b"fields = []")
            .expect("config schema should be written");
        let package_path = root.join("out.bmcblx");
        let manifest = r#"
schema_version = 2
readme = "README.md"
readme_locales = { "en-US" = "README.md", "zh-CN" = "README.zh-CN.md" }
lang_dir = "lang"
config_default = "config/default.toml"
config_schema = "config/schema.toml"
icon = "assets/icon.svg"

[permissions.resource]
allow = ["assets/"]
"#;

        write_package(&package_path, manifest, b"wasm", &plugin_dir)
            .expect("package should be written");

        let file = fs::File::open(package_path).expect("package should open");
        let mut zip = zip::ZipArchive::new(file).expect("package should be zip");
        assert_eq!(
            read_zip_entry(&mut zip, PLUGIN_MANIFEST_FILE),
            manifest.as_bytes()
        );
        assert_eq!(read_zip_entry(&mut zip, PLUGIN_WASM_FILE), b"wasm");
        assert_eq!(read_zip_entry(&mut zip, "assets/readme.txt"), b"asset");
        assert_eq!(read_zip_entry(&mut zip, "assets/icon.svg"), b"<svg/>");
        assert_eq!(read_zip_entry(&mut zip, "README.md"), b"readme");
        assert_eq!(read_zip_entry(&mut zip, "README.zh-CN.md"), b"zh readme");
        assert_eq!(read_zip_entry(&mut zip, "lang/en-US.lang"), b"hello=Hello");
        assert_eq!(
            read_zip_entry(&mut zip, "config/default.toml"),
            b"enabled = true"
        );
        assert_eq!(
            read_zip_entry(&mut zip, "config/schema.toml"),
            b"fields = []"
        );
    }

    fn read_zip_entry<R: Read + std::io::Seek>(
        zip: &mut zip::ZipArchive<R>,
        name: &str,
    ) -> Vec<u8> {
        let mut entry = zip.by_name(name).expect("zip entry should exist");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("entry should read");
        bytes
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }
}
