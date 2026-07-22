use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type LocaleTable = Vec<(String, String)>;

const CIK_MIN_BYTES: usize = 0x30;
const RELEASE_CIK_GUID_BYTES_LE_HEX: &str = "91e7b9bd7cc93437e1a8bc602552df06";
const PREVIEW_CIK_GUID_BYTES_LE_HEX: &str = "3fd6491ff58b8d1fed7edbd89477dad9";
const UTF8_REPLACEMENT_BYTES: &[u8] = &[0xef, 0xbf, 0xbd];

fn resolve_build_version() -> String {
    let version = env::var("BMCBL_BUILD_VERSION")
        .ok()
        .filter(|version| !version.trim().is_empty())
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string()));
    let version = version.trim().trim_start_matches(['v', 'V']);
    assert!(
        !version.is_empty() && !version.contains('\r') && !version.contains('\n'),
        "BMCBL_BUILD_VERSION must be a non-empty single-line version"
    );
    version.to_string()
}

fn resolve_build_channel(build_version: &str) -> &'static str {
    match env::var("BMCBL_BUILD_CHANNEL") {
        Ok(channel) => match channel.trim().to_ascii_lowercase().as_str() {
            "stable" => "stable",
            "nightly" => "nightly",
            _ => panic!("BMCBL_BUILD_CHANNEL must be either stable or nightly"),
        },
        Err(env::VarError::NotPresent) => {
            if build_version
                .split_once('-')
                .is_some_and(|(_, prerelease)| prerelease.starts_with("nightly."))
            {
                "nightly"
            } else {
                "stable"
            }
        }
        Err(error) => panic!("failed to read BMCBL_BUILD_CHANNEL: {error}"),
    }
}

#[cfg(windows)]
fn compile_windows_resources() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());

    let icon_path = manifest_dir.join("assets").join("icons").join("icon.ico");
    let dpi_manifest_path = manifest_dir
        .join("vendor")
        .join("gpui")
        .join("resources")
        .join("windows")
        .join("gpui.manifest.xml");

    // 获取包信息
    let pkg_version = resolve_build_version();
    let pkg_description = env::var("CARGO_PKG_DESCRIPTION").unwrap_or_else(|_| String::new());
    let pkg_authors = env::var("CARGO_PKG_AUTHORS").unwrap_or_else(|_| String::new());

    // 解析版本号 (major.minor.patch.build)
    let version_core = pkg_version.split('-').next().unwrap_or(&pkg_version);
    let version_core = version_core.split('+').next().unwrap_or(version_core);
    let version_parts: Vec<&str> = version_core.split('.').collect();
    let file_version = if version_parts.len() >= 3 {
        format!(
            "{}.{}.{}.0",
            version_parts[0], version_parts[1], version_parts[2]
        )
    } else {
        format!("{}.0", version_core)
    };

    // 获取 Git 提交哈希
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8(out.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // 使用 winres 编译 Windows 资源（图标和版本信息）
    let mut res = winres::WindowsResource::new();

    // 设置图标
    if icon_path.exists() {
        res.set_icon(icon_path.to_str().unwrap());
    } else {
        println!("cargo:warning=Missing icon at {}", icon_path.display());
    }

    if dpi_manifest_path.exists() {
        println!("cargo:rerun-if-changed={}", dpi_manifest_path.display());
        let dpi_manifest_path = dpi_manifest_path.to_string_lossy().into_owned();
        res.set_manifest_file(&dpi_manifest_path);
    } else {
        println!(
            "cargo:warning=Missing Windows DPI manifest at {}",
            dpi_manifest_path.display()
        );
    }

    // 设置版本信息
    res.set("FileVersion", &file_version);
    res.set("ProductVersion", &pkg_version);

    // 设置应用程序信息
    res.set("FileDescription", &pkg_description);
    res.set("ProductName", "BMCBL");
    res.set("OriginalFilename", "BMCBL.exe");
    res.set("InternalName", "BMCBL");

    // 设置版权信息
    let authors = if pkg_authors.is_empty() {
        "Chlna6666".to_string()
    } else {
        pkg_authors.replace(':', ", ")
    };
    res.set("LegalCopyright", &format!("Copyright (C) 2026 {}", authors));
    res.set("LegalTrademarks", "");

    // 设置编译信息
    res.set("BuildNumber", &git_hash);
    res.set("Comments", &format!("Git: {}", git_hash));

    // 编译资源
    if let Err(e) = res.compile() {
        println!("cargo:warning=Failed to compile Windows resources: {}", e);
    }
}

