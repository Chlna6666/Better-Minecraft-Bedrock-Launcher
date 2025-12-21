// core/downloads/md5.rs
use md5::Context;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

// 你需要在 CoreError 中添加 ChecksumMismatch 变体或相应处理

/// 返回十六进制小写 MD5 字符串
pub async fn compute_md5<P: AsRef<Path>>(path: P) -> Result<String, std::io::Error> {
    let path_ref = path.as_ref();
    debug!("开始计算文件 MD5: {}", path_ref.display());

    let mut file = File::open(path_ref).await?;
    let mut buf = vec![0u8; 8 * 1024];
    let mut ctx = Context::new();

    let mut total_bytes = 0;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        ctx.consume(&buf[..n]);
        total_bytes += n;
    }

    let digest = ctx.compute();
    let result = format!("{:x}", digest);

    debug!(
        "文件 {} 的 MD5 计算完成: {} (已读取 {} 字节)",
        path_ref.display(),
        result,
        total_bytes
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