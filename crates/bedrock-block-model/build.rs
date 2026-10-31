// Embed a versioned BSC model archive without requiring the data during
// development or on platforms where no capture has been supplied.
use std::{env, fs, path::PathBuf};
fn main() {
    let manifest=PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source=manifest.join("generated/bsc_models.bin");
    println!("cargo:rerun-if-changed={}",source.display());
    let out=PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("bsc_models.bin");
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
