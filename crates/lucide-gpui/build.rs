use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn icon_id_for(stem: &str) -> String {
    let mut name = String::with_capacity(stem.len() + 5);
    name.push_str("icon_");
    for ch in stem.chars() {
        match ch {
            '-' => name.push('_'),
            'a'..='z' | '0'..='9' | '_' => name.push(ch),
            _ => name.push('_'),
        }
    }
    name
}

fn read_icon_stems(icons_dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut stems = Vec::new();
    for entry in fs::read_dir(icons_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension() != Some(OsStr::new("svg")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        stems.push(stem.to_string());
    }
    stems.sort();
    stems.dedup();
    Ok(stems)
}

fn main() -> anyhow::Result<()> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let icons_dir = manifest_dir.join("icons");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let out_file = out_dir.join("icons_gen.rs");

    println!("cargo:rerun-if-changed={}", icons_dir.display());

    let stems = read_icon_stems(&icons_dir)?;
    let mut output = String::new();

    output.push_str("pub mod icons {\n");
    output.push_str("    #[allow(clippy::doc_markdown)]\n");
    output.push_str("    use std::sync::Once;\n\n");

    for stem in &stems {
        let function_name = icon_id_for(stem);
        let lucide_path = format!("lucide/{stem}.svg");
        let icon_rel = format!("/icons/{stem}.svg");

        output.push_str("    #[must_use]\n");
        output.push_str(&format!(
            "    pub fn {function_name}() -> &'static str {{\n"
        ));
        output.push_str(&format!("        const PATH: &str = \"{lucide_path}\";\n"));
        output.push_str("        static ONCE: Once = Once::new();\n");
        output.push_str(&format!(
            "        static BYTES: &'static [u8] = include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"{icon_rel}\"));\n"
        ));
        output.push_str("        ONCE.call_once(|| {\n");
        output.push_str("            crate::registry::register(PATH, BYTES);\n");
        output.push_str("        });\n");
        output.push_str("        PATH\n");
        output.push_str("    }\n\n");
    }

    output.push_str("}\n");

    let mut file = fs::File::create(&out_file)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}
