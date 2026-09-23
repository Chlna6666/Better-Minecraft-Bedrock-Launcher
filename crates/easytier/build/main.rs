mod rpc;

use crate::rpc::ServiceGenerator;
use cfg_aliases::cfg_aliases;
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn git_output(manifest_dir: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_path(manifest_dir: &Path, path: &str) -> Option<PathBuf> {
    git_output(
        manifest_dir,
        &["rev-parse", "--path-format=absolute", "--git-path", path],
    )
    .map(PathBuf::from)
}

fn emit_easytier_version(manifest_dir: &Path) {
    for path in ["HEAD", "packed-refs", "refs/tags"] {
        if let Some(path) = git_path(manifest_dir, path) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    if let Some(reference) = git_output(manifest_dir, &["symbolic-ref", "--quiet", "HEAD"])
        && let Some(path) = git_path(manifest_dir, &reference)
    {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let package_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string());
    let version = git_output(manifest_dir, &["describe", "--abbrev=8", "--always"])
        .filter(|version| !version.is_empty())
        .map_or_else(
            || package_version.clone(),
            |revision| format!("{package_version}-{revision}"),
        );
    println!("cargo:rustc-env=EASYTIER_VERSION={version}");
}

fn workdir() -> Option<String> {
    if let Ok(cargo_manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        return Some(cargo_manifest_dir);
    }

    let dest = std::env::var("OUT_DIR");
    if dest.is_err() {
        return None;
    }
    let dest = dest.unwrap();

    let seperator = regex::Regex::new(r"(/target/(.+?)/build/)|(\\target\\(.+?)\\build\\)")
        .expect("Invalid regex");
    let parts = seperator.split(dest.as_str()).collect::<Vec<_>>();

    if parts.len() >= 2 {
        return Some(parts[0].to_string());
    }

    None
}

fn check_locale() {
    let workdir = workdir().unwrap_or("./".to_string());

    let locale_path = format!("{workdir}/**/locales/**/*");
    if let Ok(globs) = globwalk::glob(locale_path) {
        for entry in globs {
            if let Err(e) = entry {
                println!("cargo:i18n-error={e}");
                continue;
            }

            let entry = entry.unwrap().into_path();
            println!("cargo:rerun-if-changed={}", entry.display());
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR")?);
    emit_easytier_version(&manifest_dir);

    cfg_aliases! {
        mobile: {
            any(
                target_os = "android",
                target_os = "ios",
                all(target_os = "macos", feature = "macos-ne"),
                target_env = "ohos"
            )
        }
    }

    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: Cargo runs this build script in its own process before protobuf generation starts.
    unsafe { env::set_var("PROTOC", protoc_path) };

    let proto_files_reflect = ["src/proto/peer_rpc.proto", "src/proto/common.proto"];

    let proto_files = [
        "src/proto/error.proto",
        "src/proto/tests.proto",
        "src/proto/api_instance.proto",
        "src/proto/api_logger.proto",
        "src/proto/api_config.proto",
        "src/proto/api_manage.proto",
        "src/proto/web.proto",
        "src/proto/magic_dns.proto",
        "src/proto/acl.proto",
    ];

    for proto_file in proto_files.iter().chain(proto_files_reflect.iter()) {
        println!("cargo:rerun-if-changed={proto_file}");
    }

    let out = PathBuf::from(env::var("OUT_DIR")?);
    let descriptor = out.join("descriptors.bin");

    let mut config = prost_build::Config::new();
    config
        .extern_path(".google.protobuf.Any", "::prost_wkt_types::Any")
        .extern_path(".google.protobuf.Timestamp", "::prost_wkt_types::Timestamp")
        .extern_path(".google.protobuf.Value", "::prost_wkt_types::Value")
        .file_descriptor_set_path(&descriptor)
        .service_generator(Box::new(ServiceGenerator::default()))
        .btree_map(["."])
        .skip_debug([".common.Ipv4Addr", ".common.Ipv6Addr", ".common.UUID"]);

    config.compile_protos(&proto_files, &["src/proto/"])?;

    prost_reflect_build::Builder::new()
        .file_descriptor_set_bytes("crate::proto::DESCRIPTOR_POOL_BYTES")
        .compile_protos_with_config(config, &proto_files_reflect, &["src/proto/"])?;

    let descriptor = std::fs::read(descriptor)?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor)?
        .preserve_proto_field_names()
        .btree_map(["."])
        .build(&["."])?;

    check_locale();
    Ok(())
}
