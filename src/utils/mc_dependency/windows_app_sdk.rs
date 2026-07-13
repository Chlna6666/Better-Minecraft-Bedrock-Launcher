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
    DependencyEvent, download_file_with_progress, emit_log, emit_progress, is_installed_with_min,
    translator_for, wait_for_condition,
};
use crate::i18n::{Locale, Translator};

pub const WINDOWS_APP_SDK_RELEASES_URL: &str =
    "https://learn.microsoft.com/windows/apps/windows-app-sdk/downloads-archive";
const WINDOWS_APP_SDK_RUNTIME_PACKAGE_PREFIX: &str = "Microsoft.WindowsAppRuntime.1.8";
const WINDOWS_APP_SDK_RUNTIME_VERSION_LABEL: &str = "1.8.260529003";
const WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL: &str = "https://download.microsoft.com/download/eab8bdb8-8bad-4057-a315-7a88372e689b/WindowsAppRuntimeInstall-x64.exe";
const WINDOWS_APP_SDK_X64_AKA_MS_DOWNLOAD_URL: &str =
    "https://aka.ms/windowsappsdk/1.8/1.8.260529003/windowsappruntimeinstall-x64.exe";
const WINDOWS_APP_SDK_X64_GITHUB_ACCELERATED_DOWNLOAD_URL: &str = "https://dl-proxy.bmcbl.com/https://github.com/BE-Community-Dev/AppSDKArchive/releases/download/1.8/WindowsAppRuntimeInstall-x64.exe";
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
        label: "GitHub accelerated backup",
        url: WINDOWS_APP_SDK_X64_GITHUB_ACCELERATED_DOWNLOAD_URL,
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
pub enum WindowsAppSdkInstallerSource {
    Local,
    Download,
}

#[derive(Debug, Clone)]
pub struct WindowsAppSdkInstallPlan {
    pub installer_path: PathBuf,
    pub source: WindowsAppSdkInstallerSource,
    pub version_label: &'static str,
}

pub fn is_windows_app_sdk_runtime_installed() -> bool {
    if !cfg!(windows) {
        return true;
    }

    is_installed_with_min(WINDOWS_APP_SDK_RUNTIME_PACKAGE_PREFIX, None)
}

