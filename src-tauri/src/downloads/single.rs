use std::path::Path;
use futures_util::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Duration};
use serde_json::json;
use std::sync::Arc;
use tracing::debug;
use crate::downloads::cancel;
use crate::downloads::md5::verify_md5;
use crate::result::{CoreError, CoreResult};

pub async fn download_file(
    client: reqwest::Client,
    url: &str,
    dest: impl AsRef<Path>,
    md5_expected: Option<&str>,
    cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<CoreResult<()>, CoreError> {
    let mut retry = 0u8;
    loop {
        debug!("开始下载：url={}，目标={:?}，重试={}，md5={:?}", url, dest.as_ref(), retry, md5_expected);
        let resp = client.get(url).send().await;
        match resp {
            Ok(resp) => {
                let resp = resp.error_for_status()?;
                let total = resp.content_length().unwrap_or(0);

                // 把文件 I/O 错误映射为 CoreError::Io（不可试图用 reqwest::Error::new）
                let mut file = File::create(dest.as_ref())
                    .await
                    .map_err(|e| CoreError::Io(e))?;
                let mut stream = resp.bytes_stream();

                let mut progress = crate::progress::download_progress::DownloadProgress::new(total);

                while let Some(chunk) = stream.next().await {
                    let c = chunk.map_err(|e| CoreError::Request(e))?;
                    // 写入错误同样映射为 CoreError::Io
                    file.write_all(&c).await.map_err(|e| CoreError::Io(e))?;
                    progress.update(c.len());

                    // 仅打印本次写入大小，避免调用不存在的方法
                    debug!("写入 {} 字节", c.len());

                    if progress.should_emit() {
                        let _ = crate::progress::download_progress::report_progress(&mut progress, json!({"stage": "downloading"})).await;
                    }

                    // 支持 per-task 取消
                    if cancel::is_cancelled(cancel_flag.as_ref()) {
                        debug!("检测到取消：停止下载（url={}）", url);
                        // 你可以在这里删除临时文件或保留（取决于业务）
                        return Ok(CoreResult::Cancelled);
                    }
                }

                file.flush().await.map_err(|e| CoreError::Io(e))?;

                // 最后一条进度
                let _ = crate::progress::download_progress::report_progress(&mut progress, json!({"stage": "completed"})).await;

                // md5 校验（如果提供）
                if let Some(expected) = md5_expected {
                    debug!("开始 MD5 校验：文件={:?}，期望={}", dest.as_ref(), expected);
                    match verify_md5(dest.as_ref(), expected).await {
                        Ok(true) => {
                            debug!("MD5 校验通过：{:?}", dest.as_ref());
                        }
                        Ok(false) => {
                            debug!("MD5 不匹配：文件={:?}，期望={}", dest.as_ref(), expected);
                            return Err(CoreError::ChecksumMismatch(format!(
                                "md5 mismatch for {:?}, expected {}",
                                dest.as_ref(),
                                expected
                            )));
                        }
                        Err(e) => {
                            debug!("计算 MD5 失败：文件={:?}，错误={}", dest.as_ref(), e);
                            return Err(CoreError::Io(e));
                        }
                    }
                }

                return Ok(CoreResult::Success(()));
            }
            Err(e) if retry < 3 => {
                retry += 1;
                debug!("下载出错，准备重试（第 {} 次）：{}", retry, e);
                sleep(Duration::from_secs(1)).await;
            }
            Err(e) => {
                debug!("下载最终失败：{}", e);
                return Err(CoreError::Request(e));
            }
        }
    }
}
