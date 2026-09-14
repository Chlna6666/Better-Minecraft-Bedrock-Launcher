use std::path::{Path, PathBuf};

use serde_json::json;

use super::{NativeModEntry, resolve_file_url};

#[derive(Clone, Debug)]
pub struct NativeModInstallRequest {
    pub game_directory: PathBuf,
    pub mod_entry: NativeModEntry,
    pub file_name: String,
}

pub fn start_install(request: NativeModInstallRequest) -> Result<String, String> {
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        "安装原生 Mod",
        Some(format!(
            "{} · {}",
            request.mod_entry.name, request.file_name
        )),
        "installing_native_mod",
        None,
        true,
    );
    let task_id_for_workflow = task_id.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let result = install(request, &task_id_for_workflow).await;
        match result {
            Ok(path) => crate::tasks::task_manager::finish_task(
                &task_id_for_workflow,
                "completed",
                Some(path.to_string_lossy().into_owned()),
            ),
            Err(error) => {
                crate::tasks::task_manager::finish_task(&task_id_for_workflow, "error", Some(error))
            }
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;
    crate::tasks::task_manager::register_task_abort_handle(
        task_id.clone(),
        workflow.abort_handle(),
    );
    Ok(task_id)
}

async fn install(request: NativeModInstallRequest, task_id: &str) -> Result<PathBuf, String> {
    let file = request
        .mod_entry
        .files
        .get(&request.file_name)
        .ok_or_else(|| format!("索引中不存在文件：{}", request.file_name))?;
    let client = crate::http::proxy::get_download_client_for_proxy()
        .map_err(|error| format!("构建下载客户端失败：{error}"))?;
    let url = resolve_file_url(&client, &request.mod_entry.repository, &file.url).await?;
    let cache_name = format!(
        "{}-{}",
        sanitize_component(&request.mod_entry.id),
        sanitize_component(&request.file_name)
    );
    let download_task =
        crate::downloads::api::download_resource(url, cache_name, None, Some(false), None).await?;
    let snapshot = crate::tasks::task_manager::wait_for_task_terminal(&download_task).await?;
    if snapshot.status.as_ref() != "completed" {
        return Err(format!(
            "原生 Mod 下载失败：{}",
            snapshot.message.as_deref().unwrap_or("没有错误详情")
        ));
    }
    let cached_path = snapshot
        .message
        .as_deref()
        .ok_or_else(|| "原生 Mod 下载完成但没有返回文件路径".to_string())?;
    crate::tasks::task_manager::set_task_message(
        task_id,
        Some(format!("正在安装 {}", request.mod_entry.name)),
    );

    let file_name = Path::new(&request.file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("无效的原生 Mod 文件名：{}", request.file_name))?;
    let mod_folder = sanitize_component(&request.mod_entry.id);
    let target_dir = request.game_directory.join("mods").join(mod_folder);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|error| format!("创建原生 Mod 目录失败：{error}"))?;
    let target_file = target_dir.join(file_name);
    tokio::fs::copy(cached_path, &target_file)
        .await
        .map_err(|error| format!("复制原生 Mod 文件失败：{error}"))?;
    let manifest_type = match file.file_type.as_str() {
        "native.dll" => "preload-native",
        "delay.dll" => "hot-inject",
        other => return Err(format!("暂不支持原生 Mod 类型：{other}")),
    };
    let manifest = json!({
        "name": request.mod_entry.name,
        "entry": file_name,
        "type": manifest_type,
        "version": request.mod_entry.id,
        "inject_delay_ms": 0,
    });
    let manifest_text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("序列化原生 Mod manifest 失败：{error}"))?;
    tokio::fs::write(target_dir.join("manifest.json"), manifest_text)
        .await
        .map_err(|error| format!("写入原生 Mod manifest 失败：{error}"))?;
    Ok(target_file)
}

fn sanitize_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "native-mod".to_string()
    } else {
        sanitized
    }
}