#[cfg(windows)]
fn find_easytier_third_party_dir() -> Option<PathBuf> {
    let target = env::var("TARGET").unwrap_or_default();
    let arch_dir = if target.contains("x86_64") {
        "x86_64"
    } else if target.contains("aarch64") {
        "arm64"
    } else {
        return None;
    };

    fn has_runtime_assets(dir: &Path) -> bool {
        dir.join("wintun.dll").exists() || dir.join("WinDivert64.sys").exists()
    }

    // If EasyTier is vendored into this repo, prefer it.
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(manifest_dir);
        let repo_root = &manifest_dir;

        // Repo-root layout: `EasyTier/easytier/third_party/<arch>`
        let repo_third_party = repo_root
            .join("EasyTier")
            .join("easytier")
            .join("third_party")
            .join(arch_dir);
        if has_runtime_assets(&repo_third_party) {
            return Some(repo_third_party);
        }

        // Alternative layout: `easytier/third_party/<arch>` (vendored crate folder)
        let local_flat = manifest_dir
            .join("easytier")
            .join("third_party")
            .join(arch_dir);
        if has_runtime_assets(&local_flat) {
            return Some(local_flat);
        }
    }

    // Fallback: locate a cargo git checkout.
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".cargo")));
    let cargo_home = cargo_home?;

    let checkouts_dir = cargo_home.join("git").join("checkouts");
    let repos = fs::read_dir(&checkouts_dir).ok()?;

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for repo in repos.flatten() {
        let name = repo.file_name().to_string_lossy().to_string();
        if !name.starts_with("easytier-") {
            continue;
        }

        let revs = fs::read_dir(repo.path()).ok()?;
        for rev in revs.flatten() {
            let third_party = rev
                .path()
                .join("easytier")
                .join("third_party")
                .join(arch_dir);
            if !has_runtime_assets(&third_party) {
                continue;
            }

            let modified = third_party.metadata().and_then(|m| m.modified());
            if let Ok(modified) = modified {
                match best.as_ref() {
                    Some((best_modified, _)) if *best_modified >= modified => {}
                    _ => best = Some((modified, third_party)),
                }
            } else {
                best.get_or_insert((std::time::SystemTime::UNIX_EPOCH, third_party));
            }
        }
    }

    best.map(|(_, p)| p)
}

#[cfg(windows)]
fn generate_easytier_runtime_assets_rs() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let out_rs = out_dir.join("easytier_runtime_assets.rs");

    let third_party = find_easytier_third_party_dir();
    let mut wintun_present = false;
    let mut windivert_present = false;

    if let Some(third_party) = third_party.as_ref() {
        let wintun = third_party.join("wintun.dll");
        if wintun.exists() {
            println!("cargo:rerun-if-changed={}", wintun.display());
            let _ = fs::copy(&wintun, out_dir.join("wintun.dll"));
            wintun_present = true;
        }

        let windivert = third_party.join("WinDivert64.sys");
        if windivert.exists() {
            println!("cargo:rerun-if-changed={}", windivert.display());
            let _ = fs::copy(&windivert, out_dir.join("WinDivert64.sys"));
            windivert_present = true;
        }
    } else {
        println!(
            "cargo:warning=EasyTier third_party not found; wintun/windivert will not be embedded."
        );
    }

    let code = format!(
        r#"
// Auto-generated by build.rs. Do not edit.

pub const WINTUN_DLL: Option<&'static [u8]> = {wintun};
pub const WINDIVERT64_SYS: Option<&'static [u8]> = {windivert};
"#,
        wintun = if wintun_present {
            r#"Some(include_bytes!(concat!(env!("OUT_DIR"), "/wintun.dll")))"#
        } else {
            "None"
        },
        windivert = if windivert_present {
            r#"Some(include_bytes!(concat!(env!("OUT_DIR"), "/WinDivert64.sys")))"#
        } else {
            "None"
        }
    );

    fs::write(&out_rs, code).expect("write easytier_runtime_assets.rs");
}

