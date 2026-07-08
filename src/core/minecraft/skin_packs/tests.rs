use super::*;

#[test]
fn skin_display_name_uses_pack_scoped_localization() {
    let skins_json = SkinsJson {
        serialize_name: None,
        localization_name: Some("alleis".to_string()),
        skins: Vec::new(),
    };
    let skin = SkinJsonEntry {
        localization_name: Some("birthday".to_string()),
        geometry: Some("geometry.humanoid.custom".to_string()),
        texture: Some("birthday.png".to_string()),
        skin_type: Some("free".to_string()),
        cape: None,
        extra: serde_json::Map::new(),
    };
    let lang_map = HashMap::from([(
        "skin.alleis.birthday".to_string(),
        "Birthday Skin".to_string(),
    )]);

    assert_eq!(
        skin_display_name(&skins_json, &skin, &lang_map),
        "Birthday Skin"
    );
}

#[test]
fn full_texture_path_is_separate_from_head_preview_path() {
    let skin = McSkinPackSkinInfo {
        display_name: "Alex".to_string(),
        localization_name: None,
        full_texture_path: Some("packs/alex.png".to_string()),
        preview_path: Some("cache/skin_previews/head.png".to_string()),
        model_label: "Alex".to_string(),
        geometry_path: None,
        geometry_identifier: None,
    };
    let pack = McSkinPackInfo {
        folder_name: "pack".to_string(),
        folder_path: "packs".to_string(),
        display_name: "Pack".to_string(),
        description: None,
        version: None,
        icon_path: None,
        preview_path: skin.preview_path.clone(),
        first_full_skin_texture_path: None,
        skin_count: 1,
        slim_skin_count: 1,
        source: None,
        edition: None,
        source_root: None,
        gdk_user: None,
        skins: vec![skin],
    };

    assert_eq!(pack.first_full_skin_texture_path(), Some("packs/alex.png"));
    assert_ne!(
        pack.first_full_skin_texture_path(),
        pack.preview_path.as_deref()
    );
}

#[test]
fn skins_json_accepts_lossy_utf8_in_strings() {
    let raw = b"{\"skins\":[{\"localization_name\":\"bad\xffname\",\"texture\":\"a.png\"}]}";
    let parsed: SkinsJson = serde_json::from_str(&lossy_text(raw))
        .unwrap_or_else(|error| panic!("lossy skins json should parse: {error}"));

    assert_eq!(parsed.skins.len(), 1);
    assert_eq!(
        parsed.skins[0].localization_name.as_deref(),
        Some("bad\u{fffd}name")
    );
}

#[test]
fn custom_geometry_labels_as_custom_model() {
    assert_eq!(model_label_from_geometry("geometry.n0"), "自定义");
    assert_eq!(
        model_label_from_geometry("geometry.humanoid.custom"),
        "自定义"
    );
    assert_eq!(
        model_label_from_geometry("geometry.humanoid.customSlim"),
        "Alex"
    );
}
