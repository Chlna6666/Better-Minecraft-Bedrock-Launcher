use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
fn cargo_target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR")?);
    let profile = env::var("PROFILE").ok()?;

    // OUT_DIR: target/<profile>/build/<crate>/out
    // We want: target/<profile>
    let mut cursor: &Path = out_dir.as_path();
    while let Some(parent) = cursor.parent() {
        if parent.ends_with(&profile) {
            return Some(parent.to_path_buf());
        }
        cursor = parent;
    }

    None
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

    // If EasyTier is vendored into this repo at `src-tauri/easytier/`, prefer it.
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(manifest_dir);

        // Repo-root layout (requested by this repo): `EasyTier/easytier/third_party/<arch>`
        // CARGO_MANIFEST_DIR points to `src-tauri/`, so `..` is repo root.
        let repo_root = manifest_dir.parent().unwrap_or(&manifest_dir);
        let repo_third_party = repo_root
            .join("EasyTier")
            .join("easytier")
            .join("third_party")
            .join(arch_dir);
        if has_runtime_assets(&repo_third_party) {
            return Some(repo_third_party);
        }

        for vendor_dir in ["EasyTier-BMCBL", "EasyTier-bmcb-nopacket", "EasyTier-v2.5.0"] {
            // Vendoring layout: `src-tauri/easytier/<vendor>/easytier/third_party/<arch>`
            let local_vendored = manifest_dir
                .join("easytier")
                .join(vendor_dir)
                .join("easytier")
                .join("third_party")
                .join(arch_dir);
            if has_runtime_assets(&local_vendored) {
                return Some(local_vendored);
            }
        }

        // Back-compat: `src-tauri/easytier/third_party/<arch>`
        let local_flat = manifest_dir.join("easytier").join("third_party").join(arch_dir);
        if has_runtime_assets(&local_flat) {
            return Some(local_flat);
        }
    }

    // Fallback for non-vendored setups: locate a cargo git checkout.
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".cargo")));
    let Some(cargo_home) = cargo_home else {
        return None;
    };

    let checkouts_dir = cargo_home.join("git").join("checkouts");
    let Ok(repos) = fs::read_dir(&checkouts_dir) else {
        return None;
    };

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for repo in repos.flatten() {
        let name = repo.file_name().to_string_lossy().to_string();
        if !name.starts_with("easytier-") {
            continue;
        }

        let Ok(revs) = fs::read_dir(repo.path()) else {
            continue;
        };

        for rev in revs.flatten() {
            let third_party = rev
                .path()
                .join("easytier")
                .join("third_party")
                .join(arch_dir);
            if !has_runtime_assets(&third_party) {
                continue;
            }

            // Prefer the most recently modified checkout (helps if multiple EasyTier versions exist).
            let modified = third_party
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            match &best {
                Some((best_time, _)) if *best_time >= modified => {}
                _ => best = Some((modified, third_party)),
            }
        }
    }

    best.map(|(_, p)| p)
}

#[cfg(windows)]
fn add_easytier_third_party_link_search_and_copy_runtime() {
    let Some(third_party) = find_easytier_third_party_dir() else {
        println!(
            "cargo:warning=EasyTier third_party not found; wintun/windivert runtime files will not be copied. If EasyTier is vendored, ensure src-tauri/easytier/**/easytier/third_party/<arch> exists."
        );
        return;
    };

    // Ensure runtime DLLs are next to the final exe. Otherwise running the built binary can fail
    // with STATUS_DLL_NOT_FOUND (0xc0000135), even though linking succeeded.
    let Some(target_profile_dir) = cargo_target_profile_dir() else {
        return;
    };

    let files_to_copy = ["wintun.dll", "WinDivert64.sys"];
    for name in files_to_copy {
        let src = third_party.join(name);
        if !src.exists() {
            continue;
        }

        let dst = target_profile_dir.join(name);
        // Best-effort copy; don't hard fail if the file is locked by a running process.
        let _ = fs::copy(&src, &dst);
    }
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

    fs::write(&out_rs, code).expect("Failed to write easytier_runtime_assets.rs");
}

fn main() {
    // 1. Tauri 构建配置
    let mut windows = tauri_build::WindowsAttributes::new();
    windows = windows.app_manifest(
        r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
        <requestedPrivileges>
            <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
        </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#,
    );
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run build script");

    #[cfg(windows)]
    add_easytier_third_party_link_search_and_copy_runtime();

    #[cfg(windows)]
    generate_easytier_runtime_assets_rs();

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("secrets.rs");

    let release_key_path = "src/core/minecraft/gdk/Cik/bdb9e791-c97c-3734-e1a8-bc602552df06.cik";
    let preview_key_path = "src/core/minecraft/gdk/Cik/1f49d63f-8bf5-1f8d-ed7e-dbd89477dad9.cik";

    let release_code = if Path::new(release_key_path).exists() {
        println!("cargo:rerun-if-changed={}", release_key_path);
        let bytes = fs::read(release_key_path).expect("Failed to read release key");
        let hex = hex::encode(bytes);
        format!(r#"Some("{}")"#, hex)
    } else {
        println!("cargo:warning=Local Release Key not found, fallback to env var.");
        r#"option_env!("GDK_RELEASE_KEY")"#.to_string()
    };

    let preview_code = if Path::new(preview_key_path).exists() {
        println!("cargo:rerun-if-changed={}", preview_key_path);
        let bytes = fs::read(preview_key_path).expect("Failed to read preview key");
        let hex = hex::encode(bytes);
        format!(r#"Some("{}")"#, hex)
    } else {
        r#"option_env!("GDK_PREVIEW_KEY")"#.to_string()
    };

    let secrets_content = format!(
        r#"
        pub const RELEASE_KEY_HEX: Option<&'static str> = {};
        pub const PREVIEW_KEY_HEX: Option<&'static str> = {};
        "#,
        release_code, preview_code
    );

    fs::write(&dest_path, secrets_content).expect("Failed to write secrets.rs");

    let output = Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
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
