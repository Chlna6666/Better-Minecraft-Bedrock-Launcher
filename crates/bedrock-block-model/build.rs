// Embed a versioned BSC model archive without requiring the data during
// development or on platforms where no capture has been supplied.
use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    let manifest=PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source=manifest.join("generated/bsc_models.bin");
    println!("cargo:rerun-if-changed={}",source.display());
    println!("cargo:rerun-if-env-changed=BMCBL_BSC_EXPORT");
    println!("cargo:rerun-if-env-changed=BMCBL_PYTHON");
    let out=PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("bsc_models.bin");
    // Optional direct conversion of a BSC ZIP or export directory during
    // compilation. No Python interpreter is needed for ordinary builds.
    if let Some(export)=env::var_os("BMCBL_BSC_EXPORT").filter(|value|!value.is_empty()) {
        let export=PathBuf::from(export);
        println!("cargo:rerun-if-changed={}",export.display());
        let script=manifest.join("../../scripts/pack_bsc_model_library.py");
        println!("cargo:rerun-if-changed={}",script.display());
        let python=env::var_os("BMCBL_PYTHON").unwrap_or_else(||
            if cfg!(windows) {"python".into()} else {"python3".into()});
        let result=Command::new(&python).arg(&script).arg(&export).arg(&out).status()
            .unwrap_or_else(|error|panic!("failed to invoke BSC packer {python:?}: {error}"));
        assert!(result.success(),"BSC export packing failed: {}",export.display());
        let size=fs::metadata(&out).expect("BSC embedded archive output").len();
        assert!(size<=64*1024*1024,"BSC archive exceeds 64 MiB");
        return;
    }
    match fs::read(&source) {
        Ok(data) => {
            assert!(data.len()<=64*1024*1024,"BSC archive exceeds 64 MiB");
            assert!(data.starts_with(b"PK\x03\x04"),"BSC archive must be a ZIP-based .bin");
            fs::write(out,data).expect("write embedded BSC archive");
        }
        Err(error) if error.kind()==std::io::ErrorKind::NotFound => {
            fs::write(out,[]).expect("write empty BSC archive");
        }
        Err(error) => panic!("failed to read {}: {error}",source.display()),
    }
}
