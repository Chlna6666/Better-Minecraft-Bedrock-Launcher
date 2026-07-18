use std::path::{Path, PathBuf};

/// Returns the custom icon stored beside a managed version, when present.
pub fn custom_version_icon_path(version_directory: &Path) -> Option<PathBuf> {
    let icon_path = version_directory.join("icon.png");
    icon_path.is_file().then_some(icon_path)
}

/// Copies a selected version icon into a game directory as `icon.png`.
///
/// # Errors
///
/// Returns an I/O error when the source cannot be read or the target directory
/// cannot be created or written.
pub async fn copy_version_icon(
    source_icon: &Path,
    game_directory: &Path,
) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(game_directory).await?;
    let destination = game_directory.join("icon.png");
    tokio::fs::copy(source_icon, &destination).await?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::{copy_version_icon, custom_version_icon_path};
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temporary_directory(test_name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("bmcbl-version-icon-{test_name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("temporary directory should be created");
        directory
    }

    #[test]
    fn custom_version_icon_path_returns_existing_icon_png() {
        let directory = temporary_directory("existing");
        let expected = directory.join("icon.png");
        fs::write(&expected, [0x89, b'P', b'N', b'G']).expect("test icon should be written");

        assert_eq!(custom_version_icon_path(&directory), Some(expected.clone()));

        fs::remove_dir_all(directory).expect("temporary directory should be removed");
    }

    #[test]
    fn custom_version_icon_path_returns_none_when_icon_is_missing() {
        let directory = temporary_directory("missing");

        assert_eq!(custom_version_icon_path(&directory), None);

        fs::remove_dir_all(directory).expect("temporary directory should be removed");
    }

    #[tokio::test]
    async fn copy_version_icon_writes_icon_png_to_game_directory() {
        let source_directory = temporary_directory("source");
        let game_directory = temporary_directory("game");
        let source = source_directory.join("selected.png");
        let expected = game_directory.join("icon.png");
        fs::write(&source, [0x89, b'P', b'N', b'G']).expect("source icon should be written");

        copy_version_icon(&source, &game_directory)
            .await
            .expect("PNG should be copied");

        assert_eq!(
            fs::read(expected).expect("destination should exist"),
            [0x89, b'P', b'N', b'G']
        );

        fs::remove_dir_all(source_directory).expect("source directory should be removed");
        fs::remove_dir_all(game_directory).expect("game directory should be removed");
    }
}
