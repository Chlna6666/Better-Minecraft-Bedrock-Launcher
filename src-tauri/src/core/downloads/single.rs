use std::path::Path;
use futures_util::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Duration};
use serde_json::json;
use crate::core::downloads::cancel::is_cancelled;
use crate::core::result::{CoreError, CoreResult};
use crate::progress::download_progress::{report_progress, DownloadProgress};

/// 高性能下载函数（支持新版 DownloadProgress）
pub async fn download_file(
    client: reqwest::Client,
    url: &str,
    dest: impl AsRef<Path>,
) -> Result<CoreResult<()>, CoreError> {
    let mut retry = 0;
    loop {
        let resp = client.get(url).send().await;
        match resp {
            Ok(resp) => {
                let resp = resp.error_for_status()?;
                let total = resp.content_length().unwrap_or(0);

                let mut file = File::create(dest.as_ref()).await?;
                let mut stream = resp.bytes_stream();

                let mut progress = DownloadProgress::new(total);

                while let Some(chunk) = stream.next().await {
                    let c = chunk?; // propagate IO/read error upstream
                    file.write_all(&c).await?;
                    progress.update(c.len());

                    // 如果达到节流条件，调用 report_progress（report_progress 内部会读取速度/ETA 并 mark_emitted）
                    if progress.should_emit() {
                        // 忽略发送错误，不要在此处再调用 progress.update_prev() 或 mark_emitted()
                        let _ = report_progress(&mut progress, json!({"stage": "downloading"})).await;
                    }

                    // 支持取消下载（检查放在写入与 progress 之后）
                    if is_cancelled() {
                        return Ok(CoreResult::Cancelled);
                    }
                }

                file.flush().await?;
                // 最后一条进度（也用 JSON）
                report_progress(&mut progress, json!({"stage": "completed"})).await;
                return Ok(CoreResult::Success(()));
            }
            Err(e) if retry < 3 => {
                retry += 1;
                sleep(Duration::from_secs(1)).await;
            }
            Err(e) => return Err(CoreError::Request(e)),
        }
    }
}
