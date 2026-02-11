use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

fn ensure_npcap_sdk_lib_on_windows() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }

    println!("cargo:rerun-if-env-changed=NPCAP_SDK_DIR");
    println!("cargo:rerun-if-env-changed=NPCAP_SDK_URL");
    println!("cargo:rerun-if-env-changed=BMCBL_SKIP_NPCAP_SDK_DOWNLOAD");

    if let Some(root) = env::var_os("NPCAP_SDK_DIR") {
        let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".to_string());
        let arch_dir = if target_arch == "x86" { "Win32" } else { "x64" };
        let lib_dir = PathBuf::from(root).join("Lib").join(arch_dir);
        println!("cargo:rustc-link-search=native={}", lib_dir.to_string_lossy());
        return;
    }

    if env::var_os("BMCBL_SKIP_NPCAP_SDK_DOWNLOAD").is_some() {
        println!("cargo:warning=Npcap SDK auto-download skipped (BMCBL_SKIP_NPCAP_SDK_DOWNLOAD is set). If you hit LNK1181 Packet.lib, set NPCAP_SDK_DIR to an extracted Npcap SDK folder.");
        return;
    }

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".to_string());
    let arch_dir = if target_arch == "x86" { "Win32" } else { "x64" };

    let cache_root = env::var_os("BMCBL_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("CARGO_HOME").map(|p| PathBuf::from(p).join("bmcbl-cache")))
        .or_else(|| env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("bmcbl-cache")))
        .unwrap_or_else(|| PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bmcbl-cache"));

    let sdk_version = "1.13";
    let sdk_dir = cache_root.join(format!("npcap-sdk-{}", sdk_version));
    let resolve_lib_dir = |root: &PathBuf| -> Option<PathBuf> {
        let direct = root.join("Lib").join(arch_dir);
        if direct.exists() {
            return Some(direct);
        }

        let children = fs::read_dir(root).ok()?;
        for child in children.flatten() {
            if !child.path().is_dir() {
                continue;
            }
            let candidate = child.path().join("Lib").join(arch_dir);
            if candidate.exists() {
                return Some(candidate);
            }
        }

        None
    };

    let lib_dir = resolve_lib_dir(&sdk_dir).unwrap_or_else(|| sdk_dir.join("Lib").join(arch_dir));
    let packet_lib = lib_dir.join("Packet.lib");
    let wpcap_lib = lib_dir.join("wpcap.lib");

    if packet_lib.exists() && wpcap_lib.exists() {
        println!("cargo:rustc-link-search=native={}", lib_dir.to_string_lossy());
        return;
    }

    fs::create_dir_all(&sdk_dir).ok();

    let zip_url = env::var("NPCAP_SDK_URL")
        .unwrap_or_else(|_| format!("https://npcap.com/dist/npcap-sdk-{}.zip", sdk_version));
    let zip_path = cache_root.join(format!("npcap-sdk-{}.zip", sdk_version));

    if !zip_path.exists() {
        println!("cargo:warning=Downloading Npcap SDK from {zip_url} (to satisfy Packet.lib on Windows)...");
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri '{}' -OutFile '{}'",
                    zip_url.replace('\'', "''"),
                    zip_path.to_string_lossy().replace('\'', "''")
                ),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {}
            _ => {
                println!("cargo:warning=Failed to download Npcap SDK. If you hit LNK1181 Packet.lib, set NPCAP_SDK_DIR to an extracted Npcap SDK folder.");
                return;
            }
        }
    }

    println!("cargo:warning=Extracting Npcap SDK...");
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                zip_path.to_string_lossy().replace('\'', "''"),
                sdk_dir.to_string_lossy().replace('\'', "''")
            ),
        ])
        .status();

    match status {
        Ok(s) if s.success() => {}
        _ => {
            println!("cargo:warning=Failed to extract Npcap SDK. If you hit LNK1181 Packet.lib, set NPCAP_SDK_DIR to an extracted Npcap SDK folder.");
            return;
        }
    }

    if let Some(lib_dir) = resolve_lib_dir(&sdk_dir) {
        let packet_lib = lib_dir.join("Packet.lib");
        let wpcap_lib = lib_dir.join("wpcap.lib");
        if packet_lib.exists() && wpcap_lib.exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.to_string_lossy());
            return;
        }

        println!("cargo:warning=Npcap SDK extracted but Packet.lib/wpcap.lib not found under {}. If you hit LNK1181 Packet.lib, set NPCAP_SDK_DIR to an extracted Npcap SDK folder.", lib_dir.to_string_lossy());
    } else {
        println!("cargo:warning=Npcap SDK extracted but Lib/{} not found under {}. If you hit LNK1181 Packet.lib, set NPCAP_SDK_DIR to an extracted Npcap SDK folder.", arch_dir, sdk_dir.to_string_lossy());
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

    ensure_npcap_sdk_lib_on_windows();

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
