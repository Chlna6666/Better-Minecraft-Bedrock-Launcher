// core/downloads/md5.rs
use futures::channel::oneshot;
use md5::Context;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use tracing::{debug, info, warn};

const MD5_BUFFER_SIZE: usize = 1024 * 1024;

pub fn is_md5_digest(value: &str) -> bool {
    let value = value.trim();
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn compute_md5_blocking(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; MD5_BUFFER_SIZE];
    let mut ctx = Context::new();

    let mut total_bytes = 0_u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.consume(&buf[..n]);
        total_bytes = total_bytes.saturating_add(n as u64);
    }

    let digest = ctx.compute();
    Ok((format!("{:x}", digest), total_bytes))
}

/// 返回十六进制小写 MD5 字符串
pub async fn compute_md5<P: AsRef<Path>>(path: P) -> Result<String, std::io::Error> {
    let path_ref = path.as_ref();
    debug!("开始计算文件 MD5: {}", path_ref.display());

    let path_buf = path_ref.to_path_buf();
    let display_path = path_buf.display().to_string();
    let (sender, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name("bmcbl-md5".to_string())
        .spawn(move || {
            let result = compute_md5_blocking(&path_buf);
            if sender.send(result).is_err() {
                debug!("MD5 worker result receiver was dropped");
            }
        })?;

    let (result, total_bytes) = receiver
        .await
        .map_err(|_| io::Error::other(format!("MD5 worker stopped for {display_path}")))??;

    debug!(
        "文件 {} 的 MD5 计算完成: {} (已读取 {} 字节)",
        display_path, result, total_bytes
    );

    Ok(result)
}

/// 校验文件 md5 是否等于 expected（忽略大小写）
pub async fn verify_md5<P: AsRef<Path>>(path: P, expected: &str) -> Result<bool, std::io::Error> {
    let path_ref = path.as_ref();
    debug!("正在校验文件 MD5: {}", path_ref.display());

    // 注意：这里传入 &path_ref (即 &Path)，因为 compute_md5 接受 impl AsRef<Path>
    // 这样可以防止 path 被 move，从而在下面还能继续使用 path_ref 打印日志
    let got = compute_md5(path_ref).await?;

    let is_match = got.eq_ignore_ascii_case(expected);

    if is_match {
        info!("文件 MD5 校验成功: {}", path_ref.display());
    } else {
        warn!(
            "文件 MD5 不匹配: {}。期望值: {}, 实际值: {}",
            path_ref.display(),
            expected,
            got
        );
    }

    Ok(is_match)
}

#[cfg(test)]
mod tests {
    use super::verify_md5;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_md5_path(file_name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("bmcbl-md5-{stamp}-{file_name}"))
    }

    #[test]
    fn verify_md5_works_without_tokio_runtime() {
        let path = temp_md5_path("plain-thread.bin");
        std::fs::write(&path, b"bmcbl").expect("failed to create md5 test file");

        let path_for_thread = path.clone();
        let handle = std::thread::spawn(move || {
            futures::executor::block_on(verify_md5(
                &path_for_thread,
                "35b1394f952af8af4e165bc41f15c003",
            ))
        });
        let result = handle
            .join()
            .expect("md5 verification thread should not panic")
            .expect("md5 verification should not return io error");

        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) => eprintln!("failed to remove md5 test file {}: {error}", path.display()),
        }
        assert!(result);
    }
}
