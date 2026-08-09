use super::*;

#[test]
fn go_proxy_path_escapes_uppercase_letters() {
    assert_eq!(
        go_proxy_path("github.com/LiteLDev/PreLoader"),
        "github.com/!lite!l!dev/!pre!loader"
    );
}

#[test]
fn version_requirement_accepts_lip_comparator_sequence() {
    assert!(version_matches("26.10.9", ">=26.10.0 <26.20.0"));
}

#[test]
fn version_requirement_rejects_other_minor_series() {
    assert!(!version_matches("26.20.1", "26.10.*"));
}

#[test]
fn package_id_preserves_client_variant() {
    let package =
        PackageId::parse("github.com/LiteLDev/LeviLamina#client").expect("valid package id");

    assert_eq!(package.variant, "client");
}