/// 递归扫描目录，收集所有文件及其相对路径
fn collect_assets_recursive(dir: &Path, base_dir: &Path, result: &mut Vec<(String, PathBuf)>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_assets_recursive(&path, base_dir, result);
            } else if path.is_file() {
                if let Some(rel) = path.strip_prefix(base_dir).ok() {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    result.push((rel_str, path.clone()));
                }
            }
        }
    }
}

fn generate_embedded_assets_rs(
    assets_root: &Path,
    virtual_prefix: &str,
    out_file_name: &str,
    load_fn_name: &str,
    list_fn_name: &str,
) {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let out_rs = out_dir.join(out_file_name);
    let mut assets: Vec<(String, PathBuf)> = Vec::new();
    collect_assets_recursive(assets_root, assets_root, &mut assets);

    assets.sort_by(|left, right| left.0.cmp(&right.0));

    let mut match_arms = String::new();
    let mut list_items = String::new();

    for (rel_path, full_path) in &assets {
        let path_str = format!("{virtual_prefix}/{rel_path}");
        let escaped = full_path.to_string_lossy().replace('\\', "\\\\");

        match_arms.push_str(&format!(
            r#"        "{path_str}" => Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(r"{escaped}")))),"#
        ));
        match_arms.push('\n');

        list_items.push_str(&format!(r#"        "{path_str}".into(),"#));
        list_items.push('\n');
    }

    let code = format!(
        r#"// Auto-generated by build.rs. Do not edit.
// Generated at: {}
// Total assets: {}

pub fn {load_fn_name}(path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {{
    match path {{
{match_arms}        _ => Ok(None),
    }}
}}

pub fn {list_fn_name}() -> Vec<gpui::SharedString> {{
    vec![
{list_items}    ]
}}
"#,
        chrono::Utc::now().to_rfc3339(),
        assets.len()
    );

    fs::write(&out_rs, code).expect("write embedded assets file");
    println!("cargo:rerun-if-changed={}", assets_root.display());
}

/// 生成图片资源与图标资源的嵌入代码
fn generate_asset_bundles_rs() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let images_dir = manifest_dir.join("assets").join("images");
    let icons_dir = manifest_dir.join("assets").join("icons");

    generate_embedded_assets_rs(
        &images_dir,
        "images",
        "image_assets.rs",
        "load_image_asset",
        "list_image_assets",
    );
    generate_embedded_assets_rs(
        &icons_dir,
        "icons",
        "icon_assets.rs",
        "load_icon_asset",
        "list_icon_assets",
    );
}

fn escape_rust_string(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn load_locale_lang_table(path: &Path) -> LocaleTable {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read locale lang {}: {error}", path.display()));
    let mut out = Vec::new();
    let mut seen_keys = BTreeSet::new();

    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if !seen_keys.insert(key.to_string()) {
            panic!(
                "duplicate locale key in {} at line {}: {}",
                path.display(),
                line_index + 1,
                key
            );
        }

        out.push((key.to_string(), value.trim().to_string()));
    }

    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

fn load_locale_source(locales_dir: &Path, locale_code: &str) -> LocaleTable {
    let lang_path = locales_dir.join(format!("{locale_code}.lang"));
    println!("cargo:rerun-if-changed={}", lang_path.display());
    if !lang_path.exists() {
        panic!(
            "missing locale source {}; .lang is now required",
            lang_path.display()
        );
    }
    load_locale_lang_table(&lang_path)
}

fn validate_locale_tables(locale_tables: &[(String, LocaleTable)]) {
    let Some((base_locale, base_table)) = locale_tables.first() else {
        return;
    };
    let base_keys = base_table
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<BTreeSet<_>>();

    for (locale_code, table) in &locale_tables[1..] {
        let locale_keys = table
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<BTreeSet<_>>();
        let missing = base_keys
            .difference(&locale_keys)
            .cloned()
            .collect::<Vec<_>>();
        let extra = locale_keys
            .difference(&base_keys)
            .cloned()
            .collect::<Vec<_>>();

        if !missing.is_empty() || !extra.is_empty() {
            let missing_preview = missing
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let extra_preview = extra
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            panic!(
                "locale key mismatch: base={} target={} missing={} [{}] extra={} [{}]",
                base_locale,
                locale_code,
                missing.len(),
                missing_preview,
                extra.len(),
                extra_preview
            );
        }
    }
}

fn generate_i18n_tables_rs() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let locales_dir = manifest_dir.join("assets").join("locales");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let out_rs = out_dir.join("generated_locales.rs");

    let locale_codes = ["zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR"];
    let mut locale_tables = Vec::new();
    for locale_code in locale_codes {
        locale_tables.push((
            locale_code.to_string(),
            load_locale_source(&locales_dir, locale_code),
        ));
    }
    validate_locale_tables(&locale_tables);

    let mut static_defs = String::new();
    let mut match_arms = String::new();

    for (locale_code, table) in locale_tables {
        let static_name = locale_code.replace('-', "_").to_ascii_uppercase();
        static_defs.push_str(&format!("pub static {static_name}: &[(&str, &str)] = &[\n"));
        for (key, value) in table {
            static_defs.push_str(&format!(
                "    (\"{}\", \"{}\"),\n",
                escape_rust_string(&key),
                escape_rust_string(&value)
            ));
        }
        static_defs.push_str("];\n\n");
        match_arms.push_str(&format!("        \"{locale_code}\" => {static_name},\n"));
    }

    let generated = format!(
        "// Auto-generated by build.rs. Do not edit.\n\n{static_defs}pub fn locale_entries(locale: &str) -> &'static [(&'static str, &'static str)] {{\n    match locale {{\n{match_arms}        _ => EN_US,\n    }}\n}}\n"
    );

    fs::write(&out_rs, generated).expect("write generated_locales.rs");
}

#[derive(Clone, Debug)]
struct ManifestDependency {
    name: String,
    source_kind: String,
    source_url: String,
    path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

#[derive(Clone, Debug)]
struct DependencyMetadata {
    name: String,
    version: String,
    license: String,
    source_url: String,
    source_kind: String,
}

fn generate_dependency_metadata_rs() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let manifest_path = manifest_dir.join("Cargo.toml");
    let lock_path = manifest_dir.join("Cargo.lock");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let out_rs = out_dir.join("dependency_metadata.rs");

    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!("cargo:rerun-if-changed={}", lock_path.display());

    let dependencies = read_manifest_dependencies(&manifest_path, &manifest_dir);
    let lock_packages = read_lock_packages(&lock_path);
    let mut metadata = dependencies
        .into_iter()
        .map(|dependency| dependency_metadata(&dependency, &lock_packages))
        .collect::<Vec<_>>();
    metadata.sort_by_key(|item| item.name.to_ascii_lowercase());

    let mut entries = String::new();
    for item in metadata {
        entries.push_str(&format!(
            "    DependencyMetadata {{ name: \"{}\", version: \"{}\", license: \"{}\", source_url: \"{}\", source_kind: \"{}\" }},\n",
            escape_rust_string(&item.name),
            escape_rust_string(&item.version),
            escape_rust_string(&item.license),
            escape_rust_string(&item.source_url),
            escape_rust_string(&item.source_kind),
        ));
    }

    fs::write(&out_rs, format!("&[\n{entries}]")).expect("write dependency metadata");
}

fn read_manifest_dependencies(
    manifest_path: &Path,
    manifest_dir: &Path,
) -> Vec<ManifestDependency> {
    let content = fs::read_to_string(manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let value = toml::from_str::<toml::Value>(&content)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()));
    let mut dependencies = Vec::new();

    collect_manifest_dependencies(&value, "dependencies", manifest_dir, &mut dependencies);
    collect_manifest_dependencies(
        &value,
        "build-dependencies",
        manifest_dir,
        &mut dependencies,
    );
    collect_manifest_dependencies(&value, "dev-dependencies", manifest_dir, &mut dependencies);

    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            collect_manifest_dependencies(target, "dependencies", manifest_dir, &mut dependencies);
            collect_manifest_dependencies(
                target,
                "build-dependencies",
                manifest_dir,
                &mut dependencies,
            );
            collect_manifest_dependencies(
                target,
                "dev-dependencies",
                manifest_dir,
                &mut dependencies,
            );
        }
    }

    dependencies.sort_by_key(|item| item.name.to_ascii_lowercase());
    dependencies.dedup_by(|left, right| left.name.eq_ignore_ascii_case(&right.name));
    dependencies
}

