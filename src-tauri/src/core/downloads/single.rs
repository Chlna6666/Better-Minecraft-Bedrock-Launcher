use std::path::Path;
use futures_util::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Instant, Duration};
use tauri::AppHandle;
use crate::core::downloads::cancel::is_cancelled;
use crate::core::result::{CoreError, CoreResult};
use crate::core::minecraft::utils::{emit_progress, format_eta, format_speed};
use serde_json::json;

pub async fn download_file(
    client: reqwest::Client,
    url: &str,
    dest: impl AsRef<Path>,
    app: AppHandle,
) -> Result<CoreResult<()>, CoreError> {
    let mut retry = 0;
    loop {
        let resp = client.get(url).send().await;
        match resp {
            Ok(resp) => {
                let resp = resp.error_for_status()?;
                let total = resp.content_length().unwrap_or(0);
                let mut file = File::create(dest.as_ref()).await?;
                let mut downloaded = 0u64;
                let start = Instant::now();
                let mut last_emit = start;
                let mut stream = resp.bytes_stream();

                while let Some(chunk) = stream.next().await {
                    if is_cancelled() {
                        return Ok(CoreResult::Cancelled);
                    }
                    let c = chunk?;
                    file.write_all(&c).await?;
                    downloaded += c.len() as u64;

                    if last_emit.elapsed() >= Duration::from_secs(1) || downloaded == total {
                        let elapsed = start.elapsed().as_secs_f64();
                        emit_progress(
                            &app,
                            downloaded,
                            Some(total),
                            Some(&format_speed(downloaded, elapsed)),
                            Some(&format_eta(Some(total), downloaded, elapsed)),
                            Some(json!({"stage":"downloading"})),
                        ).await;
                        last_emit = Instant::now();
                    }
                }
                file.flush().await?;
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