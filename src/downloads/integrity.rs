use crate::result::CoreError;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use tokio::task;
use tracing::debug;
use zip::ZipArchive;

pub(crate) fn is_zip_like_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_ascii_lowercase();
    [".appx", ".zip", ".mcpack", ".mcworld"]
        .iter()
        .any(|extension| {
            file_name.ends_with(extension)
                || file_name.ends_with(format!("{extension}.tmp").as_str())
        })
}

pub(crate) fn should_verify_zip_during_download(path: &Path) -> bool {
    if is_appx_download_path(path) {
        return false;
    }

    is_zip_like_path(path)
}

pub(crate) fn is_appx_download_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|file_name| file_name.to_str())
        .map(|file_name| file_name.to_ascii_lowercase())
        .is_some_and(|file_name| file_name.ends_with(".appx") || file_name.ends_with(".appx.tmp"))
}

pub(crate) fn has_zip_header(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..4),
        Some(b"PK\x03\x04" | b"PK\x05\x06" | b"PK\x07\x08")
    )
}

fn verify_zip_archive_reader<R: Read + Seek>(mut archive: ZipArchive<R>) -> Result<(), String> {
    let mut buffer = [0_u8; 128 * 1024];
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 zip 条目 #{index} 失败: {error}"))?;
        if entry.is_dir() {
            continue;
        }

        let name = entry
            .name()
            .map_err(|error| format!("读取 zip 条目名称失败 #{index}: {error}"))?
            .to_string();
        loop {
            match entry.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    return Err(format!("校验 zip 条目失败: {name} ({error})"));
                }
            }
        }
    }

    Ok(())
}

pub async fn verify_zip_integrity(path: &Path) -> Result<(), CoreError> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || {
        let file = File::open(&path)
            .map_err(|error| format!("打开 zip 文件失败: {} ({error})", path.display()))?;
        let archive = ZipArchive::new(file)
            .map_err(|error| format!("创建 ZipArchive 失败: {} ({error})", path.display()))?;
        verify_zip_archive_reader(archive)
    })
    .await
    .map_err(CoreError::Join)?
    .map_err(CoreError::ChecksumMismatch)
}

pub async fn verify_download_integrity(
    path: &Path,
    md5_expected: Option<&str>,
) -> Result<(), CoreError> {
    if let Some(expected) = md5_expected {
        let expected = expected.trim();
        if !crate::downloads::md5::is_md5_digest(expected) {
            debug!(
                "skip non-md5 checksum value for path={} value={}",
                path.display(),
                expected
            );
        } else {
            match crate::downloads::md5::verify_md5(path, expected).await {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    return Err(CoreError::ChecksumMismatch(format!(
                        "MD5 mismatch for {}",
                        path.display()
                    )));
                }
                Err(error) => return Err(CoreError::Io(error)),
            }
        }
    }

    if should_verify_zip_during_download(path) {
        verify_zip_integrity(path).await?;
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::verify_download_integrity;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_download_path(file_name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("bmcbl-download-integrity-{stamp}-{file_name}"))
    }

    #[tokio::test]
    async fn appx_tmp_skips_download_time_zip_entry_verification() {
        let path = temp_download_path("broken.appx.tmp");
        tokio::fs::write(&path, b"not a zip")
            .await
            .expect("failed to create test appx temp file");

        let result = verify_download_integrity(&path, None).await;
        let _ = tokio::fs::remove_file(&path).await;

        assert!(
            result.is_ok(),
            "appx download should not be rejected by download-time zip verification: {result:?}"
        );
    }

    #[tokio::test]
    async fn zip_tmp_keeps_download_time_zip_entry_verification() {
        let path = temp_download_path("broken.zip.tmp");
        tokio::fs::write(&path, b"not a zip")
            .await
            .expect("failed to create test zip temp file");

        let result = verify_download_integrity(&path, None).await;
        let _ = tokio::fs::remove_file(&path).await;

        assert!(
            result.is_err(),
            "zip download should still be rejected by download-time zip verification"
        );
    }
}
