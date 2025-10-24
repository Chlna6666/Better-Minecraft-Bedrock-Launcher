// core/downloads/md5.rs
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use md5::Context;
use std::path::Path;
use crate::result::CoreError; // 你需要在 CoreError 中添加 ChecksumMismatch 变体或相应处理

/// 返回十六进制小写 MD5 字符串
pub async fn compute_md5<P: AsRef<Path>>(path: P) -> Result<String, std::io::Error> {
    let mut file = File::open(path).await?;
    let mut buf = vec![0u8; 8 * 1024];
    let mut ctx = Context::new();

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        ctx.consume(&buf[..n]);
    }

    let digest = ctx.compute();
    Ok(format!("{:x}", digest))
}

/// 校验文件 md5 是否等于 expected（忽略大小写）
pub async fn verify_md5<P: AsRef<Path>>(path: P, expected: &str) -> Result<bool, std::io::Error> {
    let got = compute_md5(path).await?;
    Ok(got.eq_ignore_ascii_case(expected))
}
