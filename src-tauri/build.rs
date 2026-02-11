use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

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