fn collect_manifest_dependencies(
    value: &toml::Value,
    key: &str,
    manifest_dir: &Path,
    dependencies: &mut Vec<ManifestDependency>,
) {
    let Some(table) = value.get(key).and_then(toml::Value::as_table) else {
        return;
    };

    for (name, dependency_value) in table {
        dependencies.push(manifest_dependency(name, dependency_value, manifest_dir));
    }
}

fn manifest_dependency(name: &str, value: &toml::Value, manifest_dir: &Path) -> ManifestDependency {
    if let toml::Value::Table(table) = value {
        if let Some(git) = table.get("git").and_then(toml::Value::as_str) {
            return ManifestDependency {
                name: name.to_string(),
                source_kind: "git".to_string(),
                source_url: clean_git_url(git),
                path: None,
            };
        }

        if let Some(path) = table.get("path").and_then(toml::Value::as_str) {
            let path = manifest_dir.join(path);
            return ManifestDependency {
                name: name.to_string(),
                source_kind: "path".to_string(),
                source_url: String::new(),
                path: Some(path),
            };
        }
    }

    ManifestDependency {
        name: name.to_string(),
        source_kind: "registry".to_string(),
        source_url: format!("https://crates.io/crates/{name}"),
        path: None,
    }
}

fn read_lock_packages(lock_path: &Path) -> BTreeMap<String, LockPackage> {
    let Ok(content) = fs::read_to_string(lock_path) else {
        return BTreeMap::new();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return BTreeMap::new();
    };
    let Some(packages) = value.get("package").and_then(toml::Value::as_array) else {
        return BTreeMap::new();
    };

    let mut by_name = BTreeMap::new();
    for package in packages {
        let Some(table) = package.as_table() else {
            continue;
        };
        let Some(name) = table.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let version = table
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let source = table
            .get("source")
            .and_then(toml::Value::as_str)
            .map(str::to_string);

        by_name.entry(name.to_string()).or_insert(LockPackage {
            name: name.to_string(),
            version,
            source,
        });
    }

    by_name
}

