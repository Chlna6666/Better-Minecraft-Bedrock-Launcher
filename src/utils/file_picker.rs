use std::path::PathBuf;

/// Open a native file picker and return a selected file path.
pub fn pick_file_path_with_filter(filter_name: &str, extensions: &[&str]) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if !extensions.is_empty() {
        dialog = dialog.add_filter(filter_name, extensions);
    }
    let file: Option<PathBuf> = dialog.pick_file();

    file.map(|path| path.to_string_lossy().to_string())
}

/// Open a native file picker and return a selected background image path.
pub fn pick_background_image_path() -> Option<String> {
    pick_file_path_with_filter(
        "Image",
        &["webp", "gif", "png", "apng", "jpg", "jpeg", "bmp"],
    )
}

/// Open a native file picker and return a selected font path.
pub fn pick_font_path() -> Option<String> {
    pick_file_path_with_filter("Font", &["ttf", "otf", "ttc"])
}

pub fn pick_file_paths_with_filter(filter_name: &str, extensions: &[&str]) -> Vec<String> {
    let mut dialog = rfd::FileDialog::new();
    if !extensions.is_empty() {
        dialog = dialog.add_filter(filter_name, extensions);
    }

    dialog
        .pick_files()
        .unwrap_or_default()
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

pub fn pick_save_path_with_filter(
    filter_name: &str,
    extensions: &[&str],
    default_file_name: &str,
) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_file_name(default_file_name);
    if !extensions.is_empty() {
        dialog = dialog.add_filter(filter_name, extensions);
    }

    dialog
        .save_file()
        .map(|path| path.to_string_lossy().to_string())
}
