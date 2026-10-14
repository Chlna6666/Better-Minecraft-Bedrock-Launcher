//! Read-only measurements of actual import previews and installed-version scans.
//! cargo bench --features bench-support --bench resource_parsing --profile dev -- PACK_DIR [VERSION_DIR] [ROUNDS] [--handoff]
use std::fs::File;
use std::hint::black_box;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use bmcbl::bench_support::{inspect_archive, scan_archive, scan_local_versions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn fingerprint(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn measure<T>(
    rounds: usize,
    mut operation: impl FnMut() -> Result<T>,
    canonicalize: impl Fn(T) -> Result<Value>,
) -> Result<(Value, f64, f64)> {
    let expected = canonicalize(operation()?)?;
    let mut samples = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let started = Instant::now();
        let actual = black_box(operation()?);
        samples.push(started.elapsed().as_secs_f64() * 1e3);
        if canonicalize(actual)? != expected {
            bail!("parser output changed between rounds");
        }
    }
    samples.sort_by(f64::total_cmp);
    Ok((expected, samples[rounds / 2], samples[rounds * 9 / 10]))
}

fn scan(path: &Path) -> Result<Value> {
    let mut archive = ZipArchive::new(File::open(path)?)?;
    let entries = archive.len();
    let result = scan_archive(&mut archive)?;
    let mut roots = result.level_roots;
    roots.sort();
    Ok(json!({
        "entries": entries, "world": result.is_world, "compound": result.is_compound,
        "nested": result.has_nested_archive, "level_roots": roots,
        "packs": result.packs.iter().map(|pack| json!({
            "root": pack.root, "path": pack.manifest_path, "manifest": pack.manifest,
            "kind": pack.pack_type.to_dir_name(),
        })).collect::<Vec<_>>()
    }))
}

fn profile_pack(path: &Path, rounds: usize, handoff: bool) -> Result<()> {
    let hash = fingerprint(path)?;
    let preview_is_read_only = preview_has_metadata(path)?;
    for phase in ["scan", "preview"] {
        if phase == "preview" && !preview_is_read_only {
            println!(
                "{}",
                json!({"file": path.file_name().map(|name| name.to_string_lossy()), "phase": phase, "skipped": "requires extraction fallback"})
            );
            continue;
        }
        let measured = if phase == "scan" {
            measure(rounds, || scan(path), Ok)
        } else {
            measure(
                rounds,
                || inspect_archive(path, Some("zh-CN")),
                |preview| Ok(serde_json::to_value(preview)?),
            )
        };
        let (output, median, p90) =
            measured.with_context(|| format!("{} phase={phase}", path.display()))?;
        println!(
            "{}",
            json!({"file": path.file_name().map(|name| name.to_string_lossy()), "sha256": hash, "phase": phase,
            "rounds": rounds, "median_ms": median, "p90_ms": p90, "cache": "warm",
            "output": output})
        );
    }
    if fingerprint(path)? != hash {
        bail!("fixture changed: {}", path.display());
    }
    if handoff && preview_is_read_only {
        profile_handoff(path, rounds)?;
    }
    Ok(())
}

// Isolates buffer ownership transfer, excluding networking, decoding and GPU upload.
fn profile_handoff(path: &Path, rounds: usize) -> Result<()> {
    let preview = inspect_archive(path, Some("zh-CN"))?;
    let Some(icon) = preview.icon else {
        return Ok(());
    };
    let bytes = bytes::Bytes::from(icon.bytes.as_ref().clone());
    let iterations = 1000;
    for variant in ["icon_copy", "icon_shared", "http_copy", "http_shared"] {
        let (_, median, p90) = measure(
            rounds,
            || {
                handoff_batch(&icon.bytes, &bytes, variant, iterations);
                Ok(())
            },
            |()| Ok(Value::Null),
        )?;
        println!(
            "{}",
            json!({"file": path.file_name().map(|name| name.to_string_lossy()),
            "phase": "handoff", "variant": variant, "iterations": iterations, "rounds": rounds,
            "payload_bytes": bytes.len(), "median_ms": median, "p90_ms": p90})
        );
    }
    Ok(())
}

fn handoff_batch(icon: &Arc<Vec<u8>>, bytes: &bytes::Bytes, variant: &str, iterations: usize) {
    use gpui::{CompressedImageBytes, EncodedImageBytes, ImageFormat, http_client::AsyncBody};

    for _ in 0..iterations {
        match variant {
            "icon_copy" => {
                black_box(EncodedImageBytes::new(
                    ImageFormat::Png,
                    icon.as_ref().clone(),
                ));
            }
            "icon_shared" => {
                black_box(EncodedImageBytes::new(
                    ImageFormat::Png,
                    CompressedImageBytes::Owned(Arc::clone(icon)),
                ));
            }
            "http_copy" => {
                black_box(AsyncBody::from(bytes.to_vec()));
            }
            _ => {
                black_box(AsyncBody::from(bytes.clone()));
            }
        }
    }
}

// Full preview can persist extracted files for unrecognized compounds. Only
// measure inputs whose existing metadata/quick nested path produces a preview.
fn preview_has_metadata(path: &Path) -> Result<bool> {
    let mut archive = ZipArchive::new(File::open(path)?)?;
    let scan = scan_archive(&mut archive)?;
    if !scan.packs.is_empty() || !scan.level_roots.is_empty() || !scan.has_nested_archive {
        return Ok(true);
    }
    let names = archive
        .file_names()
        .filter_map(std::result::Result::ok)
        .filter(|name| {
            Path::new(name.as_ref())
                .extension()
                .is_some_and(|extension| {
                    ["zip", "mcpack", "mcaddon", "mcworld", "mctemplate"]
                        .iter()
                        .any(|name| extension.eq_ignore_ascii_case(name))
                })
        })
        .map(|name| name.into_owned())
        .collect::<Vec<_>>();
    for name in names {
        let mut file = archive.by_name(&name)?;
        if file.size() > 200 * 1024 * 1024 {
            continue;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let Ok(mut nested) = ZipArchive::new(Cursor::new(bytes)) else {
            continue;
        };
        let scan = scan_archive(&mut nested)?;
        if !scan.packs.is_empty() || !scan.level_roots.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let pack_dir = PathBuf::from(
        arguments
            .next()
            .context("PACK_DIR [VERSION_DIR] [ROUNDS]")?,
    );
    let version_dir = arguments.next().map(PathBuf::from);
    let rounds = arguments
        .next()
        .map(|value| value.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(9);
    let handoff = arguments
        .next()
        .is_some_and(|argument| argument == "--handoff");
    if !(3..=101).contains(&rounds) {
        bail!("rounds must be 3..=101");
    }
    let mut paths = std::fs::read_dir(pack_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    for path in paths {
        if path.is_file()
            && path.extension().is_some_and(|extension| {
                ["zip", "mcpack", "mcaddon", "mcworld", "mctemplate"]
                    .iter()
                    .any(|name| extension.eq_ignore_ascii_case(name))
            })
        {
            profile_pack(&path, rounds, handoff)?;
        }
    }
    if let Some(path) = version_dir {
        let (output, median, p90) = measure(rounds, || scan_local_versions(&path), Ok)?;
        println!(
            "{}",
            json!({"directory": path, "phase": "versions", "rounds": rounds,
            "median_ms": median, "p90_ms": p90, "cache": "warm", "output": output})
        );
    }
    Ok(())
}
