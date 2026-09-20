use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::watch;
use tracing::{info, warn};

use crate::tasks::task_manager::{self, TaskSnapshot};

static NEXT_GAME_INSTALL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub enum GamePackageSource {
    Appx { package_id: String },
    Gdk { url: String },
}

#[derive(Clone, Debug)]
pub struct GameInstallRequest {
    pub package_key: String,
    pub version_label: String,
    pub file_name: String,
    pub install_folder: String,
    pub md5: Option<String>,
    pub force_download: bool,
    pub levilamina_version: Option<String>,
    pub source: GamePackageSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameInstallStage {
    Preparing,
    Downloading { task_id: Arc<str> },
    Extracting { task_id: Arc<str> },
    InstallingLeviLamina { version: Arc<str> },
    Completed { local_path: Arc<str> },
    Failed { message: Arc<str> },
}

#[derive(Clone, Debug)]
pub struct GameInstallSnapshot {
    pub operation_id: Arc<str>,
    pub package_key: Arc<str>,
    pub file_name: Arc<str>,
    pub stage: GameInstallStage,
}

pub struct GameInstallHandle {
    /// Visible BMCBL task that owns the complete install workflow.
    pub task_id: Arc<str>,
    /// Domain stage updates used by the download page for local presentation.
    pub updates: watch::Receiver<GameInstallSnapshot>,
}

pub fn start_game_install(request: GameInstallRequest) -> Result<GameInstallHandle, String> {
    let operation_id = Arc::<str>::from(format!(
        "game-install-{}",
        NEXT_GAME_INSTALL_ID.fetch_add(1, Ordering::Relaxed)
    ));
    task_manager::create_task_with_details(
        Some(operation_id.to_string()),
        "安装 Minecraft",
        Some(request.version_label.clone()),
        "preparing_game_install",
        None,
        false,
    );

    let active_child_task = Arc::new(Mutex::new(None::<String>));
    let cancel_child = Arc::clone(&active_child_task);
    task_manager::register_task_cancel_hook(operation_id.to_string(), move || {
        let child = cancel_child.lock().ok().and_then(|guard| guard.clone());
        if let Some(child) = child {
            task_manager::cancel_task(&child);
        }
    });

    let initial = GameInstallSnapshot {
        operation_id: Arc::clone(&operation_id),
        package_key: Arc::from(request.package_key.as_str()),
        file_name: Arc::from(request.file_name.as_str()),
        stage: GameInstallStage::Preparing,
    };
    let (updates, receiver) = watch::channel(initial);
    let monitor_updates = updates.clone();
    let worker_operation_id = Arc::clone(&operation_id);
    let worker_child_task = Arc::clone(&active_child_task);
    let workflow = match crate::tasks::runtime::spawn_io(async move {
        let outcome = run_game_install(
            &request,
            &worker_operation_id,
            &updates,
            &worker_child_task,
        )
        .await;
        match outcome {
            Ok(local_path) => {
                publish_stage(
                    &updates,
                    GameInstallStage::Completed {
                        local_path: Arc::from(local_path.clone()),
                    },
                );
                if !task_manager::is_cancelled(&worker_operation_id) {
                    task_manager::finish_task(
                        &worker_operation_id,
                        "completed",
                        Some(local_path),
                    );
                }
                info!(
                    operation_id = %worker_operation_id,
                    "game install completed; invalidating local version catalog"
                );
                crate::core::version::catalog_events::notify_local_versions_changed();
            }
            Err(message) => {
                warn!(
                    operation_id = %worker_operation_id,
                    %message,
                    "game install workflow failed"
                );
                publish_stage(
                    &updates,
                    GameInstallStage::Failed {
                        message: Arc::from(message.clone()),
                    },
                );
                if !task_manager::is_cancelled(&worker_operation_id) {
                    task_manager::finish_task(&worker_operation_id, "error", Some(message));
                }
            }
        }
    }) {
        Ok(workflow) => workflow,
        Err(error) => {
            task_manager::finish_task(&operation_id, "error", Some(error.clone()));
            return Err(error);
        }
    };
    task_manager::register_task_abort_handle(operation_id.to_string(), workflow.abort_handle());

    let monitor_operation_id = Arc::clone(&operation_id);
    crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
        {
            let message = format!("安装工作流异常结束: {error}");
            publish_stage(
                &monitor_updates,
                GameInstallStage::Failed {
                    message: Arc::from(message.clone()),
                },
            );
            if !task_manager::is_cancelled(&monitor_operation_id) {
                task_manager::finish_task(&monitor_operation_id, "error", Some(message));
            }
        }
    })?;

    Ok(GameInstallHandle {
        task_id: operation_id,
        updates: receiver,
    })
}