fn dependency_metadata(
    dependency: &ManifestDependency,
    lock_packages: &BTreeMap<String, LockPackage>,
) -> DependencyMetadata {
    let lock_package = lock_packages.get(&dependency.name);
    let version = lock_package
        .map(|package| package.version.clone())
        .unwrap_or_default();
    let crate_manifest = dependency_manifest_path(dependency, lock_package);
    let (license, manifest_source_url) = crate_manifest
        .as_deref()
        .map(read_package_license_and_source)
        .unwrap_or_else(|| (String::new(), String::new()));

    let source_url = if !manifest_source_url.is_empty() {
        manifest_source_url
    } else if !dependency.source_url.is_empty() {
        dependency.source_url.clone()
    } else if dependency.source_kind == "registry" {
        format!("https://crates.io/crates/{}", dependency.name)
    } else {
        String::new()
    };

    DependencyMetadata {
        name: dependency.name.clone(),
        version,
        license,
        source_url,
        source_kind: dependency.source_kind.clone(),
    }
}

fn dependency_manifest_path(
    dependency: &ManifestDependency,
    lock_package: Option<&LockPackage>,
) -> Option<PathBuf> {
    if let Some(path) = dependency.path.as_ref() {
        return Some(path.join("Cargo.toml"));
    }

    let lock_package = lock_package?;
    let source = lock_package.source.as_deref()?;

    if source.starts_with("registry+") {
        return cargo_registry_manifest_path(&lock_package.name, &lock_package.version);
    }

    if source.starts_with("git+") {
        return cargo_git_manifest_path(&lock_package.name, source);
    }

    None
}

