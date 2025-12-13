// src/downloads/single.rs
use crate::downloads::md5::verify_md5;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{finish_task, is_cancelled, set_total, update_progress};
use futures_util::StreamExt;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Duration};
use tracing::debug;

pub async fn download_file(
    client: reqwest::Client,
    task_id: &str,
    url: &str,
    dest: impl AsRef<Path>,
    md5_expected: Option<&str>,
) -> Result<CoreResult<()>, CoreError> {
    let mut retry = 0u8;
    loop {
        debug!(
            "开始下载：task={} url={}，目标={:?}，重试={}，md5={:?}",
            task_id,
            url,
            dest.as_ref(),
            retry,
            md5_expected
        );
        let resp = client.get(url).send().await;
        match resp {
            Ok(resp) => {
                let resp = resp.error_for_status()?;
                let total = resp.content_length();
                // inform task manager total if known
                if let Some(t) = total {
                    set_total(&task_id, Some(t));
                } else {
                    set_total(&task_id, None);
                }

                // 把文件 I/O 错误映射为 CoreError::Io
                let mut file = File::create(dest.as_ref())
                    .await
                    .map_err(|e| CoreError::Io(e))?;
                let mut stream = resp.bytes_stream();

                // track overall bytes
                while let Some(chunk) = stream.next().await {
                    let c = chunk.map_err(|e| CoreError::Request(e))?;
                    // 写入错误同样映射为 CoreError::Io
                    file.write_all(&c).await.map_err(|e| CoreError::Io(e))?;

                    // 更新任务进度：传增量，total 可选
                    update_progress(&task_id, c.len() as u64, total, Some("downloading"));

                    if is_cancelled(&task_id) {
                        debug!("检测到取消：停止下载（task={}, url={}）", task_id, url);
                        // 这里可以选择删除临时文件或保留
                        // 标记任务状态
                        finish_task(&task_id, "cancelled", Some("user cancelled".to_string()));
                        return Ok(CoreResult::Cancelled);
                    }
                }

                file.flush().await.map_err(|e| CoreError::Io(e))?;

                // 最后一条进度：把 done 设置为 total（如果 known）
                if let Some(t) = total {
                    set_total(&task_id, Some(t));
                }

                // md5 校验（如果提供）
                if let Some(expected) = md5_expected {
                    debug!("开始 MD5 校验：文件={:?}，期望={}", dest.as_ref(), expected);
                    match verify_md5(dest.as_ref(), expected).await {
                        Ok(true) => {
                            debug!("MD5 校验通过：{:?}", dest.as_ref());
                        }
                        Ok(false) => {
                            debug!("MD5 不匹配：文件={:?}，期望={}", dest.as_ref(), expected);
                            finish_task(&task_id, "error", Some("md5 mismatch".to_string()));
                            return Err(CoreError::ChecksumMismatch(format!(
                                "md5 mismatch for {:?}, expected {}",
                                dest.as_ref(),
                                expected
                            )));
                        }
                        Err(e) => {
                            debug!("计算 MD5 失败：文件={:?}，错误={}", dest.as_ref(), e);
                            finish_task(&task_id, "error", Some("md5 compute failed".to_string()));
                            return Err(CoreError::Io(e));
                        }
                    }
                }

                finish_task(&task_id, "completed", None);
                return Ok(CoreResult::Success(()));
            }
            Err(e) if retry < 3 => {
                retry += 1;
                debug!("下载出错，准备重试（第 {} 次）：{}", retry, e);
                // 标记错误信息（但不结束任务）
                finish_task(&task_id, "error", Some(format!("download error: {}", e)));
                sleep(Duration::from_secs(1)).await;
            }
            Err(e) => {
                debug!("下载最终失败：{}", e);
                finish_task(&task_id, "error", Some(format!("{}", e)));
                return Err(CoreError::Request(e));
            }
        }
    }
}
