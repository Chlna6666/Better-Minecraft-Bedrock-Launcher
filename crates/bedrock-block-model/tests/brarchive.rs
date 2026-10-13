use std::fs;
use std::io::Cursor;
use std::path::Path;

use bedrock_block_model::{
    BlockModelError, BlockModelRepository, ObjTextureCopy, ObjTextureResolver,
    read_obj_texture_copy_image, write_obj_texture_copy,
};

fn write_archive(root: &Path, relative: &str, entries: &[(&str, &[u8])]) {
    let mut bytes = vec![0; 16 + entries.len() * 256];
    bytes[..8].copy_from_slice(&[0x7d, 0x27, 0x25, 0xb1, 0xa0, 0x52, 0x70, 0x26]);
    bytes[8..12].copy_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
    let mut offset = 0u32;
    for (index, (name, payload)) in entries.iter().enumerate() {
        let record = &mut bytes[16 + index * 256..16 + (index + 1) * 256];
        record[0] = u8::try_from(name.len()).unwrap();
        record[1..=name.len()].copy_from_slice(name.as_bytes());
        record[248..252].copy_from_slice(&offset.to_le_bytes());
        let length = u32::try_from(payload.len()).unwrap();
        record[252..256].copy_from_slice(&length.to_le_bytes());
        offset += length;
    }
    for (_, payload) in entries {
        bytes.extend_from_slice(payload);
    }
    let path = root.join("__brarchive").join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn atlas() -> &'static [u8] {
    br#"{"texture_data":{"stone":{"textures":"textures/blocks/stone"}}}"#
}

fn png(color: [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba(color));
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

fn resolved_copy(root: &Path) -> ObjTextureCopy {
    let resolver = ObjTextureResolver::try_with_pack_roots([root], "textures").unwrap();
    let texture = resolver.texture_for("minecraft_stone", [0, 1, 0]).unwrap();
    ObjTextureCopy {
        source_path: texture.source_path,
        relative_path: texture.relative_path,
        tint: None,
        alpha_mask: false,
    }
}

#[test]
fn archive_only_atlas_geometry_and_png_work_without_extraction() {
    let pack = tempfile::tempdir().unwrap();
    write_archive(
        pack.path(),
        "textures.brarchive",
        &[("terrain_texture.json", atlas())],
    );
    let image_bytes = png([11, 22, 33, 255]);
    write_archive(
        pack.path(),
        "textures/blocks.brarchive",
        &[("stone.png", &image_bytes)],
    );
    write_archive(pack.path(), "blocks.brarchive", &[("stone.json",
        br#"{"minecraft:block":{"description":{"identifier":"minecraft:stone"},"components":{"minecraft:geometry":"geometry.test"}}}"#)]);
    write_archive(pack.path(), "models/blocks.brarchive", &[("stone.geo.json",
        br#"{"minecraft:geometry":[{"description":{"identifier":"geometry.test"},"bones":[]}]}"#)]);
    let repository = BlockModelRepository::load_packs([pack.path()]).unwrap();
    assert!(repository.geometry("geometry.test").is_some());
    assert!(repository.blocks.contains_key("minecraft:stone"));
    assert_eq!(
        repository.terrain_textures.resolve("stone").primary_path(),
        Some("textures/blocks/stone")
    );
    let copy = resolved_copy(pack.path());
    assert!(!copy.source_path.path().exists());
    assert_eq!(copy.source_path.read().unwrap(), image_bytes);
    assert_eq!(copy.relative_path, "textures/blocks/stone.png");
    // The resolver has already been dropped; the export retains its source.
    let image = read_obj_texture_copy_image(&copy).unwrap().to_rgba8();
    assert_eq!(image.get_pixel(0, 0).0, [11, 22, 33, 255]);
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("stone.png");
    write_obj_texture_copy(&copy, &target).unwrap();
    assert_eq!(
        image::open(target).unwrap().to_rgba8().get_pixel(0, 0).0,
        [11, 22, 33, 255]
    );
    assert!(!pack.path().join("textures").exists());
}

#[test]
fn embedded_payload_wins_and_stubs_fall_back_to_loose_files() {
    let pack = tempfile::tempdir().unwrap();
    let texture_dir = pack.path().join("textures/blocks");
    fs::create_dir_all(&texture_dir).unwrap();
    fs::write(texture_dir.join("stone.png"), png([1, 2, 3, 255])).unwrap();
    write_archive(
        pack.path(),
        "textures.brarchive",
        &[("terrain_texture.json", atlas())],
    );
    let archived = png([4, 5, 6, 255]);
    write_archive(
        pack.path(),
        "textures/blocks.brarchive",
        &[("stone.png", &archived)],
    );
    assert_eq!(
        resolved_copy(pack.path()).source_path.read().unwrap(),
        archived
    );
    write_archive(
        pack.path(),
        "textures/blocks.brarchive",
        &[("stone.png", b"")],
    );
    assert_eq!(
        read_obj_texture_copy_image(&resolved_copy(pack.path()))
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0,
        [1, 2, 3, 255]
    );
    fs::remove_file(texture_dir.join("stone.png")).unwrap();
    let resolver = ObjTextureResolver::try_with_pack_roots([pack.path()], "textures").unwrap();
    assert!(resolver.texture_for("minecraft_stone", [0, 1, 0]).is_none());
}

#[test]
fn later_loose_pack_overrides_earlier_archive_texture_and_atlas() {
    let base = tempfile::tempdir().unwrap();
    write_archive(
        base.path(),
        "textures.brarchive",
        &[("terrain_texture.json", atlas())],
    );
    write_archive(
        base.path(),
        "textures/blocks.brarchive",
        &[("stone.png", &png([1, 2, 3, 255]))],
    );
    let overlay = tempfile::tempdir().unwrap();
    fs::create_dir_all(overlay.path().join("textures/blocks")).unwrap();
    fs::write(
        overlay.path().join("textures/terrain_texture.json"),
        br#"{"texture_data":{"stone":{"textures":"textures/blocks/custom"}}}"#,
    )
    .unwrap();
    let custom = png([7, 8, 9, 255]);
    fs::write(overlay.path().join("textures/blocks/custom.png"), &custom).unwrap();
    let repository = BlockModelRepository::load_packs([base.path(), overlay.path()]).unwrap();
    assert_eq!(
        repository.terrain_textures.resolve("stone").primary_path(),
        Some("textures/blocks/custom")
    );
    let resolver =
        ObjTextureResolver::try_with_pack_roots([base.path(), overlay.path()], "textures").unwrap();
    let texture = resolver.texture_for("minecraft_stone", [0, 1, 0]).unwrap();
    assert_eq!(texture.source_path.read().unwrap(), custom);
}

#[test]
fn corrupt_archive_and_compiled_mcb_produce_explicit_errors() {
    let pack = tempfile::tempdir().unwrap();
    write_archive(
        pack.path(),
        "textures.brarchive",
        &[("terrain_texture.json", b"\x7fMCB\0")],
    );
    assert!(matches!(
        BlockModelRepository::load_packs([pack.path()]),
        Err(BlockModelError::UnsupportedMcb { .. })
    ));
    let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
    assert!(
        resolver
            .model_errors()
            .iter()
            .any(|message| message.contains("MCB"))
    );
    assert!(ObjTextureResolver::try_with_pack_roots([pack.path()], "textures").is_err());
    fs::write(
        pack.path().join("__brarchive/textures.brarchive"),
        b"invalid",
    )
    .unwrap();
    assert!(matches!(
        BlockModelRepository::load_packs([pack.path()]),
        Err(BlockModelError::Archive { .. })
    ));
}
