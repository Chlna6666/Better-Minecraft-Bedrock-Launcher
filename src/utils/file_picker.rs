use gpui::Window;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tracing::warn;

const FILE_DIALOG_DEBOUNCE: Duration = Duration::from_millis(350);

static NATIVE_FILE_DIALOG_GATE: LazyLock<Mutex<NativeFileDialogGate>> =
    LazyLock::new(|| Mutex::new(NativeFileDialogGate::default()));

#[derive(Default)]
struct NativeFileDialogGate {
    open: bool,
    last_closed_at: Option<Instant>,
}

impl NativeFileDialogGate {
    fn try_open(&mut self, now: Instant) -> bool {
        if self.open
            || self.last_closed_at.is_some_and(|last_closed_at| {
                now.saturating_duration_since(last_closed_at) < FILE_DIALOG_DEBOUNCE
            })
        {
            return false;
        }

        self.open = true;
        true
    }

    fn close(&mut self, now: Instant) {
        self.open = false;
        self.last_closed_at = Some(now);
    }
}

struct NativeFileDialogGuard;

impl Drop for NativeFileDialogGuard {
    fn drop(&mut self) {
        native_file_dialog_gate().close(Instant::now());
    }
}

fn native_file_dialog_gate() -> MutexGuard<'static, NativeFileDialogGate> {
    match NATIVE_FILE_DIALOG_GATE.lock() {
        Ok(gate) => gate,
        Err(poisoned_gate) => {
            warn!("recovering poisoned native file dialog gate");
            poisoned_gate.into_inner()
        }
    }
}

fn try_open_native_file_dialog() -> Option<NativeFileDialogGuard> {
    native_file_dialog_gate()
        .try_open(Instant::now())
        .then_some(NativeFileDialogGuard)
}

/// Open a native file picker and return a selected file path.
pub fn pick_file_path_with_filter(filter_name: &str, extensions: &[&str]) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if !extensions.is_empty() {
        dialog = dialog.add_filter(filter_name, extensions);
    }
    let file: Option<PathBuf> = dialog.pick_file();

    file.map(|path| path.to_string_lossy().to_string())
}

/// Open a native directory picker and return the selected directory path.
pub fn pick_directory_path_for_window(window: &Window) -> Option<String> {
    let _dialog_guard = try_open_native_file_dialog()?;
    window.activate_window();
    rfd::FileDialog::new()
        .set_parent(window)
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Open a parent-owned native file picker, suppressing repeated dialog requests.
pub fn pick_file_path_with_filter_for_window(
    window: &Window,
    filter_name: &str,
    extensions: &[&str],
) -> Option<String> {
    let _dialog_guard = try_open_native_file_dialog()?;
    window.activate_window();

    let mut dialog = rfd::FileDialog::new().set_parent(window);
    if !extensions.is_empty() {
        dialog = dialog.add_filter(filter_name, extensions);
    }

    dialog
        .pick_file()
        .map(|path| path.to_string_lossy().to_string())
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

#[cfg(test)]
mod tests {
    use super::NativeFileDialogGate;
    use std::time::{Duration, Instant};

    #[test]
    fn native_file_dialog_gate_rejects_duplicate_and_recent_requests() {
        let mut gate = NativeFileDialogGate::default();
        let opened_at = Instant::now();

        assert!(gate.try_open(opened_at));
        assert!(!gate.try_open(opened_at));

        gate.close(opened_at);
        assert!(!gate.try_open(opened_at + Duration::from_millis(349)));
        assert!(gate.try_open(opened_at + Duration::from_millis(350)));
    }
}
