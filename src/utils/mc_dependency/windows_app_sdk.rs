use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderValue, USER_AGENT},
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Duration;
use tracing::{info, warn};

use super::{
    DependencyEvent, download_file, emit_log, emit_progress, is_installed,
    start_dependency_install_task,
    wait_for_condition,
};

pub const RELEASES_URL: &str =
    "https://learn.microsoft.com/windows/apps/windows-app-sdk/downloads-archive";
const WINDOWS_APP_SDK_RUNTIME_PACKAGE_PREFIX: &str = "Microsoft.WindowsAppRuntime.1.8";
const WINDOWS_APP_SDK_RUNTIME_VERSION_LABEL: &str = "1.8.260529003";
const WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL: &str = "https://download.microsoft.com/download/eab8bdb8-8bad-4057-a315-7a88372e689b/WindowsAppRuntimeInstall-x64.exe";
const WINDOWS_APP_SDK_X64_AKA_MS_DOWNLOAD_URL: &str =
    "https://aka.ms/windowsappsdk/1.8/1.8.260529003/windowsappruntimeinstall-x64.exe";
const WINDOWS_APP_SDK_X64_GITHUB_DOWNLOAD_URL: &str = "https://github.com/BE-Community-Dev/AppSDKArchive/releases/download/1.8/WindowsAppRuntimeInstall-x64.exe";
const WINDOWS_APP_SDK_X86_DOWNLOAD_URL: &str =
    "https://aka.ms/windowsappsdk/1.8/1.8.260529003/windowsappruntimeinstall-x86.exe";
const WINDOWS_APP_SDK_ARM64_DOWNLOAD_URL: &str =
    "https://aka.ms/windowsappsdk/1.8/1.8.260529003/windowsappruntimeinstall-arm64.exe";
const WINDOWS_APP_SDK_INSTALLER_BASE_NAME: &str = "windowsappruntimeinstall";
const WINDOWS_APP_SDK_DOWNLOAD_USER_AGENT: &str = "BMCBL-Updater";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct WindowsAppSdkDownloadSource {
    label: &'static str,
    url: &'static str,
}

const WINDOWS_APP_SDK_X64_DOWNLOAD_SOURCES: &[WindowsAppSdkDownloadSource] = &[
    WindowsAppSdkDownloadSource {
        label: "Microsoft direct",
        url: WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL,
    },
    WindowsAppSdkDownloadSource {
        label: "Microsoft archive",
        url: WINDOWS_APP_SDK_X64_AKA_MS_DOWNLOAD_URL,
    },
    WindowsAppSdkDownloadSource {
        label: "GitHub backup",
        url: WINDOWS_APP_SDK_X64_GITHUB_DOWNLOAD_URL,
    },
];
const WINDOWS_APP_SDK_X86_DOWNLOAD_SOURCES: &[WindowsAppSdkDownloadSource] =
    &[WindowsAppSdkDownloadSource {
        label: "Microsoft archive",
        url: WINDOWS_APP_SDK_X86_DOWNLOAD_URL,
    }];
const WINDOWS_APP_SDK_ARM64_DOWNLOAD_SOURCES: &[WindowsAppSdkDownloadSource] =
    &[WindowsAppSdkDownloadSource {
        label: "Microsoft archive",
        url: WINDOWS_APP_SDK_ARM64_DOWNLOAD_URL,
    }];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum InstallerSource {
    Local,
    Download,
}

#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub installer_path: PathBuf,
    pub source: InstallerSource,
    pub version_label: &'static str,
}

pub(crate) fn start(plan: InstallPlan) -> Result<String, String> {
    let detail = Some(format!("Windows App SDK {}", plan.version_label));
    start_dependency_install_task(
        "安装 Windows App SDK Runtime",
        detail,
        "installing_windows_app_sdk",
        move |sender| async move { install_runtime(plan, Some(sender)).await },
    )
}

pub fn is_runtime_installed() -> bool {
    if !cfg!(windows) {
        return true;
    }

    is_installed(WINDOWS_APP_SDK_RUNTIME_PACKAGE_PREFIX, None)
}

