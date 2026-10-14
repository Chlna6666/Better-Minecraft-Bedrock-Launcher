#![cfg(feature = "bench-support")]
//! Tests the production parsers without compiling unrelated GPUI lib-test modules.
use std::io::{Cursor, Write};

use bmcbl::bench_support::{inspect_archive, scan_archive};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn archive_bytes(language_path: Option<&str>) -> Vec<u8> {
    archive_payloads(
        language_path.map(|path| (path, "pack.name=材质测试\n".as_bytes())),
        None,
    )
}

fn archive_payloads(language: Option<(&str, &[u8])>, icon: Option<&[u8]>) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("ordinary.txt", options).unwrap();
    writer.write_all(b"not pack metadata").unwrap();
    writer.start_file("manifest.json", options).unwrap();
    writer.write_all(br#"{"format_version":2,"header":{"name":"pack.name","uuid":"fixture-id","version":[1,0,0]},"modules":[{"type":"resources"}]}"#).unwrap();
    if let Some((path, text)) = language {
        writer.start_file(path, options).unwrap();
        writer.write_all(text).unwrap();
    }
    if let Some(icon) = icon {
        writer.start_file("pack_icon.png", options).unwrap();
        writer.write_all(icon).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn preview_preserves_utf8_bom_and_utf16_languages() {
    let text = "pack.name=材质测试\n";
    let utf8_bom = [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat();
    let little = text
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let big = text
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>();
    for bytes in [
        utf8_bom,
        [b"\xff\xfe".as_slice(), little.as_slice()].concat(),
        [b"\xfe\xff".as_slice(), big.as_slice()].concat(),
        little,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("encoding.mcpack");
        std::fs::write(
            &package,
            archive_payloads(Some(("texts/zh_CN.lang", &bytes)), None),
        )
        .unwrap();
        assert_eq!(
            inspect_archive(&package, Some("zh-CN")).unwrap().name,
            "材质测试"
        );
    }
}

#[test]
fn preview_and_ui_source_share_icon_storage_and_release_it() {
    use gpui::{CompressedImageBytes, EncodedImageBytes, ImageFormat};
    use std::sync::Arc;

    let mut png = Cursor::new(Vec::new());
    gpui::image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut png, gpui::image::ImageFormat::Png)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("icon.mcpack");
    std::fs::write(&package, archive_payloads(None, Some(png.get_ref()))).unwrap();
    let preview = inspect_archive(&package, None).unwrap();
    let cloned = preview.clone();
    let icon = preview.icon.as_ref().unwrap();
    let cloned_icon = cloned.icon.as_ref().unwrap();
    assert!(Arc::ptr_eq(&icon.bytes, &cloned_icon.bytes));
    assert_eq!(icon.bytes.as_slice(), png.get_ref());
    assert_eq!(
        serde_json::to_value(icon).unwrap()["bytes"],
        serde_json::json!(png.get_ref())
    );
    let weak = Arc::downgrade(&icon.bytes);
    let source = EncodedImageBytes::new(
        ImageFormat::Png,
        CompressedImageBytes::Owned(Arc::clone(&icon.bytes)),
    );
    drop(preview);
    drop(cloned);
    assert_eq!(weak.strong_count(), 1);
    drop(source);
    assert_eq!(weak.strong_count(), 0);
}

#[test]
fn gpui_body_handoff_keeps_response_allocation() {
    let bytes = bytes::Bytes::from(vec![7; 4096]);
    let pointer = bytes.as_ptr();
    let body = gpui::http_client::AsyncBody::from(bytes);
    match body.0 {
        gpui::http_client::Inner::Bytes(cursor) => assert_eq!(cursor.get_ref().as_ptr(), pointer),
        _ => panic!("expected in-memory HTTP body"),
    }
}

#[test]
fn scan_retains_manifest_validation_and_crc_errors() {
    let bytes = archive_bytes(None);
    let mut archive = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(scan_archive(&mut archive).unwrap().packs.len(), 1);
    let mut corrupt = bytes;
    let text = corrupt
        .windows(10)
        .position(|bytes| bytes == b"fixture-id")
        .unwrap();
    // Keep valid JSON and invalidate only the content's CRC.
    corrupt[text + 9] = b'x';
    let mut archive = ZipArchive::new(Cursor::new(corrupt)).unwrap();
    assert!(scan_archive(&mut archive).unwrap().packs.is_empty());
}

#[test]
fn cp437_names_preserve_manifest_detection() {
    let mut bytes = archive_bytes(None);
    let central = bytes
        .windows(4)
        .position(|signature| signature == b"PK\x01\x02")
        .unwrap();
    bytes[7] |= 8;
    bytes[30] = 0xff;
    bytes[central + 9] |= 8;
    bytes[central + 46] = 0xff;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert!(!archive.name_for_index(0).unwrap().unwrap().is_ascii());
    assert_eq!(scan_archive(&mut archive).unwrap().packs.len(), 1);
}

#[test]
fn preview_preserves_language_path_case_slashes_and_unicode() {
    for path in ["texts/zh_CN.lang", "TEXTS\\ZH_cn.LANG", "Texts/zh-cn.Lang"] {
        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("语言测试.mcpack");
        std::fs::write(&package, archive_bytes(Some(path))).unwrap();
        let preview = inspect_archive(&package, Some("zh-CN")).unwrap();
        assert_eq!(preview.name, "材质测试");
        assert!(preview.valid);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn executable_lookup_keeps_common_identity_and_fallback_priority() {
    use bmcbl::bench_support::{find_any_game_executable_in_dir, find_game_executable_in_dir};
    for (names, identity, expected) in [
        (vec!["Report.exe", "custom.EXE"], "", Some("custom.EXE")),
        (
            vec!["Identity.exe", "custom.exe"],
            "Identity",
            Some("Identity.exe"),
        ),
        (
            vec!["Identity.exe", "Minecraft.Windows.exe", "Minecraft.exe"],
            "Identity",
            Some("Minecraft.Windows.exe"),
        ),
        (
            vec!["Minecraft.Education.exe", "Minecraft.Win10.DX11.exe"],
            "",
            Some("Minecraft.Win10.DX11.exe"),
        ),
        (vec!["CrashSender.exe", "notes.txt"], "", None),
    ] {
        let directory = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::write(directory.path().join(name), b"").unwrap();
        }
        assert_eq!(
            find_game_executable_in_dir(directory.path(), identity),
            expected.map(|name| directory.path().join(name))
        );
        if identity.is_empty() {
            assert_eq!(
                find_any_game_executable_in_dir(directory.path()),
                expected.map(|name| directory.path().join(name))
            );
        }
    }
}
