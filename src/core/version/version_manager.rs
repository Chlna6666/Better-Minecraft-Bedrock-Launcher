use crate::core::minecraft::appx::utils::{
    find_any_game_executable_in_dir, get_executable_product_version,
    get_manifest_identity_from_dir_blocking,
};
use crate::core::version::launch_versions::LaunchVersionEntry;
use anyhow::{Context as _, Result};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::fs::DirEntry;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
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

fn read_appx_version_entry(entry: DirEntry, root: &Path) -> Option<LaunchVersionEntry> {
    let file_type = match entry.file_type() {
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

    let manifest_result = get_manifest_identity_from_dir_blocking(&path);
    let pe_result = get_executable_product_version(&executable_path);

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
        Ok(Some(version)) => Some(version),
        Ok(None) => None,
        Err(error) => {
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
        custom_icon_path: crate::core::version::icons::custom_version_icon_path(&path)
            .map(|icon_path| Arc::<str>::from(icon_path.to_string_lossy().into_owned())),
    })
}

fn absolute_display_path(root: &Path, path: &Path) -> String {
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }

    root.join(path).to_string_lossy().into_owned()
}

pub async fn get_appx_version_list(folder: &Path) -> Result<Vec<LaunchVersionEntry>> {
    let folder = folder.to_path_buf();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("bmcbl-version-scan".to_string())
        .spawn(move || {
            let versions = get_appx_version_list_blocking(&folder);
            if sender.send(versions).is_err() {
                debug!("版本扫描结果接收端已关闭");
            }
        });
    thread.context("启动版本扫描线程失败")?;

    receiver.await.context("版本扫描线程异常退出")?
}

fn get_appx_version_list_blocking(folder: &Path) -> Result<Vec<LaunchVersionEntry>> {
    let start = Instant::now();
    debug!("开始读取目录: {}", folder.display());

    let read_dir = std::fs::read_dir(folder)
        .with_context(|| format!("读取版本目录失败: {}", folder.display()))?;

    let entries = read_dir
        .filter_map(|entry| match entry {
            Ok(entry) => Some(Ok(entry)),
            Err(error) => Some(Err(error)),
        })
        .collect::<std::io::Result<Vec<_>>>()
        .context("遍历版本目录失败")?;
    let concurrency = num_cpus::get().clamp(2, 4);
    let versions = match rayon::ThreadPoolBuilder::new()
        .num_threads(concurrency)
        .thread_name(|index| format!("bmcbl-version-parse-{index}"))
        .build()
    {
        Ok(pool) => pool.install(|| {
            entries
                .into_par_iter()
                .filter_map(|entry| read_appx_version_entry(entry, folder))
                .collect::<Vec<_>>()
        }),
        Err(error) => {
            debug!("创建版本解析线程池失败，改用顺序解析: {error}");
            entries
                .into_iter()
                .filter_map(|entry| read_appx_version_entry(entry, folder))
                .collect::<Vec<_>>()
        }
    };

    debug!(
        "get_appx_version_list 完成，用时: {:?}, 并发度: {}, 条目数: {}",
        start.elapsed(),
        concurrency,
        versions.len()
    );
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::get_appx_version_list;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn version_scan_does_not_wait_for_tokio_blocking_pool() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_time()
            .build()
            .expect("test runtime should build");
        let (blocking_started_sender, blocking_started_receiver) = mpsc::channel();
        let (release_blocking_sender, release_blocking_receiver) = mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            blocking_started_sender
                .send(())
                .expect("test should observe blocking worker startup");
            release_blocking_receiver
                .recv()
                .expect("test should release blocking worker");
        });
        blocking_started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking worker should be occupied");

        let test_dir = std::env::temp_dir().join(format!(
            "bmcbl-version-scan-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&test_dir).expect("test directory should be created");
        let result = runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(500), get_appx_version_list(&test_dir)).await
        });

        release_blocking_sender
            .send(())
            .expect("blocking worker should be released");
        runtime
            .block_on(blocker)
            .expect("blocking worker should finish");
        std::fs::remove_dir_all(&test_dir).expect("test directory should be removed");

        assert!(
            result
                .expect("version scan should bypass blocking pool")
                .expect("version scan should complete")
                .is_empty()
        );
    }
}
