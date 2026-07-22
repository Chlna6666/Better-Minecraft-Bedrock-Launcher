use super::sanitize_instance_folder_name;

#[test]
fn instance_folder_name_cannot_escape_prefix_root() {
    assert_eq!(sanitize_instance_folder_name("."), "default");
    assert_eq!(sanitize_instance_folder_name(".."), "default");
    assert_eq!(sanitize_instance_folder_name("../preview"), "___preview");
    assert_eq!(sanitize_instance_folder_name("folder/name"), "folder_name");
}

#[test]
fn instance_folder_name_preserves_safe_ascii_names() {
    assert_eq!(
        sanitize_instance_folder_name("1.21_preview-2"),
        "1.21_preview-2"
    );
    assert_eq!(sanitize_instance_folder_name(""), "default");
}