pub fn plan_install(package_folder: &str) -> Option<InstallPlan> {
    if is_runtime_installed() {
        info!("Windows App SDK Runtime 1.8 已存在，无需安装");
        return None;
    }

    if installer_download_sources().is_none() {
        warn!("当前目标架构不支持 Windows App SDK Runtime 1.8 自动下载");
        return None;
    }
    let Some(installer_file_name) = installer_file_name() else {
        warn!("当前目标架构不支持 Windows App SDK Runtime 1.8 安装器命名");
        return None;
    };
    let installer_path = Path::new(package_folder)
        .join("Installers")
        .join(installer_file_name);
    let source = if installer_path.exists() {
        InstallerSource::Local
    } else {
        InstallerSource::Download
    };

    let plan = InstallPlan {
        installer_path,
        source,
        version_label: WINDOWS_APP_SDK_RUNTIME_VERSION_LABEL,
    };
    info!(
        package_folder,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        version = plan.version_label,
        "已生成 Windows App SDK Runtime 安装计划"
    );
    Some(plan)
}

pub async fn install_runtime(
    plan: InstallPlan,
    sender: Option<UnboundedSender<DependencyEvent>>,
) -> Result<()> {
    let client = crate::http::proxy::clients::download_client()
        .unwrap_or_else(|_| crate::http::request::GLOBAL_CLIENT.clone());
    let installer_name = plan
        .installer_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("windowsappruntimeinstall.exe")
        .to_string();
    info!(
        installer_name = %installer_name,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        version = plan.version_label,
        "开始安装 Windows App SDK Runtime 1.8"
    );

    download_windows_app_sdk_installer(&client, &plan, &installer_name, sender.as_ref()).await?;
    emit_windows_app_sdk_admin_notice(sender.as_ref(), &plan);

    emit_progress(
        sender.as_ref(),
        85,
        crate::localized_text!("McDeps.stages.installing"),
        Some(installer_name.clone()),
    );

    install_windows_app_sdk_installer(&plan.installer_path).await?;

    wait_for_condition(
        Duration::from_secs(90),
        Duration::from_secs(1),
        is_runtime_installed,
    )
    .await
    .context("安装后未检测到 Windows App SDK Runtime 1.8")?;

    emit_progress(
        sender.as_ref(),
        100,
        crate::localized_text!("McDeps.stages.done-one"),
        Some(installer_name),
    );
    info!("Windows App SDK Runtime 1.8 安装并校验完成");

    Ok(())
}

async fn download_windows_app_sdk_installer(
    client: &Client,
    plan: &InstallPlan,
    installer_name: &str,
    sender: Option<&UnboundedSender<DependencyEvent>>,
) -> Result<()> {
    if matches!(plan.source, InstallerSource::Local) {
        return Ok(());
    }

    emit_log(
        sender,
        crate::localized_text!(
            "McDeps.logs.download_start",
            name = installer_name.to_string(),
        ),
    );
    let Some(download_sources) = installer_download_sources() else {
        return Err(anyhow::anyhow!(
            "当前目标架构不支持 Windows App SDK Runtime 1.8 自动下载"
        ));
    };
    let request_headers = download_headers();

    let mut last_error = None;
    for source in download_sources {
        let candidates =
            crate::github::configured_download_urls(source.url).map_err(anyhow::Error::msg)?;
        for candidate_url in candidates {
            let target_label = download_target_label(installer_name, *source);
            match download_file(
                client,
                &candidate_url,
                &plan.installer_path,
                crate::localized_text!("McDeps.stages.download"),
                Some(target_label.clone()),
                sender,
                Some(&request_headers),
            )
            .await
            {
                Ok(()) => {
                    info!(
                        download_source = source.label,
                        canonical_url = source.url,
                        selected_url = %candidate_url,
                        "Windows App SDK Runtime 安装器下载完成"
                    );
                    return Ok(());
                }
                Err(error) => {
                    warn!(
                        download_source = source.label,
                        canonical_url = source.url,
                        candidate_url = %candidate_url,
                        error = %error,
                        "Windows App SDK Runtime 安装器下载候选失败，尝试下一个候选"
                    );
                    last_error = Some(error);
                    remove_partial_installer(&plan.installer_path).await;
                }
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("没有可用的 Windows App SDK Runtime 1.8 下载源")))
}

fn installer_file_name() -> Option<String> {
    target_arch().map(|arch| format!("{WINDOWS_APP_SDK_INSTALLER_BASE_NAME}-{arch}.exe"))
}

fn installer_download_sources() -> Option<&'static [WindowsAppSdkDownloadSource]> {
    match target_arch()? {
        "x86" => Some(WINDOWS_APP_SDK_X86_DOWNLOAD_SOURCES),
        "arm64" => Some(WINDOWS_APP_SDK_ARM64_DOWNLOAD_SOURCES),
        "x64" => Some(WINDOWS_APP_SDK_X64_DOWNLOAD_SOURCES),
        _ => None,
    }
}

fn download_headers() -> HeaderMap {
    let mut headers = crate::http::download::request_headers();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(WINDOWS_APP_SDK_DOWNLOAD_USER_AGENT),
    );
    headers
}