pub fn plan_windows_app_sdk_install(package_folder: &str) -> Option<WindowsAppSdkInstallPlan> {
    if is_windows_app_sdk_runtime_installed() {
        info!("Windows App SDK Runtime 1.8 已存在，无需安装");
        return None;
    }

    if windows_app_sdk_installer_download_sources().is_none() {
        warn!("当前目标架构不支持 Windows App SDK Runtime 1.8 自动下载");
        return None;
    }
    let Some(installer_file_name) = windows_app_sdk_installer_file_name() else {
        warn!("当前目标架构不支持 Windows App SDK Runtime 1.8 安装器命名");
        return None;
    };
    let installer_path = Path::new(package_folder)
        .join("Installers")
        .join(installer_file_name);
    let source = if installer_path.exists() {
        WindowsAppSdkInstallerSource::Local
    } else {
        WindowsAppSdkInstallerSource::Download
    };

    let plan = WindowsAppSdkInstallPlan {
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

pub async fn install_windows_app_sdk_runtime(
    locale: Locale,
    plan: WindowsAppSdkInstallPlan,
    sender: Option<UnboundedSender<DependencyEvent>>,
) -> Result<()> {
    let translator = translator_for(locale);
    let client = crate::http::proxy::get_download_client_for_proxy()
        .unwrap_or_else(|_| crate::http::request::GLOBAL_CLIENT.clone());
    let installer_name = plan
        .installer_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("windowsappruntimeinstall.exe")
        .to_string();
    info!(
        locale = ?locale,
        installer_name = %installer_name,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        version = plan.version_label,
        "开始安装 Windows App SDK Runtime 1.8"
    );

    download_windows_app_sdk_installer_if_needed(
        &client,
        &plan,
        &installer_name,
        &translator,
        sender.as_ref(),
    )
    .await?;
    emit_windows_app_sdk_admin_notice(sender.as_ref(), &translator, &plan);

    emit_progress(
        sender.as_ref(),
        85,
        translator
            .translate("McDeps.stages.installing")
            .into_owned(),
        Some(installer_name.clone()),
    );

    install_windows_app_sdk_installer(&plan.installer_path).await?;

    wait_for_condition(
        Duration::from_secs(90),
        Duration::from_secs(1),
        is_windows_app_sdk_runtime_installed,
    )
    .await
    .context("安装后未检测到 Windows App SDK Runtime 1.8")?;

    emit_progress(
        sender.as_ref(),
        100,
        translator.translate("McDeps.stages.done-one").into_owned(),
        Some(installer_name),
    );
    info!("Windows App SDK Runtime 1.8 安装并校验完成");

    Ok(())
}

async fn download_windows_app_sdk_installer_if_needed(
    client: &Client,
    plan: &WindowsAppSdkInstallPlan,
    installer_name: &str,
    translator: &Translator,
    sender: Option<&UnboundedSender<DependencyEvent>>,
) -> Result<()> {
    if matches!(plan.source, WindowsAppSdkInstallerSource::Local) {
        return Ok(());
    }

    emit_log(
        sender,
        translator
            .translate_args(
                "McDeps.logs.download_start",
                crate::i18n_args![("name", installer_name)],
            )
            .into_owned(),
    );
    let Some(download_sources) = windows_app_sdk_installer_download_sources() else {
        return Err(anyhow::anyhow!(
            "当前目标架构不支持 Windows App SDK Runtime 1.8 自动下载"
        ));
    };
    let request_headers = windows_app_sdk_download_headers();

    let mut last_error = None;
    for source in download_sources {
        let target_label = download_target_label(installer_name, *source);
        match download_file_with_progress(
            client,
            source.url,
            &plan.installer_path,
            translator.translate("McDeps.stages.download").into_owned(),
            Some(target_label),
            sender,
            Some(&request_headers),
        )
        .await
        {
            Ok(()) => {
                info!(
                    download_source = source.label,
                    download_url = source.url,
                    "Windows App SDK Runtime 安装器下载完成"
                );
                return Ok(());
            }
            Err(error) => {
                warn!(
                    download_source = source.label,
                    download_url = source.url,
                    error = %error,
                    "Windows App SDK Runtime 安装器下载源失败，尝试下一个源"
                );
                last_error = Some(error);
                remove_partial_installer(&plan.installer_path).await;
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("没有可用的 Windows App SDK Runtime 1.8 下载源")))
}

fn windows_app_sdk_installer_file_name() -> Option<String> {
    target_arch().map(|arch| format!("{WINDOWS_APP_SDK_INSTALLER_BASE_NAME}-{arch}.exe"))
}

fn windows_app_sdk_installer_download_sources() -> Option<&'static [WindowsAppSdkDownloadSource]> {
    match target_arch()? {
        "x86" => Some(WINDOWS_APP_SDK_X86_DOWNLOAD_SOURCES),
        "arm64" => Some(WINDOWS_APP_SDK_ARM64_DOWNLOAD_SOURCES),
        "x64" => Some(WINDOWS_APP_SDK_X64_DOWNLOAD_SOURCES),
        _ => None,
    }
}

fn windows_app_sdk_download_headers() -> HeaderMap {
    let mut headers = crate::http::proxy::download_request_headers();
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
    translator: &crate::i18n::Translator,
    plan: &WindowsAppSdkInstallPlan,
) {
    if !crate::utils::developer_mode::is_process_elevated() {
        tracing::warn!(
            installer_path = %plan.installer_path.display(),
            "安装 Windows App SDK Runtime 1.8 可能需要管理员权限"
        );
        super::emit_admin_required(
            sender,
            translator
                .translate("LaunchPrereq.adminRunRequired")
                .into_owned(),
        );
    }
}

#[cfg(not(windows))]
fn emit_windows_app_sdk_admin_notice(
    _sender: Option<&UnboundedSender<DependencyEvent>>,
    _translator: &crate::i18n::Translator,
    _plan: &WindowsAppSdkInstallPlan,
) {
}

#[cfg(windows)]
async fn install_windows_app_sdk_installer(installer_path: &Path) -> Result<()> {
    let installer_path = installer_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
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
        WINDOWS_APP_SDK_DOWNLOAD_USER_AGENT, WINDOWS_APP_SDK_X64_GITHUB_ACCELERATED_DOWNLOAD_URL,
        WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL, windows_app_sdk_download_headers,
        windows_app_sdk_installer_download_sources, windows_app_sdk_installer_file_name,
    };
    use reqwest::header::USER_AGENT;

    #[test]
    fn installer_file_name_matches_target_architecture() {
        let file_name = windows_app_sdk_installer_file_name()
            .expect("test target should map to a supported Windows App SDK architecture");

        assert!(file_name.starts_with("windowsappruntimeinstall-"));
        assert!(file_name.ends_with(".exe"));
    }

    #[test]
    fn first_installer_url_points_to_supported_runtime_installer() {
        let sources = windows_app_sdk_installer_download_sources()
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
    fn x64_download_sources_include_official_direct_and_accelerated_backup() {
        if !cfg!(target_arch = "x86_64") {
            return;
        }

        let sources = windows_app_sdk_installer_download_sources()
            .expect("x64 target should have Windows App SDK download sources");

        assert_eq!(
            sources[0].url,
            WINDOWS_APP_SDK_X64_MICROSOFT_DIRECT_DOWNLOAD_URL
        );
        assert!(
            sources
                .iter()
                .any(|source| source.url == WINDOWS_APP_SDK_X64_GITHUB_ACCELERATED_DOWNLOAD_URL)
        );
        assert_eq!(sources.len(), 3);
    }

    #[test]
    fn download_headers_use_proxy_accepted_user_agent() {
        let headers = windows_app_sdk_download_headers();

        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(WINDOWS_APP_SDK_DOWNLOAD_USER_AGENT)
        );
    }
}
