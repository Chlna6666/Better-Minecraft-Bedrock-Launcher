use super::*;

#[test]
fn flipbook_metadata_follows_the_resolved_images_pack() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base");
    let overlay = directory.path().join("overlay");
    for root in [&base, &overlay] {
        std::fs::create_dir_all(root.join("textures/blocks")).unwrap();
        std::fs::write(root.join("textures/blocks/water.png"), []).unwrap();
    }
    let original = serde_json::json!([{
        "flipbook_texture":"textures/blocks/water", "ticks_per_frame":2,
        "frames":[2,0], "replicate":2, "blend_frames":true
    }]);
    std::fs::write(
        base.join("textures/flipbook_textures.json"),
        original.to_string(),
    )
    .unwrap();
    let resolver = ObjTextureResolver::with_pack_roots([&base, &overlay], "textures");
    let texture = resolver
        .environment_texture("textures/blocks/water")
        .unwrap();
    assert_eq!(texture.pack_root, overlay);
    assert!(
        resolver.flipbook_definition_for(&texture).is_none(),
        "lower pack timing must not animate an overriding static image"
    );
    std::fs::write(
        overlay.join("textures/flipbook_textures.json"),
        original.to_string(),
    )
    .unwrap();
    let resolver = ObjTextureResolver::with_pack_roots([&base, &overlay], "textures");
    let texture = resolver
        .environment_texture("textures/blocks/water")
        .unwrap();
    assert_eq!(
        resolver.flipbook_definition_for(&texture),
        Some(&original[0])
    );
}
