from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
path = ROOT / "src/ui/window/map_viewer/players.rs"
text = path.read_text(encoding="utf-8")
old = '''        let Some(stem) = path.file_stem().and_then(|v| v.to_str()) else {
            continue;
        };
        if stem.ends_with("_0") || stem.ends_with("_1") {
            continue;
        }
        let id = normalize_item_id(stem);
        by_id
            .entry(id.clone())
            .or_insert_with(|| PlayerItemTexture {
                id: SharedString::from(id),
                label: SharedString::from(stem.replace('_', " ")),
                path: Arc::<Path>::from(path.into_boxed_path()),
            });'''
new = '''        let Some(stem) = path
            .file_stem()
            .and_then(|v| v.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if stem.ends_with("_0") || stem.ends_with("_1") {
            continue;
        }
        let id = normalize_item_id(&stem);
        by_id
            .entry(id.clone())
            .or_insert_with(|| PlayerItemTexture {
                id: SharedString::from(id),
                label: SharedString::from(stem.replace('_', " ")),
                path: Arc::<Path>::from(path.into_boxed_path()),
            });'''
if text.count(old) != 1:
    raise SystemExit(f"expected one scan_flat_textures anchor, got {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
