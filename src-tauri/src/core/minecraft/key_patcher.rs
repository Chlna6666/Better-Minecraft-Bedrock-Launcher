use std::fs::{self, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, error, info, warn};

/// 支持的可执行文件名（小写比较）
pub const VALID_EXE_NAMES: &[&str] = &["minecraft.windows.exe"];

/// 旧/新公钥（Base64 文本），长度必须相同
const OLD_ROOT_PUBLIC_KEY: &str = "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAE8ELkixyLcwlZryUQcu1TvPOmI2B7vX83ndnWRUaXm74wFfa5f/lwQNTfrLVHa2PmenpGI6JhIMUJaWZrjmMj90NoKNFSNBuKdm8rYiXsfaz3K36x/1U26HpG0ZxK/V1V";
const NEW_ROOT_PUBLIC_KEY: &str = "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAECRXueJeTDqNRRgJi/vlRufByu/2G0i2Ebt6YMar5QX/R0DIIyrJMcUpruK4QveTfJSTp3Shlq4Gk34cD/4GUWwkv0DVuzeuB+tXija7HBxii03NHDbPAD0AKnLr2wdAp";

/// 补丁错误类型
#[derive(Debug)]
pub enum PatchError {
    Io(io::Error),           // 底层 IO 错误
    InvalidExeName,          // 文件名不是受支持的 exe
    KeyNotFound,             // 在文件中找不到旧公钥
    BackupFailed(io::Error), // 备份创建失败
}

/// 补丁操作的结果
#[derive(Debug)]
pub enum PatchResult {
    Patched(PathBuf), // 成功应用补丁，值为备份文件路径
    NotApplicable,    // 未找到适用的文件，无需操作
}


impl From<io::Error> for PatchError {
    fn from(err: io::Error) -> Self {
        PatchError::Io(err)
    }
}

/// 计算 KMP 的 LPS 表
fn compute_lps(pattern: &[u8]) -> Vec<usize> {
    let m = pattern.len();
    debug!("compute_lps: 模式长度 = {}", m);

    let mut lps = vec![0usize; m];
    let mut len: usize = 0;
    let mut i = 1usize;
    while i < m {
        if pattern[i] == pattern[len] {
            len += 1;
            lps[i] = len;
            i += 1;
        } else {
            if len != 0 {
                len = lps[len - 1];
            } else {
                lps[i] = 0;
                i += 1;
            }
        }
    }

    debug!(
        "compute_lps: 完成（前 10 项） = {:?}",
        &lps[..std::cmp::min(10, lps.len())]
    );
    lps
}

/// 在字节数组中用 KMP 查找 needle 的偏移（返回起始字节索引）
fn find_key_offset_in_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    debug!(
        "find_key_offset_in_bytes: haystack.len() = {}, needle.len() = {}",
        haystack.len(),
        needle.len()
    );

    if needle.is_empty() || haystack.len() < needle.len() {
        warn!("find_key_offset_in_bytes: 模式为空或被搜索内容长度不足");
        return None;
    }
    let lps = compute_lps(needle);
    let mut i = 0usize;
    let mut j = 0usize;

    while i < haystack.len() {
        if haystack[i] == needle[j] {
            i += 1;
            j += 1;
            if j == needle.len() {
                let found = i - j;
                debug!("find_key_offset_in_bytes: 在偏移 0x{:X} 处找到模式", found);
                return Some(found);
            }
        } else if j != 0 {
            j = lps[j - 1];
        } else {
            i += 1;
        }
    }

    debug!("find_key_offset_in_bytes: 未找到模式");
    None
}

/// 在磁盘上创建备份文件，返回备份路径
fn backup_file(original: &Path) -> Result<PathBuf, io::Error> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let file_name = original
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("backup");
    let parent = original.parent().unwrap_or_else(|| Path::new("."));
    let bak_name = format!("{}.bak.{}", file_name, now);
    let bak_path = parent.join(bak_name);

    info!(
        "backup_file: 为 '{}' 创建备份 -> '{}'",
        original.display(),
        bak_path.display()
    );

    fs::copy(original, &bak_path)?;
    debug!("backup_file: 备份复制成功");

    Ok(bak_path)
}

/// 判断路径文件名是否为受支持的 exe 之一（不区分大小写）
pub fn is_valid_exe_path(path: &Path) -> bool {
    match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => {
            let lower = name.to_ascii_lowercase();
            debug!("is_valid_exe_path: 原文件名='{}' 小写='{}'", name, lower);
            VALID_EXE_NAMES.contains(&lower.as_str())
        }
        None => {
            debug!("is_valid_exe_path: 路径 '{}' 没有文件名", path.display());
            false
        }
    }
}

/// 在目录中查找受支持的 exe（第一个匹配的文件）
pub fn find_exe_in_dir(dir: &Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        warn!("find_exe_in_dir: 提供的路径不是目录: {}", dir.display());
        return None;
    }
    info!("find_exe_in_dir: 在目录中查找: {}", dir.display());

    let entries = fs::read_dir(dir).ok()?;
    for res in entries {
        match res {
            Ok(entry) => {
                let p = entry.path();
                if p.is_file() {
                    debug!("find_exe_in_dir: 发现文件: {}", p.display());
                    if is_valid_exe_path(&p) {
                        info!("find_exe_in_dir: 匹配到可执行文件: {}", p.display());
                        return Some(p);
                    }
                }
            }
            Err(e) => {
                warn!("find_exe_in_dir: 读取目录条目时出错: {}", e);
            }
        }
    }

    debug!("find_exe_in_dir: 在 {} 中未找到匹配的 exe", dir.display());
    None
}

