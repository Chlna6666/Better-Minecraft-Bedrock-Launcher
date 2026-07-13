use std::cmp::Ordering;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use reqwest::Client;
use reqwest::header::{CONTENT_LENGTH, HeaderMap};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};
#[cfg(windows)]
use url::Url;
#[cfg(windows)]
use windows::Foundation::Uri;
#[cfg(windows)]
use windows::Management::Deployment::{AddPackageOptions, PackageManager};
#[cfg(windows)]
use windows::Win32::System::ApplicationInstallationAndServicing::{
    INSTALLUILEVEL_NONE, MsiInstallProductW, MsiSetInternalUI,
};
#[cfg(windows)]
use windows::core::{HRESULT, HSTRING, PCWSTR};
#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

use crate::http::proxy::get_client_for_proxy;
use crate::http::request::GLOBAL_CLIENT;
use crate::i18n::{Locale, Translator};
#[cfg(windows)]
use crate::utils::developer_mode;

mod windows_app_sdk;
pub use windows_app_sdk::{
    WINDOWS_APP_SDK_RELEASES_URL, WindowsAppSdkInstallPlan, WindowsAppSdkInstallerSource,
    install_windows_app_sdk_runtime, is_windows_app_sdk_runtime_installed,
    plan_windows_app_sdk_install,
};

pub const GAMEINPUT_RELEASES_URL: &str = "https://github.com/microsoftconnect/GameInput/releases";
pub const GAMEINPUT_LATEST_DOWNLOAD_URL: &str =
    "https://github.com/microsoftconnect/GameInput/releases/latest/download/GameInputRedist.msi";