async fn run_game_install(
    request: &GameInstallRequest,
    operation_id: &str,
    updates: &watch::Sender<GameInstallSnapshot>,
    active_child_task: &Arc<Mutex<Option<String>>>,
) -> Result<String, String> {
    info!(
        operation_id,
        package_key = request.package_key,
        file_name = request.file_name,
        "game install workflow started"
    );
    let local_path = if request.force_download {
        None
    } else {
        crate::downloads::api::local_download_path(request.file_name.clone(), request.md5.clone())
            .await?
    };

    let package_path = match local_path {
        Some(path) => path,
        None => download_package(request, updates, active_child_task).await?,
    };
    let extract_task_id = start_extract(request, &package_path).await?;
    set_active_child_task(active_child_task, Some(extract_task_id.clone()));
    publish_stage(
        updates,
        GameInstallStage::Extracting {
            task_id: Arc::from(extract_task_id.as_str()),
        },
    );

    let extract_snapshot = task_manager::wait_for_task_terminal(&extract_task_id).await?;
    set_active_child_task(active_child_task, None);
    require_completed(&extract_snapshot, "安装")?;
    if let Some(loader_version) = &request.levilamina_version {
        publish_stage(
            updates,
            GameInstallStage::InstallingLeviLamina {
                version: Arc::from(loader_version.as_str()),
            },
        );
        let game_directory =
            crate::utils::file_ops::bmcbl_subdir("versions").join(&request.install_folder);
        let handle = crate::core::levilamina::start_install(
            crate::core::levilamina::LeviLaminaInstallRequest::Loader {
                game_directory,
                game_version: request.version_label.clone(),
                loader_version: loader_version.clone(),
            },
        )?;
        set_active_child_task(active_child_task, Some(handle.task_id.to_string()));
        let loader_snapshot = task_manager::wait_for_task_terminal(handle.task_id.as_ref()).await?;
        set_active_child_task(active_child_task, None);
        require_completed(&loader_snapshot, "LeviLamina 安装")?;
    }
    info!(
        operation_id,
        extract_task_id, "game install workflow completed"
    );
    Ok(package_path)
}

async fn download_package(
    request: &GameInstallRequest,
    updates: &watch::Sender<GameInstallSnapshot>,
    active_child_task: &Arc<Mutex<Option<String>>>,
) -> Result<String, String> {
    let task_id = match &request.source {
        GamePackageSource::Appx { package_id } => {
            crate::downloads::api::download_appx(
                package_id.clone(),
                request.file_name.clone(),
                request.md5.clone(),
                Some(request.force_download),
                None,
            )
            .await?
        }
        GamePackageSource::Gdk { url } => {
            crate::downloads::api::download_game_resource(
                url.clone(),
                request.file_name.clone(),
                request.md5.clone(),
                Some(request.force_download),
                None,
            )
            .await?
        }
    };

    if !task_manager::set_task_labels(
        &task_id,
        request.file_name.as_str(),
        Some(request.version_label.clone()),
    ) {
        warn!(
            task_id,
            "download task disappeared before labels were applied"
        );
    }
    set_active_child_task(active_child_task, Some(task_id.clone()));
    publish_stage(
        updates,
        GameInstallStage::Downloading {
            task_id: Arc::from(task_id.as_str()),
        },
    );

    let snapshot = task_manager::wait_for_task_terminal(&task_id).await?;
    set_active_child_task(active_child_task, None);
    require_completed(&snapshot, "下载")?;
    snapshot
        .message
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| "下载完成但任务没有返回文件路径".to_string())
}

async fn start_extract(request: &GameInstallRequest, package_path: &str) -> Result<String, String> {
    match request.source {
        GamePackageSource::Gdk { .. } => {
            crate::core::minecraft::gdk::unpack::start_unpack_gdk_task(
                package_path,
                &request.install_folder,
            )
        }
        GamePackageSource::Appx { .. } => {
            crate::archive::api::extract_zip_appx(
                format!("{}.appx", request.install_folder),
                package_path.to_string(),
                true,
                true,
            )
            .await
        }
    }
}

fn set_active_child_task(active_child_task: &Arc<Mutex<Option<String>>>, task_id: Option<String>) {
    if let Ok(mut current) = active_child_task.lock() {
        *current = task_id;
    }
}

fn require_completed(snapshot: &TaskSnapshot, operation: &str) -> Result<(), String> {
    if snapshot.status.as_ref() == "completed" {
        return Ok(());
    }
    Err(format!(
        "{operation}{}: {}",
        snapshot.status,
        snapshot.message.as_deref().unwrap_or("没有错误详情")
    ))
}

fn publish_stage(updates: &watch::Sender<GameInstallSnapshot>, stage: GameInstallStage) {
    let operation_id = updates.borrow().operation_id.clone();
    match &stage {
        GameInstallStage::Preparing => {
            task_manager::update_progress(&operation_id, 0, None, Some("preparing_game_install"));
            task_manager::set_task_message(&operation_id, Some("正在准备安装".to_string()));
        }
        GameInstallStage::Downloading { .. } => {
            task_manager::update_progress(&operation_id, 0, None, Some("downloading_game"));
            task_manager::set_task_message(&operation_id, Some("正在下载游戏包".to_string()));
        }
        GameInstallStage::Extracting { .. } => {
            task_manager::update_progress(&operation_id, 0, None, Some("installing_game"));
            task_manager::set_task_message(&operation_id, Some("正在解压并安装游戏".to_string()));
        }
        GameInstallStage::InstallingLeviLamina { version } => {
            task_manager::update_progress(
                &operation_id,
                0,
                None,
                Some("installing_levilamina"),
            );
            task_manager::set_task_message(
                &operation_id,
                Some(format!("正在安装 LeviLamina {version}")),
            );
        }
        GameInstallStage::Completed { .. } | GameInstallStage::Failed { .. } => {}
    }
    updates.send_modify(|snapshot| {
        snapshot.stage = stage;
    });
}