fn cargo_registry_manifest_path(name: &str, version: &str) -> Option<PathBuf> {
    let cargo_home = cargo_home()?;
    let registry_src = cargo_home.join("registry").join("src");
    let entries = fs::read_dir(registry_src).ok()?;

    for entry in entries.flatten() {
        let manifest = entry
            .path()
            .join(format!("{name}-{version}"))
            .join("Cargo.toml");
        if manifest.exists() {
            return Some(manifest);
        }
    }

    None
}

fn cargo_git_manifest_path(name: &str, source: &str) -> Option<PathBuf> {
    let cargo_home = cargo_home()?;
    let query = source.split('?').nth(1).unwrap_or_default();
    let revision = query
        .split('#')
        .nth(1)
        .filter(|value| !value.is_empty())
        .or_else(|| source.split('#').nth(1).filter(|value| !value.is_empty()));
    let checkouts = cargo_home.join("git").join("checkouts");
    let entries = fs::read_dir(checkouts).ok()?;

    for entry in entries.flatten() {
        let checkout_root = entry.path();
        if let Some(revision) = revision {
            let short_revision = &revision[..revision.len().min(7)];
            let manifest = checkout_root.join(short_revision).join("Cargo.toml");
            if manifest.exists() && manifest_package_name(&manifest).as_deref() == Some(name) {
                return Some(manifest);
            }
        }

        if let Ok(revisions) = fs::read_dir(checkout_root) {
            for revision_entry in revisions.flatten() {
                let manifest = revision_entry.path().join("Cargo.toml");
                if manifest.exists() && manifest_package_name(&manifest).as_deref() == Some(name) {
                    return Some(manifest);
                }
            }
        }
    }

    None
}

fn manifest_package_name(manifest: &Path) -> Option<String> {
    let content = fs::read_to_string(manifest).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

fn read_package_license_and_source(manifest: &Path) -> (String, String) {
    println!("cargo:rerun-if-changed={}", manifest.display());

    let Ok(content) = fs::read_to_string(manifest) else {
        return (String::new(), String::new());
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return (String::new(), String::new());
    };
    let Some(package) = value.get("package").and_then(toml::Value::as_table) else {
        return (String::new(), String::new());
    };

    let license = package
        .get("license")
        .and_then(toml::Value::as_str)
        .or_else(|| package.get("license-file").and_then(toml::Value::as_str))
        .unwrap_or_default()
        .to_string();
    let source_url = package
        .get("repository")
        .and_then(toml::Value::as_str)
        .or_else(|| package.get("homepage").and_then(toml::Value::as_str))
        .or_else(|| package.get("documentation").and_then(toml::Value::as_str))
        .map(clean_git_url)
        .unwrap_or_default();

    (license, source_url)
}

fn cargo_home() -> Option<PathBuf> {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|profile| PathBuf::from(profile).join(".cargo")))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
}

fn clean_git_url(url: &str) -> String {
    let without_prefix = url.strip_prefix("git+").unwrap_or(url);
    without_prefix
        .split(['?', '#'])
        .next()
        .unwrap_or(without_prefix)
        .to_string()
}

fn contains_cik_guid(bytes: &[u8], expected_guid_bytes: &[u8]) -> bool {
    bytes
        .windows(expected_guid_bytes.len())
        .position(|window| window == expected_guid_bytes)
        .map_or(false, |start_index| {
            start_index + CIK_MIN_BYTES <= bytes.len()
        })
}

fn contains_utf8_replacement_bytes(bytes: &[u8]) -> bool {
    bytes
        .windows(UTF8_REPLACEMENT_BYTES.len())
        .any(|window| window == UTF8_REPLACEMENT_BYTES)
}

fn validate_cik_bytes(
    source_name: &str,
    bytes: &[u8],
    expected_guid_bytes: &[u8],
) -> Result<(), String> {
    if bytes.len() < CIK_MIN_BYTES {
        return Err(format!(
            "{source_name} 长度不足: {} bytes，至少需要 {CIK_MIN_BYTES} bytes",
            bytes.len()
        ));
    }

    if contains_cik_guid(bytes, expected_guid_bytes) {
        return Ok(());
    }

    if contains_utf8_replacement_bytes(bytes) {
        return Err(format!(
            "{source_name} 包含 UTF-8 替换字节 EF BF BD，密钥文件可能被按文本保存而损坏"
        ));
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        let clean_hex = text
            .chars()
            .filter(|character| character.is_ascii_hexdigit())
            .collect::<String>();

        if clean_hex.len() >= CIK_MIN_BYTES * 2 {
            let even_length = clean_hex.len() - clean_hex.len() % 2;
            if let Ok(decoded_bytes) = hex::decode(&clean_hex[..even_length]) {
                if contains_cik_guid(&decoded_bytes, expected_guid_bytes) {
                    return Ok(());
                }
            }
        }
    }

    Err(format!("{source_name} 与预期 CIK GUID 不匹配"))
}

