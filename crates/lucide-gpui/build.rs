use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

fn main() -> anyhow::Result<()> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let bmcbl_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("lucide-gpui must remain under BMCBL/crates"))?;
    let src_dir = bmcbl_root.join("src");
    let icons_dir = manifest_dir.join("icons");

    let icons = collect_used_icons(&src_dir)?;
    generate_assets(&icons, &icons_dir, &out_dir)
}

fn collect_used_icons(src_dir: &Path) -> anyhow::Result<BTreeSet<String>> {
    anyhow::ensure!(
        src_dir.is_dir(),
        "BMCBL source directory does not exist: {}",
        src_dir.display()
    );
    println!("cargo:rerun-if-changed={}", src_dir.display());

    let mut source_files = Vec::new();
    collect_rust_sources(src_dir, &mut source_files)?;
    source_files.sort_unstable();

    let mut icons = BTreeSet::new();
    for source_path in source_files {
        let source = fs::read_to_string(&source_path)?;
        collect_icon_calls(&source, &mut icons);
    }
    Ok(icons)
}

fn generate_assets(
    icons: &BTreeSet<String>,
    icons_dir: &Path,
    out_dir: &Path,
) -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed={}", icons_dir.display());
    let mut bytes = Vec::new();
    let mut generated = String::from("#[macro_export]\nmacro_rules! icon {\n");
    for (index, icon) in icons.iter().enumerate() {
        writeln!(
            generated,
            "    ({icon} $(,)?) => {{ $crate::__icon_path({index}) }};"
        )?;
    }
    generated.push_str(
        "    ($name:ident $(,)?) => { compile_error!(concat!(\"unused or unknown Lucide icon: \", stringify!($name))) };\n}\n\n",
    );
    generated.push_str("pub(crate) static ICONS: &[IconEntry] = &[\n");

    for icon in icons {
        let svg_path = icons_dir.join(format!("{}.svg", icon.replace('_', "-")));
        anyhow::ensure!(
            svg_path.is_file(),
            "icon!({icon}) maps to missing Lucide asset {}",
            svg_path.display()
        );
        println!("cargo:rerun-if-changed={}", svg_path.display());

        let svg = fs::read(&svg_path)?;
        let offset = bytes.len();
        bytes.extend_from_slice(&svg);
        writeln!(
            generated,
            "    IconEntry {{ path: \"lucide/{icon}.svg\", offset: {offset}, len: {} }},",
            svg.len()
        )?;
    }
    generated.push_str("];\n");
    writeln!(
        generated,
        "#[cfg(test)]\npub(crate) const ICON_BYTES_LEN: usize = {};",
        bytes.len()
    )?;

    fs::write(out_dir.join("lucide_icons.bin"), bytes)?;
    fs::write(out_dir.join("lucide_icons.rs"), generated)?;
    Ok(())
}

fn collect_rust_sources(dir: &Path, sources: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_sources(&path, sources)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            sources.push(path);
        }
    }
    Ok(())
}

fn collect_icon_calls(source: &str, icons: &mut BTreeSet<String>) {
    let bytes = source.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index..].starts_with(b"//") {
            index = skip_line_comment(bytes, index + 2);
        } else if bytes[index..].starts_with(b"/*") {
            index = skip_block_comment(bytes, index + 2);
        } else if let Some(end) = raw_string_end(bytes, index) {
            index = end;
        } else if bytes[index] == b'"' {
            index = skip_quoted_string(bytes, index + 1);
        } else if matches!(bytes[index], b'b' | b'c') && bytes.get(index + 1) == Some(&b'"') {
            index = skip_quoted_string(bytes, index + 2);
        } else if bytes[index] == b'\''
            && let Some(end) = char_literal_end(source, index)
        {
            index = end;
        } else if is_ident_start(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_ident_continue(bytes[index]) {
                index += 1;
            }
            if &source[start..index] == "lucide_gpui"
                && let Some((name, end)) = parse_icon_invocation(source, index)
            {
                icons.insert(name);
                index = end;
            }
        } else {
            index += 1;
        }
    }
}

fn parse_icon_invocation(source: &str, mut index: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    index = skip_whitespace(bytes, index);
    if !bytes.get(index..)?.starts_with(b"::") {
        return None;
    }
    index = skip_whitespace(bytes, index + 2);

    let icon_end = index.checked_add(4)?;
    if bytes.get(index..icon_end) != Some(b"icon")
        || bytes
            .get(icon_end)
            .is_some_and(|byte| is_ident_continue(*byte))
    {
        return None;
    }
    index = skip_whitespace(bytes, icon_end);
    if bytes.get(index) != Some(&b'!') {
        return None;
    }
    index = skip_whitespace(bytes, index + 1);
    if bytes.get(index) != Some(&b'(') {
        return None;
    }
    index = skip_whitespace(bytes, index + 1);

    let start = index;
    if !bytes.get(index).copied().is_some_and(is_ident_start) {
        return None;
    }
    index += 1;
    while index < bytes.len() && is_ident_continue(bytes[index]) {
        index += 1;
    }
    let name = source[start..index].to_owned();

    index = skip_whitespace(bytes, index);
    if bytes.get(index) == Some(&b',') {
        index = skip_whitespace(bytes, index + 1);
    }
    (bytes.get(index) == Some(&b')')).then_some((name, index + 1))
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> usize {
    let mut depth = 1_u32;
    while index + 1 < bytes.len() {
        if bytes[index..].starts_with(b"/*") {
            depth = depth.saturating_add(1);
            index += 2;
        } else if bytes[index..].starts_with(b"*/") {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
        } else {
            index += 1;
        }
    }
    bytes.len()
}

fn skip_quoted_string(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    if matches!(bytes.get(index), Some(b'b' | b'c')) {
        if bytes.get(index + 1) != Some(&b'r') {
            return None;
        }
        index += 1;
    }
    if bytes.get(index) != Some(&b'r') {
        return None;
    }
    index += 1;

    let mut hashes = 0;
    while bytes.get(index) == Some(&b'#') {
        hashes += 1;
        index += 1;
    }
    if bytes.get(index) != Some(&b'"') {
        return None;
    }
    index += 1;

    while index < bytes.len() {
        if bytes[index] == b'"' {
            let end = index + 1 + hashes;
            if bytes
                .get(index + 1..end)
                .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                return Some(end);
            }
        }
        index += 1;
    }
    Some(bytes.len())
}

fn char_literal_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'\\') {
        index += 1;
        match bytes.get(index).copied()? {
            b'x' => index += 3,
            b'u' => {
                index += 2;
                while bytes.get(index).is_some_and(|byte| *byte != b'}') {
                    index += 1;
                }
                index += 1;
            }
            _ => index += 1,
        }
    } else {
        let ch = source.get(index..)?.chars().next()?;
        if matches!(ch, '\n' | '\r' | '\'') {
            return None;
        }
        index += ch.len_utf8();
    }
    (bytes.get(index) == Some(&b'\'')).then_some(index + 1)
}

#[inline]
fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

#[inline]
fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}
