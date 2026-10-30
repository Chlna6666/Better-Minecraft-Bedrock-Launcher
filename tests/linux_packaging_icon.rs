use std::fs;
use std::path::Path;

#[test]
fn test_linux_packaging_icon_configuration() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script_path = manifest_dir.join("scripts").join("package-linux.sh");
    assert!(script_path.is_file(), "package-linux.sh must exist");

    let script_content = fs::read_to_string(&script_path).expect("failed to read package-linux.sh");

    // Must prefer high resolution logo.png rather than icon.ico frame [0]
    assert!(
        script_content.contains("assets/images/logo.png"),
        "package-linux.sh must reference assets/images/logo.png as high-res icon source"
    );

    // Verify logo.png exists and is high-resolution
    let logo_path = manifest_dir.join("assets").join("images").join("logo.png");
    assert!(logo_path.is_file(), "assets/images/logo.png must exist");

    let logo_img = image::open(&logo_path).expect("failed to open assets/images/logo.png");
    assert!(
        logo_img.width() >= 512 && logo_img.height() >= 512,
        "assets/images/logo.png should be at least 512x512, got {}x{}",
        logo_img.width(),
        logo_img.height()
    );

    // Check all standard hicolor sizes are generated
    for size in [16, 32, 48, 64, 128, 256, 512] {
        let size_str = format!("{size}x{size}");
        assert!(
            script_content.contains(&format!("icons/hicolor/{size_str}/apps")),
            "package-linux.sh must install {size_str} icon"
        );
    }

    // Check pixmaps fallback is installed
    assert!(
        script_content.contains("usr/share/pixmaps"),
        "package-linux.sh must install pixmaps fallback icon"
    );

    // Check RPM spec %files lists the installed icons
    for size in [16, 32, 48, 64, 128, 256, 512] {
        let size_str = format!("{size}x{size}");
        assert!(
            script_content.contains(&format!(
                "/usr/share/icons/hicolor/{size_str}/apps/$APP_ID.png"
            )),
            "rpm spec %files must list /usr/share/icons/hicolor/{size_str}/apps/$APP_ID.png"
        );
    }
    assert!(
        script_content.contains("/usr/share/pixmaps/$APP_ID.png"),
        "rpm spec %files must list /usr/share/pixmaps/$APP_ID.png"
    );

    // Check desktop file references io.github.chlna6666.BMCBL
    let desktop_path = manifest_dir
        .join("packaging")
        .join("linux")
        .join("io.github.chlna6666.BMCBL.desktop");
    assert!(desktop_path.is_file(), "desktop file must exist");
    let desktop_content = fs::read_to_string(&desktop_path).expect("failed to read desktop file");
    assert!(
        desktop_content.contains("Icon=io.github.chlna6666.BMCBL"),
        "desktop file must specify Icon=io.github.chlna6666.BMCBL"
    );
}