fn load_key_hex(
    env_name: &str,
    local_path: &Path,
    expected_guid_bytes_hex: &str,
) -> Result<String, String> {
    println!("cargo:rerun-if-env-changed={}", env_name);
    let expected_guid_bytes = hex::decode(expected_guid_bytes_hex)
        .map_err(|error| format!("内部 CIK GUID 常量无效: {error}"))?;

    if let Ok(env_hex) = env::var(env_name) {
        let env_hex = env_hex.trim();
        if env_hex.is_empty() {
            return Err(format!("{env_name} 已设置但为空"));
        }

        let env_bytes = hex::decode(env_hex)
            .map_err(|error| format!("{env_name} 不是有效的十六进制密钥: {error}"))?;
        validate_cik_bytes(env_name, &env_bytes, &expected_guid_bytes)?;
        return Ok(hex::encode(env_bytes));
    }

    println!("cargo:rerun-if-changed={}", local_path.display());
    let local_bytes = fs::read(local_path)
        .map_err(|error| format!("无法读取 {}: {error}", local_path.display()))?;

    validate_cik_bytes(
        &format!("本地密钥文件 {}", local_path.display()),
        &local_bytes,
        &expected_guid_bytes,
    )?;
    Ok(hex::encode(local_bytes))
}

fn main() {
    let build_version = resolve_build_version();
    let build_channel = resolve_build_channel(&build_version);
    println!("cargo:rerun-if-env-changed=BMCBL_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=BMCBL_BUILD_CHANNEL");
    println!("cargo:rustc-env=BMCBL_BUILD_VERSION={build_version}");
    println!("cargo:rustc-env=BMCBL_BUILD_CHANNEL={build_channel}");

    #[cfg(windows)]
    compile_windows_resources();

    #[cfg(windows)]
    generate_easytier_runtime_assets_rs();

    // 自动生成图片与图标资源嵌入代码
    generate_asset_bundles_rs();
    generate_i18n_tables_rs();
    generate_dependency_metadata_rs();

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("secrets.rs");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let key_dir = manifest_dir
        .join("src")
        .join("core")
        .join("minecraft")
        .join("gdk")
        .join("Cik");
    let release_key_path = key_dir.join("bdb9e791-c97c-3734-e1a8-bc602552df06.cik");
    let preview_key_path = key_dir.join("1f49d63f-8bf5-1f8d-ed7e-dbd89477dad9.cik");

    let release_code = load_key_hex(
        "GDK_RELEASE_KEY",
        &release_key_path,
        RELEASE_CIK_GUID_BYTES_LE_HEX,
    )
    .ok();
    let preview_code = load_key_hex(
        "GDK_PREVIEW_KEY",
        &preview_key_path,
        PREVIEW_CIK_GUID_BYTES_LE_HEX,
    )
    .ok();

    let secrets_content = format!(
        r#"
pub const RELEASE_KEY_HEX: Option<&'static str> = {};
pub const PREVIEW_KEY_HEX: Option<&'static str> = {};
"#,
        release_code.map_or("None".to_string(), |v| format!("Some(\"{v}\")")),
        preview_code.map_or("None".to_string(), |v| format!("Some(\"{v}\")")),
    );

    fs::write(&dest_path, secrets_content).expect("write secrets.rs");

    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let git_hash = String::from_utf8(out.stdout).unwrap();
            println!("cargo:rustc-env=GIT_COMMIT_HASH={}", git_hash.trim());
        }
        _ => {
            println!("cargo:rustc-env=GIT_COMMIT_HASH=unknown");
        }
    }

    let build_time = chrono::Utc::now().to_rfc3339();
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);
}
