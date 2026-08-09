use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::watch;
use tracing::{debug, warn};

use crate::tasks::task_manager::{self, TaskSnapshot};

use super::archive::{install_uncompressed_asset, install_zip_asset};
use super::installation_state::{
    inspect_installation as inspect_installation_state, prepare_package_install,
    read_installed_packages, uninstall_loader, update_lock, write_preloader_manifest,
};
use super::lip::{AssetKind, PackageId, render_template};
use super::planner::{
    PackageDisposition, PendingPackage, ResolvedPackage, github_repository, installation_order,
    resolve_packages,
};

pub(super) const LEVILAMINA_PACKAGE: &str = "github.com/LiteLDev/LeviLamina";
pub(super) const PRELOADER_PACKAGE: &str = "github.com/LiteLDev/PreLoader";
pub(super) const RUNTIME_DATA_PACKAGE: &str = "github.com/LiteLDev/bedrock-runtime-data";
pub(super) const CRASH_LOGGER_PACKAGE: &str = "github.com/LiteLDev/CrashLogger";
pub(super) const LOCATION_PACKAGE: &str = "github.com/LiteLDev/levilamina-loc";
pub(super) const MAX_RESOLVED_PACKAGES: usize = 64;

static NEXT_INSTALL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub enum LeviLaminaInstallRequest {
    Loader {
        game_directory: PathBuf,
        game_version: String,
        loader_version: String,
    },
    Mod {
        game_directory: PathBuf,
        game_version: String,
        package_id: String,
        version: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeviLaminaInstallStage {
    Resolving,
    Downloading {
        package: Arc<str>,
        task_id: Arc<str>,
    },
    Installing {
        package: Arc<str>,
    },
    Completed {
        message: Arc<str>,
    },
    Failed {
        message: Arc<str>,
    },
}

#[derive(Clone, Debug)]
pub struct LeviLaminaInstallSnapshot {
    pub operation_id: Arc<str>,
    pub stage: LeviLaminaInstallStage,
}

pub struct LeviLaminaInstallHandle {
    pub updates: watch::Receiver<LeviLaminaInstallSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeviLaminaInstallation {
    pub loader_version: Option<String>,
    pub preloader_version: Option<String>,
    pub has_runtime_data: bool,
}

pub fn start_install(request: LeviLaminaInstallRequest) -> Result<LeviLaminaInstallHandle, String> {
    start_operation(move |updates| async move {
        run_install(request, Some(&updates))
            .await
            .map(|_| LeviLaminaInstallStage::Completed {
                message: Arc::from("LeviLamina 组件安装完成"),
            })
    })
}

pub fn start_uninstall(game_directory: PathBuf) -> Result<LeviLaminaInstallHandle, String> {
    start_operation(move |_updates| async move {
        uninstall_loader(&game_directory).await?;
        Ok(LeviLaminaInstallStage::Completed {
            message: Arc::from("LeviLamina 已删除"),
        })
    })
}

pub async fn install_loader(
    game_directory: PathBuf,
    game_version: String,
    loader_version: String,
) -> Result<LeviLaminaInstallation, String> {
    run_install(
        LeviLaminaInstallRequest::Loader {
            game_directory: game_directory.clone(),
            game_version,
            loader_version,
        },
        None,
    )
    .await?;
    inspect_installation(game_directory).await
}

pub async fn inspect_installation(
    game_directory: PathBuf,
) -> Result<LeviLaminaInstallation, String> {
    inspect_installation_state(game_directory).await
}

fn start_operation<F, Fut>(operation: F) -> Result<LeviLaminaInstallHandle, String>
where
    F: FnOnce(watch::Sender<LeviLaminaInstallSnapshot>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<LeviLaminaInstallStage, String>> + Send + 'static,
{
    let operation_id = Arc::<str>::from(format!(
        "levilamina-install-{}",
        NEXT_INSTALL_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let (updates, receiver) = watch::channel(LeviLaminaInstallSnapshot {
        operation_id: Arc::clone(&operation_id),
        stage: LeviLaminaInstallStage::Resolving,
    });
    let monitor_updates = updates.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let stage = operation(updates.clone()).await.unwrap_or_else(|message| {
            LeviLaminaInstallStage::Failed {
                message: Arc::from(message),
            }
        });
        publish_stage(&updates, stage);
    })?;
    crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
        {
            publish_stage(
                &monitor_updates,
                LeviLaminaInstallStage::Failed {
                    message: Arc::from(format!("LeviLamina 工作流异常结束: {error}")),
                },
            );
        }
    })?;
    Ok(LeviLaminaInstallHandle { updates: receiver })
}

async fn run_install(
    request: LeviLaminaInstallRequest,
    updates: Option<&watch::Sender<LeviLaminaInstallSnapshot>>,
) -> Result<(), String> {
    let (game_directory, game_version, root) = root_package(&request)?;
    validate_game_directory(&game_directory).await?;
    let installed_packages = read_installed_packages(&game_directory).await?;
    let plan = resolve_packages(root, &game_version, &installed_packages).await?;
    let ordered = installation_order(&plan)?;
    for package in ordered {
        if package.disposition == PackageDisposition::Reuse {
            debug!(
                package = %package.id.display(),
                version = %package.manifest.version,
                "复用已安装 Lip 依赖，跳过下载和安装"
            );
            continue;
        }
        if let Some(updates) = updates {
            publish_stage(
                updates,
                LeviLaminaInstallStage::Installing {
                    package: Arc::from(package.id.display()),
                },
            );
        }
        install_package(&game_directory, package, updates).await?;
    }
    Ok(())
}

fn root_package(
    request: &LeviLaminaInstallRequest,
) -> Result<(PathBuf, String, PendingPackage), String> {
    match request {
        LeviLaminaInstallRequest::Loader {
            game_directory,
            game_version,
            loader_version,
        } => Ok((
            game_directory.clone(),
            game_version.clone(),
            PendingPackage {
                id: PackageId::parse(&format!("{LEVILAMINA_PACKAGE}#client"))?,
                requirement: loader_version.clone(),
                explicit_version: Some(loader_version.clone()),
                explicit: true,
            },
        )),
        LeviLaminaInstallRequest::Mod {
            game_directory,
            game_version,
            package_id,
            version,
        } => {
            let client_package = if package_id.contains('#') {
                package_id.clone()
            } else {
                format!("{package_id}#client")
            };
            Ok((
                game_directory.clone(),
                game_version.clone(),
                PendingPackage {
                    id: PackageId::parse(&client_package)?,
                    requirement: version.clone(),
                    explicit_version: Some(version.clone()),
                    explicit: true,
                },
            ))
        }
    }
}

async fn validate_game_directory(game_directory: &Path) -> Result<(), String> {
    let directory = game_directory.to_path_buf();
    crate::tasks::runtime::run_io_blocking(move || {
        if !directory.is_dir() {
            return Err(format!("游戏目录不存在: {}", directory.display()));
        }
        if !directory.join("Minecraft.Windows.exe").is_file() {
            return Err(format!(
                "目标不是有效客户端游戏目录: {}",
                directory.display()
            ));
        }
        Ok(())
    })
    .await?
}

async fn install_package(
    game_directory: &Path,
    package: &ResolvedPackage,
    updates: Option<&watch::Sender<LeviLaminaInstallSnapshot>>,
) -> Result<(), String> {
    prepare_package_install(game_directory, &package.id).await?;
    if !package.variant.scripts.pre_install.is_empty()
        || !package.variant.scripts.install.is_empty()
        || !package.variant.scripts.post_install.is_empty()
    {
        warn!(
            package = %package.id.display(),
            "忽略 Lip 生命周期脚本；BMCBL 不执行第三方清单命令"
        );
    }
    let mut files = Vec::new();
    for (asset_index, asset) in package.variant.assets.iter().enumerate() {
        let (archive_path, strip_root) = match asset.kind {
            AssetKind::Self_ => {
                let repository = github_repository(&package.id.path)?;
                let url = format!(
                    "https://github.com/{repository}/archive/refs/tags/v{}.zip",
                    package.manifest.version
                );
                (
                    download_asset(&url, package, asset_index, updates).await?,
                    true,
                )
            }
            AssetKind::Zip => (
                download_asset_urls(&asset.urls, package, asset_index, updates).await?,
                false,
            ),
            AssetKind::Uncompressed => {
                let source =
                    download_asset_urls(&asset.urls, package, asset_index, updates).await?;
                let game_directory = game_directory.to_path_buf();
                let placements = asset.placements.clone();
                let installed = crate::tasks::runtime::run_archive_blocking(move || {
                    install_uncompressed_asset(&source, &game_directory, &placements)
                })
                .await??;
                files.extend(installed);
                continue;
            }
            AssetKind::Tar | AssetKind::Tgz => {
                return Err(format!(
                    "暂不支持 Lip {} 资产: {}",
                    match asset.kind {
                        AssetKind::Tar => "tar",
                        _ => "tgz",
                    },
                    package.id.display()
                ));
            }
        };
        let game_directory = game_directory.to_path_buf();
        let placements = asset.placements.clone();
        files.extend(
            crate::tasks::runtime::run_archive_blocking(move || {
                install_zip_asset(&archive_path, &game_directory, &placements, strip_root)
            })
            .await??,
        );
    }

    if package.id.path.eq_ignore_ascii_case(PRELOADER_PACKAGE) {
        files.push(write_preloader_manifest(game_directory, &package.manifest.version).await?);
    }
    update_lock(game_directory, package, files).await
}

async fn download_asset_urls(
    urls: &[String],
    package: &ResolvedPackage,
    asset_index: usize,
    updates: Option<&watch::Sender<LeviLaminaInstallSnapshot>>,
) -> Result<PathBuf, String> {
    let mut errors = Vec::new();
    for url in urls {
        let rendered = render_template(url, &package.manifest);
        match download_asset(&rendered, package, asset_index, updates).await {
            Ok(path) => return Ok(path),
            Err(error) => errors.push(error),
        }
    }
    Err(format!(
        "下载 Lip 资产失败 {}: {}",
        package.id.display(),
        errors.join("; ")
    ))
}

async fn download_asset(
    url: &str,
    package: &ResolvedPackage,
    asset_index: usize,
    updates: Option<&watch::Sender<LeviLaminaInstallSnapshot>>,
) -> Result<PathBuf, String> {
    let file_name = format!(
        "lip-{}-{}-{asset_index}.zip",
        package
            .id
            .path
            .replace(|character: char| !character.is_ascii_alphanumeric(), "-"),
        package.manifest.version
    );
    let task_id = crate::downloads::api::download_resource(
        url.to_string(),
        file_name,
        None,
        Some(false),
        None,
    )
    .await?;
    task_manager::set_task_labels(
        &task_id,
        package.manifest.info.name.as_str(),
        Some(package.manifest.version.clone()),
    );
    if let Some(updates) = updates {
        publish_stage(
            updates,
            LeviLaminaInstallStage::Downloading {
                package: Arc::from(package.id.display()),
                task_id: Arc::from(task_id.as_str()),
            },
        );
    }
    let snapshot = task_manager::wait_for_task_terminal(&task_id).await?;
    require_download_path(&snapshot).map(PathBuf::from)
}

fn require_download_path(snapshot: &TaskSnapshot) -> Result<String, String> {
    if snapshot.status.as_ref() != "completed" {
        return Err(format!(
            "Lip 资产下载{}: {}",
            snapshot.status,
            snapshot.message.as_deref().unwrap_or("没有错误详情")
        ));
    }
    snapshot
        .message
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| "Lip 资产下载完成但没有返回文件路径".to_string())
}

fn publish_stage(
    updates: &watch::Sender<LeviLaminaInstallSnapshot>,
    stage: LeviLaminaInstallStage,
) {
    updates.send_modify(|snapshot| snapshot.stage = stage);
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