/// 在原地补丁文件（先备份，再写入），成功返回备份路径
pub fn patch_file_in_place(exe_path: &Path) -> Result<PathBuf, PatchError> {
    info!(
        "patch_file_in_place: 尝试对 '{}' 应用补丁",
        exe_path.display()
    );

    if !is_valid_exe_path(exe_path) {
        error!(
            "patch_file_in_place: 无效的可执行文件名: {}",
            exe_path.display()
        );
        return Err(PatchError::InvalidExeName);
    }

    debug_assert_eq!(
        OLD_ROOT_PUBLIC_KEY.len(),
        NEW_ROOT_PUBLIC_KEY.len(),
        "old/new key length mismatch"
    );

    info!(
        "patch_file_in_place: 读取文件到内存: {}",
        exe_path.display()
    );
    let file_bytes = fs::read(exe_path).map_err(|e| {
        error!(
            "patch_file_in_place: 无法读取文件 '{}': {}",
            exe_path.display(),
            e
        );
        PatchError::Io(e)
    })?;
    debug!("patch_file_in_place: 文件大小 = {} 字节", file_bytes.len());

    let old_bytes = OLD_ROOT_PUBLIC_KEY.as_bytes();
    let new_bytes = NEW_ROOT_PUBLIC_KEY.as_bytes();

    let offset = match find_key_offset_in_bytes(&file_bytes, old_bytes) {
        Some(off) => {
            info!("patch_file_in_place: 在偏移 0x{:X} 处定位到旧公钥", off);
            off
        }
        None => {
            info!(
                "patch_file_in_place: 在 '{}' 中未发现旧公钥",
                exe_path.display()
            );
            return Err(PatchError::KeyNotFound);
        }
    };

    info!("patch_file_in_place: 在写入前创建备份");
    let bak = match backup_file(exe_path) {
        Ok(b) => {
            info!("patch_file_in_place: 备份已创建于 '{}'", b.display());
            b
        }
        Err(e) => {
            error!(
                "patch_file_in_place: 备份创建失败 '{}': {}",
                exe_path.display(),
                e
            );
            return Err(PatchError::BackupFailed(e));
        }
    };

    info!(
        "patch_file_in_place: 以写入模式打开文件: {}",
        exe_path.display()
    );
    let mut f = OpenOptions::new().write(true).open(exe_path).map_err(|e| {
        error!(
            "patch_file_in_place: 无法以写入模式打开 '{}': {}",
            exe_path.display(),
            e
        );
        PatchError::Io(e)
    })?;

    debug!("patch_file_in_place: 定位到偏移 0x{:X}", offset);
    f.seek(SeekFrom::Start(offset as u64)).map_err(|e| {
        error!("patch_file_in_place: seek 操作失败: {}", e);
        PatchError::Io(e)
    })?;

    debug!(
        "patch_file_in_place: 在偏移 0x{:X} 写入 {} 字节",
        offset,
        new_bytes.len()
    );
    f.write_all(new_bytes).map_err(|e| {
        error!("patch_file_in_place: 写入失败: {}", e);
        PatchError::Io(e)
    })?;

    f.flush().map_err(|e| {
        error!("patch_file_in_place: flush 失败: {}", e);
        PatchError::Io(e)
    })?;

    info!(
        "patch_file_in_place: 对 '{}' 的补丁已成功应用",
        exe_path.display()
    );
    Ok(bak)
}

/// 支持传入目录或文件路径。
/// 若传入目录，则在目录内查找受支持的 exe 并应用补丁。
/// 若未找到适用文件，则返回 Ok(PatchResult::NotApplicable)。
pub fn patch_path(path: &Path) -> Result<PatchResult, PatchError> {
    info!("patch_path: 入口，路径='{}'", path.display());

    let exe_to_patch = if path.is_dir() {
        info!("patch_path: 提供的是目录，正在目录中查找 exe");
        find_exe_in_dir(path)
    } else {
        // 如果是文件，仅当它是有效 exe 时才处理
        if is_valid_exe_path(path) {
            Some(path.to_path_buf())
        } else {
            None
        }
    };

    if let Some(exe_path) = exe_to_patch {
        if !exe_path.exists() {
            error!(
                "patch_path: 解析得到的 exe 路径不存在: {}",
                exe_path.display()
            );
            return Err(PatchError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "path does not exist",
            )));
        }

        debug!("patch_path: 解析到 exe 路径 '{}'", exe_path.display());
        // 调用原地补丁函数，并映射结果
        patch_file_in_place(&exe_path).map(PatchResult::Patched)
    } else {
        // 在目录中未找到，或提供的文件不是有效目标
        info!(
            "patch_path: 在 '{}' 中未找到可应用补丁的 exe，跳过操作。",
            path.display()
        );
        Ok(PatchResult::NotApplicable)
    }
}