#[derive(Debug, Clone)]
pub struct MissingUwpDependency {
    pub name: String,
    pub pfn: String,
    pub min_version: Option<String>,
    pub issue_kind: UwpDependencyIssueKind,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UwpDependencyIssueKind {
    Missing,
    VersionMismatch {
        installed_version: Option<String>,
        required_version: String,
    },
}

impl MissingUwpDependency {
    pub fn issue_summary(&self) -> String {
        match &self.issue_kind {
            UwpDependencyIssueKind::Missing => format!("{} [missing]", self.name),
            UwpDependencyIssueKind::VersionMismatch {
                installed_version,
                required_version,
            } => match installed_version {
                Some(installed_version) => format!(
                    "{} [version mismatch: current {}, required >= {}]",
                    self.name, installed_version, required_version
                ),
                None => format!(
                    "{} [version mismatch: current unknown, required >= {}]",
                    self.name, required_version
                ),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GameInputInstallerSource {
    Local,
    Download,
}

#[derive(Debug, Clone)]
pub struct GameInputInstallPlan {
    pub installer_path: PathBuf,
    pub source: GameInputInstallerSource,
}

#[derive(Debug, Clone)]
pub enum DependencyEvent {
    Log(String),
    Progress {
        percent: u32,
        stage: String,
        target: Option<String>,
    },
    AdminRequired(String),
}

fn extract_version(input: &str) -> Option<String> {
    let regex = Regex::new(r"(\d+\.\d+\.\d+\.\d+)").ok()?;
    regex
        .captures(input)
        .and_then(|capture| capture.get(1).map(|value| value.as_str().to_string()))
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let parse_parts = |value: &str| {
        value
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };

    let left_parts = parse_parts(left);
    let right_parts = parse_parts(right);
    let max_len = left_parts.len().max(right_parts.len());

    for index in 0..max_len {
        let left_part = *left_parts.get(index).unwrap_or(&0);
        let right_part = *right_parts.get(index).unwrap_or(&0);
        match left_part.cmp(&right_part) {
            Ordering::Equal => {}
            non_equal => return non_equal,
        }
    }

    Ordering::Equal
}

pub fn is_installed_with_min(prefix: &str, min_version: Option<&str>) -> bool {
    if !cfg!(windows) {
        return true;
    }

    inspect_uwp_dependency(prefix, min_version).is_none()
}

enum InstalledUwpDependencyState {
    NotInstalled,
    Installed { version: Option<String> },
}

#[cfg(windows)]
fn read_installed_uwp_dependency_state(prefix: &str) -> InstalledUwpDependencyState {
    let package_manager = match PackageManager::new() {
        Ok(package_manager) => package_manager,
        Err(error) => {
            debug!("无法创建 PackageManager: {:?}", error);
            return InstalledUwpDependencyState::NotInstalled;
        }
    };

    let mut found_package = false;
    let mut highest_version: Option<String> = None;

    if let Ok(packages) = package_manager.FindPackages() {
        for package in packages {
            let Ok(id) = package.Id() else {
                continue;
            };
            let name = id
                .Name()
                .map(|value| value.to_string())
                .unwrap_or_else(|_| String::new());
            if !name.starts_with(prefix) {
                continue;
            }
            found_package = true;
            let installed_version = id
                .Version()
                .map(|version| {
                    format!(
                        "{}.{}.{}.{}",
                        version.Major, version.Minor, version.Build, version.Revision
                    )
                })
                .ok()
                .or_else(|| extract_version(&name));
            if let Some(installed_version) = installed_version {
                let should_replace = highest_version
                    .as_ref()
                    .is_none_or(|current| compare_versions(&installed_version, current).is_gt());
                if should_replace {
                    highest_version = Some(installed_version);
                }
            }
        }
    }

    if found_package {
        InstalledUwpDependencyState::Installed {
            version: highest_version,
        }
    } else {
        InstalledUwpDependencyState::NotInstalled
    }
}

#[cfg(not(windows))]
fn read_installed_uwp_dependency_state(_prefix: &str) -> InstalledUwpDependencyState {
    InstalledUwpDependencyState::Installed { version: None }
}

fn inspect_uwp_dependency(name: &str, min_version: Option<&str>) -> Option<MissingUwpDependency> {
    let installed_state = read_installed_uwp_dependency_state(name);
    let issue_kind = match (installed_state, min_version) {
        (InstalledUwpDependencyState::Installed { .. }, None) => return None,
        (InstalledUwpDependencyState::NotInstalled, None) => UwpDependencyIssueKind::Missing,
        (InstalledUwpDependencyState::NotInstalled, Some(_)) => UwpDependencyIssueKind::Missing,
        (
            InstalledUwpDependencyState::Installed {
                version: Some(installed_version),
            },
            Some(required_version),
        ) if compare_versions(&installed_version, required_version) != Ordering::Less => {
            return None;
        }
        (InstalledUwpDependencyState::Installed { version }, Some(required_version)) => {
            UwpDependencyIssueKind::VersionMismatch {
                installed_version: version,
                required_version: required_version.to_string(),
            }
        }
    };

    Some(MissingUwpDependency {
        name: name.to_string(),
        pfn: format!("{name}_8wekyb3d8bbwe"),
        min_version: min_version.map(str::to_string),
        issue_kind,
    })
}

fn uwp_deps_list() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("Microsoft.VCLibs.140.00", Some("14.0.33519.0")),
        ("Microsoft.NET.Native.Runtime.1.4", None),
        ("Microsoft.NET.Native.Runtime.2.2", Some("2.2.28604.0")),
        ("Microsoft.VCLibs.140.00.UWPDesktop", None),
        ("Microsoft.Services.Store.Engagement", None),
        ("Microsoft.NET.Native.Framework.1.3", None),
        ("Microsoft.NET.Native.Framework.2.2", Some("2.2.29512.0")),
        ("Microsoft.GamingServices", Some("33.108.12001.0")),
    ]
}

#[cfg(windows)]
pub fn compute_missing_uwp_dependencies() -> Vec<MissingUwpDependency> {
    let missing = uwp_deps_list()
        .iter()
        .copied()
        .filter_map(|(name, min_version)| inspect_uwp_dependency(name, min_version))
        .collect::<Vec<_>>();
    info!(
        missing_count = missing.len(),
        dependencies = ?missing
            .iter()
            .map(MissingUwpDependency::issue_summary)
            .collect::<Vec<_>>(),
        "已完成 UWP 依赖检查"
    );
    missing
}

#[cfg(not(windows))]
pub fn compute_missing_uwp_dependencies() -> Vec<MissingUwpDependency> {
    info!("非 Windows 平台跳过 UWP 依赖检查");
    Vec::new()
}

fn select_best_candidate(
    mut candidates: Vec<(String, String)>,
    min_version: Option<&str>,
) -> Option<(String, String)> {
    candidates.sort_by(|left, right| {
        let left_version = extract_version(&left.0);
        let right_version = extract_version(&right.0);
        match (left_version, right_version) {
            (Some(left_version), Some(right_version)) => {
                compare_versions(&right_version, &left_version)
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    });

    if let Some(required_min_version) = min_version {
        for (name, url) in &candidates {
            let Some(candidate_version) = extract_version(name) else {
                continue;
            };
            if compare_versions(&candidate_version, required_min_version) != Ordering::Less {
                return Some((name.clone(), url.clone()));
            }
        }
    }

    candidates.into_iter().next()
}

#[cfg(windows)]
pub fn is_game_input_installed() -> bool {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let uninstall_paths = [
        (
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_READ | KEY_WOW64_64KEY,
        ),
        (
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_READ,
        ),
    ];

    for (subkey_path, flags) in uninstall_paths {
        let Ok(root_key) = hklm.open_subkey_with_flags(subkey_path, flags) else {
            continue;
        };

        for key_name in root_key.enum_keys().flatten() {
            let Ok(entry) = root_key.open_subkey_with_flags(&key_name, flags) else {
                continue;
            };
            let display_name: String = entry.get_value("DisplayName").unwrap_or_default();
            let publisher: String = entry.get_value("Publisher").unwrap_or_default();
            let normalized_name = display_name.to_ascii_lowercase();
            let normalized_publisher = publisher.to_ascii_lowercase();

            if normalized_name.contains("gameinput")
                && (normalized_publisher.contains("microsoft")
                    || normalized_name.contains("microsoft"))
            {
                info!(
                    display_name = %display_name,
                    publisher = %publisher,
                    "已检测到已安装的 GameInput Runtime"
                );
                return true;
            }
        }
    }

    debug!("未检测到已安装的 GameInput Runtime");
    false
}

#[cfg(not(windows))]
pub fn is_game_input_installed() -> bool {
    true
}

pub fn plan_game_input_install(package_folder: &str) -> Option<GameInputInstallPlan> {
    if is_game_input_installed() {
        info!("GameInput Runtime 已存在，无需安装");
        return None;
    }

    let installer_path = Path::new(package_folder)
        .join("Installers")
        .join("GameInputRedist.msi");
    let source = if installer_path.exists() {
        GameInputInstallerSource::Local
    } else {
        GameInputInstallerSource::Download
    };

    let plan = GameInputInstallPlan {
        installer_path,
        source,
    };
    info!(
        package_folder,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        "已生成 GameInput 安装计划"
    );
    Some(plan)
}

pub async fn install_missing_uwp_dependencies(
    locale: Locale,
    missing_dependencies: Vec<MissingUwpDependency>,
    sender: Option<UnboundedSender<DependencyEvent>>,
) -> Result<()> {
    if missing_dependencies.is_empty() {
        info!("没有缺失的 UWP 依赖，跳过安装");
        return Ok(());
    }

    info!(
        locale = ?locale,
        missing_count = missing_dependencies.len(),
        dependencies = ?missing_dependencies
            .iter()
            .map(MissingUwpDependency::issue_summary)
            .collect::<Vec<_>>(),
        "开始安装缺失的 UWP 依赖"
    );

    let translator = translator_for(locale);
    let client = get_client_for_proxy().unwrap_or_else(|_| GLOBAL_CLIENT.clone());
    let anchor_regex = Regex::new(r#"<a\s+href=\"(?P<href>[^\"]+)\"[^>]*>(?P<name>[^<]+)</a>"#)?;

    emit_log(
        sender.as_ref(),
        translator.translate("McDeps.logs.start").into_owned(),
    );

    for dependency in missing_dependencies {
        info!(
            dependency = %dependency.name,
            min_version = ?dependency.min_version,
            issue = %dependency.issue_summary(),
            "开始处理 UWP 依赖"
        );
        let package_family = dependency.pfn.clone();
        let request_body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("type", "PackageFamilyName")
            .append_pair("url", package_family.as_str())
            .append_pair("ring", "RP")
            .append_pair("lang", "en-US")
            .finish();

        let response = client
            .post("https://store.rg-adguard.net/api/GetFiles")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Origin", "https://store.rg-adguard.net")
            .header("Referer", "https://store.rg-adguard.net/")
            .body(request_body)
            .send()
            .await
            .with_context(|| format!("请求依赖下载页失败: {}", dependency.name))?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .with_context(|| format!("读取依赖下载页失败: {}", dependency.name))?;

        if !status.is_success() {
            emit_log(
                sender.as_ref(),
                translator
                    .translate_args(
                        "McDeps.logs.source_http_error",
                        crate::i18n_args![("pkg", &dependency.name)],
                    )
                    .into_owned(),
            );
            return Err(anyhow!("依赖源请求失败: {} ({status})", dependency.name));
        }

        let mut candidates = Vec::new();
        for capture in anchor_regex.captures_iter(&response_text) {
            let name = &capture["name"];
            let href = &capture["href"];
            let lower_name = name.to_ascii_lowercase();
            if (lower_name.contains("x64") || lower_name.contains("neutral"))
                && (lower_name.ends_with(".appx")
                    || lower_name.ends_with(".appxbundle")
                    || lower_name.ends_with(".msix")
                    || lower_name.ends_with(".msixbundle"))
            {
                candidates.push((name.to_string(), href.to_string()));
            }
        }

        let Some((asset_name, asset_url)) =
            select_best_candidate(candidates, dependency.min_version.as_deref())
        else {
            emit_log(
                sender.as_ref(),
                translator
                    .translate_args(
                        "McDeps.logs.no_candidates",
                        crate::i18n_args![("pkg", &dependency.name)],
                    )
                    .into_owned(),
            );
            return Err(anyhow!("未找到可用依赖安装包: {}", dependency.name));
        };
        debug!(
            dependency = %dependency.name,
            asset_name = %asset_name,
            asset_url = %asset_url,
            "已选择 UWP 依赖安装包"
        );

        emit_log(
            sender.as_ref(),
            translator
                .translate_args(
                    "McDeps.logs.download_start",
                    crate::i18n_args![("name", &asset_name)],
                )
                .into_owned(),
        );

        let download_path = env::temp_dir().join(&asset_name);
        download_file_with_progress(
            &client,
            &asset_url,
            &download_path,
            translator.translate("McDeps.stages.download").into_owned(),
            Some(asset_name.clone()),
            sender.as_ref(),
            None,
        )
        .await?;

        emit_progress(
            sender.as_ref(),
            80,
            translator
                .translate("McDeps.stages.installing")
                .into_owned(),
            Some(asset_name.clone()),
        );

        install_appx_package_from_file(&download_path).await?;

        emit_progress(
            sender.as_ref(),
            90,
            translator
                .translate("McDeps.stages.installing")
                .into_owned(),
            Some(dependency.name.clone()),
        );

        wait_for_condition(Duration::from_secs(90), Duration::from_secs(1), || {
            is_installed_with_min(&dependency.name, dependency.min_version.as_deref())
        })
        .await
        .with_context(|| format!("安装后未检测到依赖: {}", dependency.name))?;

        emit_log(
            sender.as_ref(),
            translator
                .translate_args(
                    "McDeps.logs.detect_installed",
                    crate::i18n_args![("pkg", &dependency.name)],
                )
                .into_owned(),
        );
        emit_progress(
            sender.as_ref(),
            100,
            translator.translate("McDeps.stages.done-one").into_owned(),
            Some(dependency.name.clone()),
        );
        info!(dependency = %dependency.name, "UWP 依赖安装并校验完成");
    }

    info!("所有缺失的 UWP 依赖安装完成");
    Ok(())
}

pub async fn install_game_input_runtime(
    locale: Locale,
    plan: GameInputInstallPlan,
    sender: Option<UnboundedSender<DependencyEvent>>,
) -> Result<()> {
    let translator = translator_for(locale);
    let client = get_client_for_proxy().unwrap_or_else(|_| GLOBAL_CLIENT.clone());
    let installer_name = plan
        .installer_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("GameInputRedist.msi")
        .to_string();
    info!(
        locale = ?locale,
        installer_name = %installer_name,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        "开始安装 GameInput Runtime"
    );

    if matches!(plan.source, GameInputInstallerSource::Download) {
        emit_log(
            sender.as_ref(),
            translator
                .translate_args(
                    "McDeps.logs.download_start",
                    crate::i18n_args![("name", &installer_name)],
                )
                .into_owned(),
        );
        download_file_with_progress(
            &client,
            GAMEINPUT_LATEST_DOWNLOAD_URL,
            &plan.installer_path,
            translator.translate("McDeps.stages.download").into_owned(),
            Some(installer_name.clone()),
            sender.as_ref(),
            None,
        )
        .await?;
    }

    emit_game_input_admin_notice(sender.as_ref(), &translator, &plan);

    emit_progress(
        sender.as_ref(),
        85,
        translator
            .translate("McDeps.stages.installing")
            .into_owned(),
        Some(installer_name.clone()),
    );

    let installer_path = plan.installer_path.clone();
    tokio::task::spawn_blocking(move || install_game_input_with_msi(&installer_path))
        .await
        .context("等待 GameInput MSI 安装线程失败")??;

    wait_for_condition(
        Duration::from_secs(90),
        Duration::from_secs(1),
        is_game_input_installed,
    )
    .await
    .context("安装后未检测到 GameInput Runtime")?;

    emit_progress(
        sender.as_ref(),
        100,
        translator.translate("McDeps.stages.done-one").into_owned(),
        Some(installer_name),
    );
    info!("GameInput Runtime 安装并校验完成");

    Ok(())
}

fn translator_for(locale: Locale) -> Translator {
    let mut translator = Translator::new();
    translator.set_locale(locale);
    translator
}

fn emit_log(sender: Option<&UnboundedSender<DependencyEvent>>, message: String) {
    if let Some(sender) = sender {
        let _ = sender.send(DependencyEvent::Log(message));
    }
}

fn emit_progress(
    sender: Option<&UnboundedSender<DependencyEvent>>,
    percent: u32,
    stage: String,
    target: Option<String>,
) {
    if let Some(sender) = sender {
        let _ = sender.send(DependencyEvent::Progress {
            percent: percent.min(100),
            stage,
            target,
        });
    }
}

fn emit_admin_required(sender: Option<&UnboundedSender<DependencyEvent>>, message: String) {
    if let Some(sender) = sender {
        let _ = sender.send(DependencyEvent::AdminRequired(message));
    }
}

#[cfg(windows)]
fn emit_game_input_admin_notice(
    sender: Option<&UnboundedSender<DependencyEvent>>,
    translator: &Translator,
    plan: &GameInputInstallPlan,
) {
    if !developer_mode::is_process_elevated() {
        warn!(
            installer_path = %plan.installer_path.display(),
            "安装 GameInput Runtime 可能需要管理员权限"
        );
        emit_admin_required(
            sender,
            translator
                .translate("LaunchPrereq.adminRunRequired")
                .into_owned(),
        );
    }
}

#[cfg(not(windows))]
fn emit_game_input_admin_notice(
    _sender: Option<&UnboundedSender<DependencyEvent>>,
    _translator: &Translator,
    _plan: &GameInputInstallPlan,
) {
}

async fn download_file_with_progress(
    client: &Client,
    url: &str,
    destination: &Path,
    stage_label: String,
    target: Option<String>,
    sender: Option<&UnboundedSender<DependencyEvent>>,
    request_headers: Option<&HeaderMap>,
) -> Result<()> {
    const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(120);

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }

    let mut request = client.get(url);
    if let Some(headers) = request_headers {
        request = request.headers(headers.clone());
    }
    let mut response = request
        .send()
        .await
        .with_context(|| format!("下载失败: {url}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("下载失败，HTTP {status}: {url}"));
    }

    let total_len = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut file = tokio::fs::File::create(destination)
        .await
        .with_context(|| format!("创建下载文件失败: {}", destination.display()))?;

    emit_progress(sender, 0, stage_label.clone(), target.clone());

    let start = Instant::now();
    let mut downloaded = 0u64;
    let mut last_emitted_at = start;
    let mut last_emitted_percent = Some(0);
    while let Some(chunk) = response.chunk().await.context("读取下载数据失败")? {
        file.write_all(&chunk)
            .await
            .with_context(|| format!("写入下载文件失败: {}", destination.display()))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);

        let percent = total_len
            .map(|total| ((downloaded as f64 / total as f64) * 100.0).round() as u32)
            .unwrap_or_else(|| ((downloaded / 1024) % 100) as u32);
        let now = Instant::now();
        let should_emit = percent >= 100
            || last_emitted_percent != Some(percent)
                && now.saturating_duration_since(last_emitted_at) >= PROGRESS_EMIT_INTERVAL;
        if should_emit {
            emit_progress(sender, percent, stage_label.clone(), target.clone());
            last_emitted_at = now;
            last_emitted_percent = Some(percent);
        }
    }

    file.flush()
        .await
        .with_context(|| format!("刷新下载文件失败: {}", destination.display()))?;
    debug!(
        "依赖下载完成: url={}, path={}, elapsed={:.2?}",
        url,
        destination.display(),
        start.elapsed()
    );
    emit_progress(sender, 100, stage_label, target);

    Ok(())
}

#[cfg(windows)]
async fn install_appx_package_from_file(package_path: &Path) -> Result<()> {
    let package_path = package_path.to_path_buf();
    tokio::task::spawn_blocking(move || install_appx_package_from_file_blocking(&package_path))
        .await
        .context("等待 APPX 安装线程失败")??;
    Ok(())
}

#[cfg(not(windows))]
async fn install_appx_package_from_file(_package_path: &Path) -> Result<()> {
    Err(anyhow!("UWP 依赖只能在 Windows 上安装"))
}

#[cfg(windows)]
fn install_appx_package_from_file_blocking(package_path: &Path) -> Result<()> {
    info!(package_path = %package_path.display(), "开始通过 PackageManager 安装 APPX 依赖");
    let package_manager = PackageManager::new().context("无法创建 PackageManager")?;
    let package_uri = Uri::CreateUri(&HSTRING::from(path_to_file_uri(package_path)?))
        .context("无法创建 APPX 文件 URI")?;
    let options = AddPackageOptions::new().context("无法创建 AddPackageOptions")?;
    options.SetForceAppShutdown(true).ok();
    options.SetForceTargetAppShutdown(true).ok();
    options.SetRetainFilesOnFailure(true).ok();

    let result = package_manager
        .AddPackageByUriAsync(&package_uri, &options)
        .context("启动 APPX 安装失败")?
        .join()
        .context("等待 APPX 安装完成失败")?;

    let extended_error = result.ExtendedErrorCode().unwrap_or(HRESULT(0));
    if extended_error != HRESULT(0) {
        let error_text = result
            .ErrorText()
            .map(|value| value.to_string_lossy())
            .unwrap_or_else(|_| String::new());
        return Err(anyhow!(
            "APPX 安装失败: code={:?}, message={}",
            extended_error,
            error_text
        ));
    }

    info!(package_path = %package_path.display(), "APPX 依赖安装完成");
    Ok(())
}

#[cfg(windows)]
fn install_game_input_with_msi(installer_path: &Path) -> Result<()> {
    info!(installer_path = %installer_path.display(), "开始执行 GameInput MSI 安装");
    let installer = HSTRING::from(installer_path.to_string_lossy().to_string());

    unsafe {
        let previous_ui = MsiSetInternalUI(INSTALLUILEVEL_NONE, None);
        let result = MsiInstallProductW(PCWSTR(installer.as_ptr()), PCWSTR::null());
        let _ = MsiSetInternalUI(previous_ui, None);

        if result == 0 {
            info!(installer_path = %installer_path.display(), "GameInput MSI 安装调用返回成功");
            Ok(())
        } else {
            Err(anyhow!("GameInput MSI 安装失败，错误码 {result}"))
        }
    }
}

#[cfg(not(windows))]
fn install_game_input_with_msi(_installer_path: &Path) -> Result<()> {
    Err(anyhow!("GameInput Runtime 只能在 Windows 上安装"))
}

#[cfg(windows)]
fn path_to_file_uri(path: &Path) -> Result<String> {
    let canonical_path = std::fs::canonicalize(path)
        .with_context(|| format!("获取文件绝对路径失败: {}", path.display()))?;
    Url::from_file_path(&canonical_path)
        .map(|value| value.to_string())
        .map_err(|_| anyhow!("无法转换文件 URI: {}", canonical_path.display()))
}

async fn wait_for_condition<F>(
    timeout: Duration,
    interval: Duration,
    mut condition: F,
) -> Result<()>
where
    F: FnMut() -> bool,
{
    let started_at = Instant::now();
    while started_at.elapsed() <= timeout {
        if condition() {
            return Ok(());
        }
        sleep(interval).await;
    }

    Err(anyhow!("等待条件超时"))
}
