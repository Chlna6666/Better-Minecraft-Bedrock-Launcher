use crate::core::minecraft::appx::utils::{
    find_any_game_executable_in_dir, get_executable_product_version, get_manifest_identity_from_dir,
};
use crate::core::version::launch_versions::LaunchVersionEntry;
use futures::StreamExt;
use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::read_dir;
use tokio_stream::wrappers::ReadDirStream;
use tracing::debug;

fn next_version_number(version: &str, cursor: &mut usize) -> Option<u64> {
    let bytes = version.as_bytes();
    let len = bytes.len();

    while *cursor < len {
        let byte = bytes[*cursor];
        if byte.is_ascii_digit() {
            break;
        }
        *cursor += 1;
    }

    if *cursor >= len {
        return None;
    }

    let start = *cursor;
    while *cursor < len && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }

    version[start..*cursor].parse::<u64>().ok()
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let mut left_cursor = 0;
    let mut right_cursor = 0;

    loop {
        let left_number = next_version_number(left, &mut left_cursor);
        let right_number = next_version_number(right, &mut right_cursor);

        match (left_number, right_number) {
            (Some(left_number), Some(right_number)) => match left_number.cmp(&right_number) {
                Ordering::Equal => continue,
                non_equal => return non_equal,
            },
            (Some(left_number), None) => {
                return if left_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                };
            }
            (None, Some(right_number)) => {
                return if right_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Less
                };
            }
            (None, None) => return Ordering::Equal,
        }
    }
}

pub(crate) fn is_win32_version(version: &str) -> bool {
    let mut cursor = 0;
    let major = next_version_number(version, &mut cursor).unwrap_or(0);
    let minor = next_version_number(version, &mut cursor).unwrap_or(0);
    let build = next_version_number(version, &mut cursor).unwrap_or(0);

    if major == 1 && minor == 21 && build >= 12201 {
        return true;
    }

    const THRESHOLD: &str = "1.21.12000.20";
    compare_versions(version, THRESHOLD) != Ordering::Less
}

fn determine_kind_from_version(version: &str) -> &'static str {
    if is_win32_version(version) {
        "GDK"
    } else {
        "UWP"
    }
}

async fn read_appx_version_entry(
    entry: tokio::fs::DirEntry,
    root: &Path,
) -> Option<LaunchVersionEntry> {
    let file_type = match entry.file_type().await {
        Ok(file_type) => file_type,
        Err(error) => {
            debug!("获取文件类型失败: {}", error);
            return None;
        }
    };

    if !file_type.is_dir() {
        debug!("跳过非目录: {:?}", entry.path());
        return None;
    }

    let path = entry.path();
    let folder = match entry.file_name().into_string() {
        Ok(folder) => folder,
        Err(_) => return None,
    };

    let display_path = absolute_display_path(root, &path);
    let executable_path = match find_any_game_executable_in_dir(&path) {
        Some(executable_path) => executable_path,
        None => {
            debug!("跳过无效版本目录（缺少 exe）: {}", path.display());
            return None;
        }
    };

    let manifest_future = get_manifest_identity_from_dir(&path);
    let pe_executable_path = executable_path.clone();
    let pe_future =
        tokio::task::spawn_blocking(move || get_executable_product_version(&pe_executable_path));
    let (manifest_result, pe_result) = tokio::join!(manifest_future, pe_future);

    let (name, manifest_version) = match manifest_result {
        Ok(identity) => identity,
        Err(error) => {
            debug!(
                "manifest 解析失败，跳过目录: dir={}, exe={}, error={}",
                path.display(),
                executable_path.display(),
                error
            );
            return None;
        }
    };

    let pe_version = match pe_result {
        Ok(Ok(Some(version))) => Some(version),
        Ok(Ok(None)) => None,
        Ok(Err(error)) => {
            debug!(
                "PE 版本解析失败，使用 manifest 版本: dir={}, exe={}, identity={}, manifest_version={}, error={}",
                path.display(),
                executable_path.display(),
                name,
                manifest_version,
                error
            );
            None
        }
        Err(error) => {
            debug!(
                "PE 解析任务失败，使用 manifest 版本: dir={}, exe={}, identity={}, manifest_version={}, error={}",
                path.display(),
                executable_path.display(),
                name,
                manifest_version,
                error
            );
            None
        }
    };

    let manifest_version: Arc<str> = Arc::from(manifest_version);
    let product_version_log = pe_version.as_deref().unwrap_or("-");
    let version_kind_source = pe_version.as_deref().unwrap_or(manifest_version.as_ref());
    let kind = determine_kind_from_version(version_kind_source);
    debug!(
        "版本条目解析成功: dir={}, exe={}, identity={}, manifest_version={}, product_version={}, kind={}",
        path.display(),
        executable_path.display(),
        name,
        manifest_version,
        product_version_log,
        kind
    );

    let version = match pe_version {
        Some(product_version) => Arc::<str>::from(product_version),
        None => manifest_version.clone(),
    };

    Some(LaunchVersionEntry {
        folder: Arc::<str>::from(folder),
        name: Arc::<str>::from(name),
        version,
        manifest_version,
        path: Arc::<str>::from(display_path),
        kind: Arc::<str>::from(kind),
    })
}

fn absolute_display_path(root: &Path, path: &Path) -> String {
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }

    root.join(path).to_string_lossy().into_owned()
}

pub async fn get_appx_version_list(folder: &Path) -> Vec<LaunchVersionEntry> {
    let start = Instant::now();
    debug!("开始读取目录: {}", folder.display());

    let read_dir = match read_dir(folder).await {
        Ok(read_dir) => read_dir,
        Err(error) => {
            debug!("读取目录失败: {}", error);
            return Vec::new();
        }
    };

    let concurrency = num_cpus::get().clamp(2, 4);
    let root = Arc::new(folder.to_path_buf());
    let versions = ReadDirStream::new(read_dir)
        .filter_map(|entry| async move {
            match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    debug!("遍历目录项出错: {}", error);
                    None
                }
            }
        })
        .map(move |entry| {
            let root = Arc::clone(&root);
            async move { read_appx_version_entry(entry, root.as_path()).await }
        })
        .buffer_unordered(concurrency)
        .filter_map(|entry| async move { entry })
        .collect::<Vec<_>>()
        .await;

    debug!(
        "get_appx_version_list 完成，用时: {:?}, 并发度: {}, 条目数: {}",
        start.elapsed(),
        concurrency,
        versions.len()
    );
    versions
}
