use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
fn add_easytier_third_party_link_search() {
    let target = env::var("TARGET").unwrap_or_default();
    let arch_dir = if target.contains("x86_64") {
        "x86_64"
    } else if target.contains("i686") {
        "i686"
    } else if target.contains("aarch64") {
        "arm64"
    } else {
        return;
    };

    // EasyTier's build script prints `-L native=easytier/third_party/<arch>/` as a relative path,
    // which works inside the EasyTier repo but breaks when EasyTier is used as a git dependency.
    // Work around it by adding an absolute `-L` to the dependency checkout's third_party dir.
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".cargo")));
    let Some(cargo_home) = cargo_home else {
        return;
    };

    let checkouts_dir = cargo_home.join("git").join("checkouts");
    let Ok(repos) = fs::read_dir(&checkouts_dir) else {
        return;
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
            if !third_party.join("Packet.lib").exists() {
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

    if let Some((_, third_party)) = best {
        println!("cargo:rustc-link-search=native={}", third_party.display());
    }
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
    add_easytier_third_party_link_search();

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
