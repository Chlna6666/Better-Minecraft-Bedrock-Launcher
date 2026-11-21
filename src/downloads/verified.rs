//! Downloads with an exact size and SHA-256 shared by every source.

use std::path::PathBuf;
use std::time::Duration;

pub(crate) use bmcbl_plugin_api::app::VerifiedDownload as Request;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::tasks::TaskStage;
use crate::tasks::task_manager::{
    ProgressUpdate, TaskFinishStatus, TaskOptions, TaskType, create_task, finish_task,
    is_cancelled, register_abort_handle, reset_progress, update_progress,
};

pub(crate) fn start(request: Request) -> Result<String, String> {
    validate(&request)?;
    let task = create_task(
        TaskOptions::new(
            t!("TaskTitle.download_resource_file").to_string(),
            TaskStage::READY,
        )
        .detail(request.filename.clone())
        .kind(TaskType::Transfer)
        .in_current_scope(),
    )?;
    let id = task.clone();
    match crate::tasks::runtime::spawn_download_task(task.clone(), async move {
        let result = download(Some(&id), request).await;
        if is_cancelled(&id) {
            return;
        }
        match result {
            Ok(path) => finish_task(
                &id,
                TaskFinishStatus::Completed,
                Some(path.to_string_lossy().into_owned()),
            ),
            Err(error) => finish_task(&id, TaskFinishStatus::Error, Some(error)),
        }
    }) {
        Ok(abort) => register_abort_handle(task.clone(), abort),
        Err(error) => {
            finish_task(&task, TaskFinishStatus::Error, Some(error.clone()));
            return Err(error);
        }
    }
    Ok(task)
}

pub(crate) async fn fetch(request: Request) -> Result<PathBuf, String> {
    validate(&request)?;
    download(None, request).await
}

fn validate(request: &Request) -> Result<(), String> {
    if !valid_sha256(&request.sha256)
        || !(1..=256 * 1024 * 1024).contains(&request.size)
        || request.filename.len() > 128
        || request.filename.starts_with('.')
        || !request.filename.contains('.')
        || !request
            .filename
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || !(1..=8).contains(&request.urls.len())
        || request.urls.iter().any(|url| {
            !reqwest::Url::parse(url).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        })
    {
        return Err("无效的下载来源、文件名、大小或 SHA-256".into());
    }
    Ok(())
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

async fn download(task: Option<&str>, request: Request) -> Result<PathBuf, String> {
    let directory = crate::utils::file_ops::mod_downloads_dir()
        .join("verified")
        .join(&request.sha256);
    let path = directory.join(&request.filename);
    if matches_file(&path, &request).await.unwrap_or(false) {
        return Ok(path);
    }
    let client =
        crate::http::proxy::clients::download_client().map_err(|error| error.to_string())?;
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| error.to_string())?;
    let mut last_error = String::new();
    for source in &request.urls {
        for url in crate::github::configured_download_urls(source)? {
            if let Some(task) = task {
                if is_cancelled(task) {
                    return Err("下载已取消".into());
                }
                reset_progress(task, Some(request.size), Some(TaskStage::STARTING));
            }
            let temporary =
                tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
            let output = temporary.reopen().map_err(|error| error.to_string())?;
            let result = transfer(
                &client,
                &url,
                tokio::fs::File::from_std(output),
                task,
                &request,
            )
            .await;
            match result {
                Ok(()) => {
                    if let Err(error) = temporary.persist(&path)
                        && !matches_file(&path, &request).await.unwrap_or(false)
                    {
                        return Err(error.to_string());
                    }
                    return Ok(path);
                }
                Err(error) => last_error = error,
            }
        }
    }
    Err(format!("下载来源均不可用：{last_error}"))
}

async fn matches_file(path: &std::path::Path, request: &Request) -> std::io::Result<bool> {
    let file = tokio::fs::File::open(path).await?;
    if file.metadata().await?.len() != request.size {
        return Ok(false);
    }
    let mut file = file.take(request.size + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(file.limit() == 1 && hex::encode(hash.finalize()) == request.sha256)
}

async fn transfer(
    client: &reqwest::Client,
    url: &str,
    mut output: tokio::fs::File,
    task: Option<&str>,
    request: &Request,
) -> Result<(), String> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.without_url().to_string())?;
    let mut received = 0;
    let mut hash = Sha256::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(20), response.chunk())
        .await
        .map_err(|_| "下载来源停止响应".to_string())?
        .map_err(|error| error.without_url().to_string())?
    {
        received += chunk.len() as u64;
        if received > request.size {
            return Err("下载大小超过索引声明".into());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
        hash.update(&chunk);
        if let Some(task) = task {
            update_progress(task, ProgressUpdate::advance(chunk.len() as u64));
        }
    }
    if received != request.size || hex::encode(hash.finalize()) != request.sha256 {
        return Err("下载文件的大小或 SHA-256 不匹配".into());
    }
    output.flush().await.map_err(|error| error.to_string())
}