fn download_target_label(installer_name: &str, source: WindowsAppSdkDownloadSource) -> String {
    format!("{installer_name} ({})", source.label)
}

async fn remove_partial_installer(installer_path: &Path) {
    match tokio::fs::remove_file(installer_path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(
                installer_path = %installer_path.display(),
                error = %error,
                "删除未完成的 Windows App SDK Runtime 安装器失败"
            );
        }
    }
}

fn target_arch() -> Option<&'static str> {
    if cfg!(target_arch = "x86") {
        Some("x86")
    } else if cfg!(target_arch = "x86_64") {
        Some("x64")
    } else if cfg!(target_arch = "aarch64") {
        Some("arm64")
    } else {
        None
    }
}

#[cfg(windows)]
fn emit_windows_app_sdk_admin_notice(
    sender: Option<&UnboundedSender<DependencyEvent>>,
    plan: &InstallPlan,
) {
    if !crate::utils::developer_mode::is_process_elevated() {
        tracing::warn!(
            installer_path = %plan.installer_path.display(),
            "安装 Windows App SDK Runtime 1.8 可能需要管理员权限"
        );
        super::emit_admin_required(
            sender,
            crate::localized_text!("LaunchPrereq.adminRunRequired"),
        );
    }
}

#[cfg(not(windows))]
fn emit_windows_app_sdk_admin_notice(
    _sender: Option<&UnboundedSender<DependencyEvent>>,
    _plan: &InstallPlan,
) {
}

#[cfg(windows)]
async fn install_windows_app_sdk_installer(installer_path: &Path) -> Result<()> {
    let installer_path = installer_path.to_path_buf();
    crate::tasks::runtime::blocking::run_io(move || {
        use std::process::Command;

        let output = Command::new(&installer_path)
            .args(["--quiet", "--force"])
            .output()
            .with_context(|| {
                format!(
                    "启动 Windows App SDK Runtime 安装器失败: {}",
                    installer_path.display()
                )
            })?;

        if output.status.success() {
            info!(
                installer_path = %installer_path.display(),
                "Windows App SDK Runtime 安装器执行成功"
            );
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "Windows App SDK Runtime 安装器失败，退出码 {:?}: {}",
                output.status.code(),
                stderr.trim()
            ))
        }
    })
    .await
    .map_err(anyhow::Error::msg)
    .context("等待 Windows App SDK Runtime 安装线程失败")?
}

#[cfg(not(windows))]
async fn install_windows_app_sdk_installer(_installer_path: &Path) -> Result<()> {
    Err(anyhow::anyhow!(
        "Windows App SDK Runtime 只能在 Windows 上安装"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        WINDOWS_APP_SDK_DOWNLOAD_USER_AGENT, WINDOWS_APP_SDK_X64_GITHUB_DOWNLOAD_URL,
        WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL, download_headers,
        installer_download_sources, installer_file_name,
    };
    use reqwest::header::USER_AGENT;

    #[test]
    fn installer_file_name_matches_target_architecture() {
        let file_name = installer_file_name()
            .expect("test target should map to a supported Windows App SDK architecture");

        assert!(file_name.starts_with("windowsappruntimeinstall-"));
        assert!(file_name.ends_with(".exe"));
    }

    #[test]
    fn first_installer_url_points_to_supported_runtime_installer() {
        let sources = installer_download_sources()
            .expect("test target should map to supported Windows App SDK download sources");
        let Some(first_source) = sources.first() else {
            panic!("supported architecture should have at least one download source");
        };

        assert!(first_source.url.starts_with("https://"));
        assert!(first_source.url.to_ascii_lowercase().ends_with(".exe"));
        if cfg!(target_arch = "x86_64") {
            assert_eq!(
                first_source.url,
                WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL
            );
        }
    }

    #[test]
    fn x64_download_sources_include_official_direct_and_github_backup() {
        if !cfg!(target_arch = "x86_64") {
            return;
        }

        let sources = installer_download_sources()
            .expect("x64 target should have Windows App SDK download sources");

        assert_eq!(
            sources[0].url,
            WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL
        );
        assert!(
            sources
                .iter()
                .any(|source| source.url == WINDOWS_APP_SDK_X64_GITHUB_DOWNLOAD_URL)
        );
        assert_eq!(sources.len(), 3);
    }

    #[test]
    fn download_headers_use_proxy_accepted_user_agent() {
        let headers = download_headers();

        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(WINDOWS_APP_SDK_DOWNLOAD_USER_AGENT)
        );
    }
}
