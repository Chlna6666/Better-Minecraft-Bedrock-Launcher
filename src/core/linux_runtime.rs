use crate::downloads::manager::{DownloadOptions, DownloaderManager};
use crate::result::CoreResult;
use crate::tasks::task_manager::{
    append_task_log, create_task_with_details, finish_task, set_task_message, update_progress,
};
use crate::utils::file_ops;
use std::env;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tracing::{info, warn};

pub(crate) const PROTON_GDK_RELEASE_SOURCES: [&str; 3] = [
    "RoundMCDev/ProtonGDK-Release",
    "Weather-OS/GDK-Proton",
    "LukasPAH/GDK-Proton-Custom",
];

const PROTON_GDK_METADATA_FILE: &str = ".bmcbl-proton-gdk.json";
const PROTON_GDK_METADATA_SCHEMA_VERSION: u32 = 1;
const ROUNDMCDEV_RELEASE_TAG: &str = "Release10-32";
const ROUNDMCDEV_RELEASE_NAME: &str = "GDK-Proton10-32-Kits.01";

#[derive(Clone, Copy, Debug)]
struct RoundMcDevAsset {
    name: &'static str,
    url: &'static str,
    expected_size: u64,
    extraction_directory: &'static str,
}

const ROUNDMCDEV_ASSETS: [RoundMcDevAsset; 4] = [
    RoundMcDevAsset {
        name: "GameRunningFixKit.tar.gz",
        url: "https://github.com/RoundMCDev/ProtonGDK-Release/releases/download/Release10-32/GameRunningFixKit.tar.gz",
        expected_size: 11_643_764,
        extraction_directory: ".",
    },
    RoundMcDevAsset {
        name: "GDK-Proton-xuser.tar.gz",
        url: "https://github.com/RoundMCDev/ProtonGDK-Release/releases/download/Release10-32/GDK-Proton-xuser.tar.gz",
        expected_size: 862_210_941,
        extraction_directory: "proton",
    },
    RoundMcDevAsset {
        name: "Proton-Launch-umu.tar.gz",
        url: "https://github.com/RoundMCDev/ProtonGDK-Release/releases/download/Release10-32/Proton-Launch-umu.tar.gz",
        expected_size: 289_719_588,
        extraction_directory: ".",
    },
    RoundMcDevAsset {
        name: "GamePatch.zip",
        url: "https://github.com/RoundMCDev/ProtonGDK-Release/releases/download/Release10-32/GamePatch.zip",
        expected_size: 2_459_214,
        extraction_directory: "GamePatch",
    },
];

#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ProtonGdkRunnerMetadata {
    schema_version: u32,
    source: String,
    repository: String,
    release_tag: String,
    release_name: String,
    asset_name: String,
    #[serde(default)]
    asset_names: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProtonGdkSource {
    RoundMcDev,
    WeatherOs,
    LukasPah,
}

impl ProtonGdkSource {
    pub(crate) fn from_config(value: &str) -> Self {
        Self::from_stored_value(value).unwrap_or(Self::RoundMcDev)
    }

    fn from_stored_value(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("roundmcdev") {
            Some(Self::RoundMcDev)
        } else if value.eq_ignore_ascii_case("weather-os") {
            Some(Self::WeatherOs)
        } else if value.eq_ignore_ascii_case("lukaspah") {
            Some(Self::LukasPah)
        } else {
            None
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::RoundMcDev => "roundmcdev",
            Self::WeatherOs => "weather-os",
            Self::LukasPah => "lukaspah",
        }
    }

    pub(crate) fn repository(self) -> &'static str {
        match self {
            Self::RoundMcDev => "RoundMCDev/ProtonGDK-Release",
            Self::WeatherOs => "Weather-OS/GDK-Proton",
            Self::LukasPah => "LukasPAH/GDK-Proton-Custom",
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::RoundMcDev => "RoundMCDev",
            Self::WeatherOs => "Weather-OS",
            Self::LukasPah => "LukasPAH",
        }
    }

    pub(crate) fn login_capability(self) -> &'static str {
        match self {
            Self::RoundMcDev => "支持登录",
            Self::WeatherOs | Self::LukasPah => "无法登录",
        }
    }

    fn latest_release_api(self) -> &'static str {
        match self {
            Self::RoundMcDev => {
                "https://api.github.com/repos/RoundMCDev/ProtonGDK-Release/releases/latest"
            }
            Self::WeatherOs => "https://api.github.com/repos/Weather-OS/GDK-Proton/releases/latest",
            Self::LukasPah => {
                "https://api.github.com/repos/LukasPAH/GDK-Proton-Custom/releases/latest"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtonGdkIdentity {
    Metadata,
    DirectoryName,
    Unknown,
}

#[derive(Clone, Debug)]
pub(crate) struct InstalledProtonGdkRunner {
    executable: PathBuf,
    display_name: String,
    source: Option<ProtonGdkSource>,
    release_tag: Option<String>,
    asset_names: Vec<String>,
    identity: ProtonGdkIdentity,
}

impl InstalledProtonGdkRunner {
    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn release_tag(&self) -> Option<&str> {
        self.release_tag.as_deref()
    }

    pub(crate) fn bundle_asset_count(&self) -> usize {
        self.asset_names.len()
    }

    pub(crate) fn source_label(&self) -> &'static str {
        self.source
            .map_or("来源未知", ProtonGdkSource::display_name)
    }

    pub(crate) fn login_capability(&self) -> &'static str {
        self.source
            .map_or("登录能力未知", ProtonGdkSource::login_capability)
    }

    pub(crate) fn identity_label(&self) -> &'static str {
        match self.identity {
            ProtonGdkIdentity::Metadata => "安装记录",
            ProtonGdkIdentity::DirectoryName => "目录识别",
            ProtonGdkIdentity::Unknown => "未识别",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunnerKind {
    Proton,
    Umu,
    Wine,
}

impl RunnerKind {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Proton => "Proton",
            Self::Umu => "UMU/Proton-GDK",
            Self::Wine => "Wine",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Runner {
    pub(crate) executable: PathBuf,
    pub(crate) kind: RunnerKind,
    pub(crate) steam_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct LinuxInstallPlan {
    pub(crate) distribution_name: Arc<str>,
    pub(crate) authorization_program: PathBuf,
    pub(crate) package_manager: PathBuf,
    pub(crate) arguments: Arc<[Arc<str>]>,
    pub(crate) packages: Arc<[Arc<str>]>,
}

impl LinuxInstallPlan {
    pub(crate) fn command_preview(&self) -> String {
        std::iter::once(self.authorization_program.to_string_lossy().into_owned())
            .chain(std::iter::once(
                self.package_manager.to_string_lossy().into_owned(),
            ))
            .chain(self.arguments.iter().map(ToString::to_string))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LinuxRuntimeCheck {
    pub(crate) runner: Option<Runner>,
    pub(crate) missing_reason: Option<Arc<str>>,
    pub(crate) distribution_name: Arc<str>,
    pub(crate) install_plan: Option<LinuxInstallPlan>,
    pub(crate) manual_install_hint: Arc<str>,
}

impl LinuxRuntimeCheck {
    pub(crate) fn is_ready(&self) -> bool {
        self.runner.is_some()
    }
}

#[derive(Default)]
struct OsRelease {
    id: String,
    id_like: String,
    pretty_name: String,
}

pub(crate) fn check_linux_runtime() -> LinuxRuntimeCheck {
    let os_release = detect_os_release();
    let distribution_name: Arc<str> = if os_release.pretty_name.is_empty() {
        Arc::from("未知 Linux 发行版")
    } else {
        Arc::from(os_release.pretty_name.as_str())
    };
    match resolve_proton_runner() {
        Ok(runner) => match validate_proton_game_runtime(&runner) {
            Ok(()) => LinuxRuntimeCheck {
                distribution_name,
                runner: Some(runner),
                missing_reason: None,
                install_plan: None,
                manual_install_hint: Arc::from(""),
            },
            Err(reason) => {
                let missing_i386_loader = reason.contains("/lib/ld-linux.so.2");
                LinuxRuntimeCheck {
                    runner: None,
                    missing_reason: Some(Arc::from(reason)),
                    install_plan: missing_i386_loader
                        .then(|| {
                            build_proton_host_dependencies_plan(
                                &os_release,
                                distribution_name.clone(),
                            )
                        })
                        .flatten(),
                    distribution_name,
                    manual_install_hint: if missing_i386_loader {
                        Arc::from(
                            "GDK-Proton 的游戏 runner 需要 32 位 glibc。可授权系统包管理器安装，或手动安装后重新检测。",
                        )
                    } else {
                        Arc::from(
                            "请前往 Proton-GDK 设置页安装并选择 RoundMCDev 版本，不要为此错误授权安装系统软件包。",
                        )
                    },
                }
            }
        },
        Err(reason) => LinuxRuntimeCheck {
            runner: None,
            missing_reason: Some(Arc::from(reason)),
            distribution_name,
            install_plan: None,
            manual_install_hint: Arc::from(
                "请前往 Proton-GDK 设置页安装或管理运行环境；安装过程不需要管理员权限。",
            ),
        },
    }
}

pub(crate) fn validate_proton_game_runtime(runner: &Runner) -> Result<(), String> {
    if runner.kind == RunnerKind::Wine {
        return Ok(());
    }
    let Some(proton_root) = runner_runtime_root(runner) else {
        return Ok(());
    };
    if proton_root.join("files/bin/wine").is_file() && !Path::new("/lib/ld-linux.so.2").is_file() {
        return Err(
            "已安装 Proton-GDK，但系统缺少 32 位 glibc 加载器 /lib/ld-linux.so.2；Minecraft GDK 不能使用简化的 WoW64 模式启动"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn resolve_proton_runner() -> Result<Runner, String> {
    let runner = resolve_runner()?;
    if runner.kind != RunnerKind::Wine {
        Ok(runner)
    } else {
        Err("已检测到 Wine，但 Minecraft UWP/GDK 版本需要 Proton/UMU".to_string())
    }
}

pub(crate) fn roundmcdev_bundle_root(runner: &Runner) -> Option<PathBuf> {
    find_bundle_root_for_path(&runner.executable)
}

pub(crate) fn runner_runtime_root(runner: &Runner) -> Option<PathBuf> {
    match runner.kind {
        RunnerKind::Umu => {
            let bundle_root = roundmcdev_bundle_root(runner)?;
            find_gdk_proton_root(&bundle_root.join("proton"))
        }
        RunnerKind::Proton => proton_gdk_runner_root(&runner.executable),
        RunnerKind::Wine => runner.executable.parent().map(Path::to_path_buf),
    }
}

fn find_bundle_root_for_path(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent()?.to_path_buf();
    for _ in 0..6 {
        if current.join("gameFix").is_dir()
            && current.join("GamePatch/gdk/mcpatcher_core.dll").is_file()
        {
            return Some(current);
        }
        current = current.parent()?.to_path_buf();
    }
    None
}

pub(crate) fn start_proton_gdk_install_latest(source: ProtonGdkSource) -> String {
    let release_label = match source {
        ProtonGdkSource::RoundMcDev => ROUNDMCDEV_RELEASE_TAG,
        ProtonGdkSource::WeatherOs | ProtonGdkSource::LukasPah => "latest",
    };
    let task_id = create_task_with_details(
        None,
        "安装 Proton-GDK",
        Some(format!("{} · {release_label}", source.repository())),
        "resolving_proton_gdk",
        None,
        false,
    );
    if source == ProtonGdkSource::RoundMcDev {
        append_task_log(
            &task_id,
            format!("使用 RoundMCDev 固定资源包：{ROUNDMCDEV_RELEASE_TAG}（支持登录）"),
        );
        set_task_message(&task_id, Some("准备 RoundMCDev 登录运行环境".to_string()));
    } else {
        append_task_log(
            &task_id,
            format!("正在获取 {} 最新版本", source.repository()),
        );
        set_task_message(&task_id, Some("正在获取可安装版本".to_string()));
    }

    let worker_task_id = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        match install_latest_proton_gdk(source, &worker_task_id).await {
            Ok(install_path) => finish_task(
                &worker_task_id,
                "completed",
                Some(format!("Proton-GDK 已安装到 {}", install_path.display())),
            ),
            Err(error) => {
                append_task_log(&worker_task_id, format!("安装失败：{error}"));
                finish_task(&worker_task_id, "error", Some(error));
            }
        }
    }) {
        finish_task(
            &task_id,
            "error",
            Some(format!("无法调度 Proton-GDK 安装任务：{error}")),
        );
    }
    task_id
}

async fn install_latest_proton_gdk(
    source: ProtonGdkSource,
    task_id: &str,
) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .user_agent("BMCBL-Proton-GDK")
        .build()
        .map_err(|error| format!("创建 GitHub 客户端失败：{error}"))?;
    if source == ProtonGdkSource::RoundMcDev {
        return install_roundmcdev_bundle(&client, task_id).await;
    }

    let release = client
        .get(source.latest_release_api())
        .send()
        .await
        .map_err(|error| format!("获取 Proton-GDK 版本失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub 返回错误：{error}"))?
        .json::<GithubRelease>()
        .await
        .map_err(|error| format!("解析 Proton-GDK 版本失败：{error}"))?;
    let asset = release
        .assets
        .iter()
        .find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.contains("proton") && (name.ends_with(".tar.gz") || name.ends_with(".tgz"))
        })
        .ok_or_else(|| "最新版本没有可安装的 Proton-GDK tar.gz 资源".to_string())?;
    let release_name = release
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&release.tag_name)
        .to_string();
    let version_name = proton_gdk_install_directory_name(source, &release_name);
    let install_path = file_ops::runners_dir().join(&version_name);
    let metadata = ProtonGdkRunnerMetadata {
        schema_version: PROTON_GDK_METADATA_SCHEMA_VERSION,
        source: source.config_value().to_string(),
        repository: source.repository().to_string(),
        release_tag: release.tag_name.clone(),
        release_name,
        asset_name: asset.name.clone(),
        asset_names: vec![asset.name.clone()],
    };
    if install_path.exists() {
        if let Some(proton) = find_proton_file(&install_path) {
            append_task_log(
                task_id,
                format!(
                    "检测到已有 Proton-GDK 文件，正在修复安装：{}",
                    proton.display()
                ),
            );
            finalize_proton_gdk_install(source, &install_path, &proton, &metadata).await?;
            return Ok(install_path);
        }
        preserve_incomplete_proton_gdk_install(&install_path).await?;
    }

    let download_dir = file_ops::downloads_dir()
        .join("proton-gdk")
        .join(source.config_value());
    tokio::fs::create_dir_all(&download_dir)
        .await
        .map_err(|error| format!("创建 Proton-GDK 下载目录失败：{error}"))?;
    let archive_path = download_dir.join(&asset.name);
    download_proton_gdk_asset(
        &client,
        &asset.name,
        &asset.browser_download_url,
        asset.size,
        &archive_path,
        task_id,
    )
    .await?;

    let staging_path = file_ops::runners_dir().join(format!(
        ".{version_name}.installing-{}",
        sanitize_instance_name(task_id)
    ));
    if staging_path.exists() {
        tokio::fs::remove_dir_all(&staging_path)
            .await
            .map_err(|error| format!("清理 Proton-GDK 临时安装目录失败：{error}"))?;
    }
    tokio::fs::create_dir_all(&staging_path)
        .await
        .map_err(|error| format!("创建 Proton-GDK 临时安装目录失败：{error}"))?;
    update_progress(task_id, 0, None, Some("extracting_proton_gdk"));
    set_task_message(task_id, Some("正在解压 Proton-GDK".to_string()));
    append_task_log(
        task_id,
        format!("解压到临时目录 {}", staging_path.display()),
    );
    let output = match tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&staging_path)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => {
            if let Err(cleanup_error) = tokio::fs::remove_dir_all(&staging_path).await {
                append_task_log(
                    task_id,
                    format!("清理未启动解压的临时目录失败：{cleanup_error}"),
                );
            }
            return Err(format!("无法启动 tar：{error}"));
        }
    };
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if let Err(cleanup_error) = tokio::fs::remove_dir_all(&staging_path).await {
            append_task_log(
                task_id,
                format!("清理解压失败的临时目录失败：{cleanup_error}"),
            );
        }
        return Err(format!("解压 Proton-GDK 失败：{}", error.trim()));
    }

    let proton = match promote_staged_proton_gdk(&staging_path, &install_path).await {
        Ok(proton) => proton,
        Err(error) => {
            if let Err(cleanup_error) = tokio::fs::remove_dir_all(&staging_path).await {
                append_task_log(
                    task_id,
                    format!("清理无效 Proton-GDK 临时目录失败：{cleanup_error}"),
                );
            }
            return Err(error);
        }
    };
    if staging_path.exists()
        && let Err(error) = tokio::fs::remove_dir_all(&staging_path).await
    {
        append_task_log(task_id, format!("清理 Proton-GDK 临时目录失败：{error}"));
    }
    finalize_proton_gdk_install(source, &install_path, &proton, &metadata).await?;
    Ok(install_path)
}

async fn install_roundmcdev_bundle(
    client: &reqwest::Client,
    task_id: &str,
) -> Result<PathBuf, String> {
    let source = ProtonGdkSource::RoundMcDev;
    let version_name = proton_gdk_install_directory_name(source, ROUNDMCDEV_RELEASE_NAME);
    let install_path = file_ops::runners_dir().join(&version_name);
    let metadata = ProtonGdkRunnerMetadata {
        schema_version: PROTON_GDK_METADATA_SCHEMA_VERSION,
        source: source.config_value().to_string(),
        repository: source.repository().to_string(),
        release_tag: ROUNDMCDEV_RELEASE_TAG.to_string(),
        release_name: ROUNDMCDEV_RELEASE_NAME.to_string(),
        asset_name: "RoundMCDev ProtonGDK kit bundle".to_string(),
        asset_names: ROUNDMCDEV_ASSETS
            .iter()
            .map(|asset| asset.name.to_string())
            .collect(),
    };

    if install_path.exists() {
        if roundmcdev_bundle_is_complete(&install_path) {
            append_task_log(
                task_id,
                format!(
                    "检测到完整的 RoundMCDev 运行环境：{}",
                    install_path.display()
                ),
            );
            finalize_roundmcdev_install(&install_path, &metadata, task_id).await?;
            return Ok(install_path);
        }
        preserve_incomplete_proton_gdk_install(&install_path).await?;
    }

    let download_dir = file_ops::downloads_dir()
        .join("proton-gdk")
        .join(source.config_value())
        .join(ROUNDMCDEV_RELEASE_TAG);
    tokio::fs::create_dir_all(&download_dir)
        .await
        .map_err(|error| format!("创建 RoundMCDev 下载目录失败：{error}"))?;

    let mut archive_paths = Vec::with_capacity(ROUNDMCDEV_ASSETS.len());
    for asset in ROUNDMCDEV_ASSETS {
        let archive_path = download_dir.join(asset.name);
        download_proton_gdk_asset(
            client,
            asset.name,
            asset.url,
            asset.expected_size,
            &archive_path,
            task_id,
        )
        .await?;
        archive_paths.push((asset, archive_path));
    }

    let staging_path = file_ops::runners_dir().join(format!(
        ".{version_name}.installing-{}",
        sanitize_instance_name(task_id)
    ));
    if staging_path.exists() {
        tokio::fs::remove_dir_all(&staging_path)
            .await
            .map_err(|error| format!("清理 RoundMCDev 临时安装目录失败：{error}"))?;
    }
    tokio::fs::create_dir_all(&staging_path)
        .await
        .map_err(|error| format!("创建 RoundMCDev 临时安装目录失败：{error}"))?;

    update_progress(task_id, 0, None, Some("extracting_proton_gdk"));
    for (asset, archive_path) in archive_paths {
        let extraction_directory = if asset.extraction_directory == "." {
            staging_path.clone()
        } else {
            staging_path.join(asset.extraction_directory)
        };
        tokio::fs::create_dir_all(&extraction_directory)
            .await
            .map_err(|error| {
                format!(
                    "创建 {} 解压目录失败：{} ({error})",
                    asset.name,
                    extraction_directory.display()
                )
            })?;
        append_task_log(
            task_id,
            format!("解压 {} 到 {}", asset.name, extraction_directory.display()),
        );
        extract_roundmcdev_asset(&archive_path, &extraction_directory, asset.name).await?;
    }

    if !roundmcdev_bundle_is_complete(&staging_path) {
        tokio::fs::remove_dir_all(&staging_path)
            .await
            .map_err(|error| format!("清理不完整的 RoundMCDev 临时安装目录失败：{error}"))?;
        return Err(
            "RoundMCDev 资源包解压完成，但缺少 proton、umu、gameFix 或 GamePatch/gdk/mcpatcher_core.dll"
                .to_string(),
        );
    }

    promote_staged_roundmcdev_bundle(&staging_path, &install_path).await?;
    finalize_roundmcdev_install(&install_path, &metadata, task_id).await?;
    Ok(install_path)
}

async fn finalize_roundmcdev_install(
    install_path: &Path,
    metadata: &ProtonGdkRunnerMetadata,
    task_id: &str,
) -> Result<(), String> {
    let umu_runner = find_named_file(&install_path.join("umu"), "umu-run")
        .ok_or_else(|| "RoundMCDev 安装目录中没有找到 umu/umu-run".to_string())?;
    let gdk_root = find_gdk_proton_root(&install_path.join("proton"))
        .ok_or_else(|| "RoundMCDev 安装目录中没有找到 GDK-Proton 根目录".to_string())?;
    let game_fix = install_path.join("gameFix");
    if !game_fix.is_dir() {
        return Err(format!(
            "RoundMCDev 安装目录中没有找到 GameRunningFixKit：{}",
            game_fix.display()
        ));
    }
    let game_patch = install_path.join("GamePatch/gdk/mcpatcher_core.dll");
    if !game_patch.is_file() {
        return Err(format!(
            "RoundMCDev 安装目录中没有找到 GamePatch：{}",
            game_patch.display()
        ));
    }
    let install_path_for_permissions = install_path.to_path_buf();
    crate::tasks::runtime::run_io_blocking(move || {
        make_roundmcdev_bundle_files_executable(&install_path_for_permissions)
    })
    .await
    .map_err(|error| format!("设置 RoundMCDev 运行包权限任务失败：{error}"))??;
    let executables = [
        umu_runner.clone(),
        gdk_root.join("proton"),
        gdk_root.join("files/bin/wine"),
        gdk_root.join("files/bin/wineboot"),
        gdk_root.join("files/bin/wineserver"),
    ];
    for executable in executables {
        if !executable.is_file() {
            continue;
        }
        let mut permissions = tokio::fs::metadata(&executable)
            .await
            .map_err(|error| format!("读取 {} 权限失败：{error}", executable.display()))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        tokio::fs::set_permissions(&executable, permissions)
            .await
            .map_err(|error| format!("设置 {} 可执行权限失败：{error}", executable.display()))?;
    }
    write_proton_gdk_metadata(install_path, metadata).await?;
    append_task_log(
        task_id,
        format!(
            "RoundMCDev 四组件安装完成：GameRunningFixKit={}；GDK-Proton={}；Proton-Launch-umu={}；GamePatch={}",
            game_fix.display(),
            gdk_root.display(),
            umu_runner.display(),
            game_patch.display()
        ),
    );
    let selected_runner = umu_runner.to_string_lossy().into_owned();
    crate::config::config::update_config(|config| {
        config.launcher.proton_gdk_runner = selected_runner.clone();
        config.launcher.proton_gdk_source = ProtonGdkSource::RoundMcDev.config_value().to_string();
    })
    .map_err(|error| format!("保存 Proton-GDK 默认版本失败：{error}"))?;
    info!(
        source = ProtonGdkSource::RoundMcDev.config_value(),
        install_path = %install_path.display(),
        game_fix = %game_fix.display(),
        gdk_proton = %gdk_root.display(),
        umu = %umu_runner.display(),
        game_patch = %game_patch.display(),
        "RoundMCDev UMU Proton-GDK installation finalized"
    );
    Ok(())
}

fn make_roundmcdev_bundle_files_executable(bundle_root: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(bundle_root)
        .map_err(|error| format!("读取 RoundMCDev 运行包失败：{error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取 RoundMCDev 运行包条目失败：{error}"))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("读取 {} 权限失败：{error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            make_roundmcdev_bundle_files_executable(&path)?;
            continue;
        }
        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        std::fs::set_permissions(&path, permissions)
            .map_err(|error| format!("设置 {} 可执行权限失败：{error}", path.display()))?;
    }
    Ok(())
}

async fn promote_staged_proton_gdk(
    staging_path: &Path,
    install_path: &Path,
) -> Result<PathBuf, String> {
    let staged_proton = find_proton_file(staging_path).ok_or_else(|| {
        format!(
            "安装包中没有 proton 或 bin/proton：{}",
            staging_path.display()
        )
    })?;
    let staged_runner_root = proton_gdk_runner_root(&staged_proton)
        .ok_or_else(|| "无法确定 Proton-GDK 解压后的运行器目录".to_string())?;
    let proton_relative_path = staged_proton
        .strip_prefix(&staged_runner_root)
        .map(Path::to_path_buf)
        .map_err(|error| format!("无法确定 Proton-GDK 可执行文件相对路径：{error}"))?;
    tokio::fs::rename(&staged_runner_root, install_path)
        .await
        .map_err(|error| format!("完成 Proton-GDK 原子安装失败：{error}"))?;
    Ok(install_path.join(proton_relative_path))
}

async fn finalize_proton_gdk_install(
    source: ProtonGdkSource,
    install_path: &Path,
    proton: &Path,
    metadata: &ProtonGdkRunnerMetadata,
) -> Result<(), String> {
    let mut permissions = tokio::fs::metadata(&proton)
        .await
        .map_err(|error| format!("读取 Proton-GDK 权限失败：{error}"))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    tokio::fs::set_permissions(&proton, permissions)
        .await
        .map_err(|error| format!("设置 Proton-GDK 可执行权限失败：{error}"))?;
    let runner_root = proton_gdk_runner_root(proton)
        .ok_or_else(|| "无法确定 Proton-GDK 安装记录目录".to_string())?;
    write_proton_gdk_metadata(&runner_root, metadata).await?;
    let selected_runner = proton.to_string_lossy().into_owned();
    crate::config::config::update_config(|config| {
        config.launcher.proton_gdk_runner = selected_runner.clone();
        config.launcher.proton_gdk_source = source.config_value().to_string();
    })
    .map_err(|error| format!("保存 Proton-GDK 默认版本失败：{error}"))?;
    info!(
        source = source.config_value(),
        install_path = %install_path.display(),
        proton = %proton.display(),
        "Proton-GDK installation finalized"
    );
    Ok(())
}

async fn preserve_incomplete_proton_gdk_install(install_path: &Path) -> Result<(), String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("生成 Proton-GDK 残留目录时间戳失败：{error}"))?
        .as_secs();
    let file_name = install_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("proton-gdk");
    let backup_path = install_path.with_file_name(format!("{file_name}.incomplete-{timestamp}"));
    tokio::fs::rename(install_path, &backup_path)
        .await
        .map_err(|error| {
            format!(
                "保存 Proton-GDK 未完成安装失败（{} -> {}）：{error}",
                install_path.display(),
                backup_path.display()
            )
        })
}

async fn write_proton_gdk_metadata(
    install_path: &Path,
    metadata: &ProtonGdkRunnerMetadata,
) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(metadata)
        .map_err(|error| format!("生成 Proton-GDK 安装记录失败：{error}"))?;
    tokio::fs::write(install_path.join(PROTON_GDK_METADATA_FILE), contents)
        .await
        .map_err(|error| format!("保存 Proton-GDK 安装记录失败：{error}"))
}

async fn download_proton_gdk_asset(
    client: &reqwest::Client,
    asset_name: &str,
    asset_url: &str,
    expected_size: u64,
    archive_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    if expected_size > 0
        && tokio::fs::metadata(archive_path)
            .await
            .is_ok_and(|metadata| metadata.len() == expected_size)
    {
        update_progress(
            task_id,
            expected_size,
            Some(expected_size),
            Some("downloading_proton_gdk"),
        );
        set_task_message(task_id, Some(format!("使用缓存 {}", asset_name)));
        append_task_log(task_id, format!("使用缓存：{}", archive_path.display()));
        append_task_log(task_id, format!("资源 {asset_name} 已准备完成"));
        return Ok(());
    }

    update_progress(
        task_id,
        0,
        (expected_size > 0).then_some(expected_size),
        Some("downloading_proton_gdk"),
    );
    set_task_message(task_id, Some(format!("正在下载 {asset_name}")));
    append_task_log(task_id, format!("下载：{asset_url}"));

    let manager = DownloaderManager::with_client(client.clone());
    let options = DownloadOptions::default();
    let result = manager
        .download_with_options(
            task_id,
            asset_url.to_string(),
            archive_path.to_path_buf(),
            &options,
        )
        .await
        .map_err(|error| format!("下载 Proton-GDK 失败：{error:?}"))?;

    match result {
        CoreResult::Success(path) => {
            if expected_size > 0 {
                let actual_size = tokio::fs::metadata(&path)
                    .await
                    .map_err(|error| format!("读取 {asset_name} 下载文件大小失败：{error}"))?
                    .len();
                if actual_size != expected_size {
                    return Err(format!(
                        "{asset_name} 下载大小不匹配：期望 {expected_size} 字节，实际 {actual_size} 字节"
                    ));
                }
            }
            append_task_log(
                task_id,
                format!("资源 {asset_name} 下载完成：{}", path.display()),
            );
            Ok(())
        }
        CoreResult::Cancelled => Err("下载已取消".to_string()),
        CoreResult::Error(error) => Err(format!("下载 Proton-GDK 失败：{error:?}")),
    }
}

async fn extract_roundmcdev_asset(
    archive_path: &Path,
    destination: &Path,
    asset_name: &str,
) -> Result<(), String> {
    if asset_name.ends_with(".zip") {
        let archive_path = archive_path.to_path_buf();
        let destination = destination.to_path_buf();
        crate::tasks::runtime::run_archive_blocking(move || {
            extract_roundmcdev_zip_blocking(&archive_path, &destination)
        })
        .await
        .map_err(|error| format!("解压 {asset_name} 的归档任务失败：{error}"))??;
        return Ok(());
    }

    let output = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive_path)
        .arg("-C")
        .arg(destination)
        .output()
        .await
        .map_err(|error| format!("无法启动 tar 解压 {asset_name}：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "解压 {asset_name} 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn extract_roundmcdev_zip_blocking(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("打开 {} 失败：{error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("读取 {} 失败：{error}", archive_path.display()))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 zip 条目 #{index} 失败：{error}"))?;
        let entry_name = entry
            .name()
            .map_err(|error| format!("读取 zip 条目名称 #{index} 失败：{error}"))?
            .into_owned();
        let relative_path = Path::new(&entry_name);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!("zip 条目包含不安全路径：{entry_name}"));
        }

        let output_path = destination.join(relative_path);
        if entry.is_dir() {
            std::fs::create_dir_all(&output_path)
                .map_err(|error| format!("创建目录 {} 失败：{error}", output_path.display()))?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建目录 {} 失败：{error}", parent.display()))?;
        }
        let mut output = File::create(&output_path)
            .map_err(|error| format!("创建文件 {} 失败：{error}", output_path.display()))?;
        io::copy(&mut entry, &mut output)
            .map_err(|error| format!("写入文件 {} 失败：{error}", output_path.display()))?;
        output
            .flush()
            .map_err(|error| format!("刷新文件 {} 失败：{error}", output_path.display()))?;

        if let Some(mode) = entry.unix_mode() {
            std::fs::set_permissions(&output_path, std::fs::Permissions::from_mode(mode & 0o7777))
                .map_err(|error| format!("设置文件 {} 权限失败：{error}", output_path.display()))?;
        }
    }
    Ok(())
}

fn roundmcdev_bundle_is_complete(bundle_root: &Path) -> bool {
    find_gdk_proton_root(&bundle_root.join("proton")).is_some_and(|proton_root| {
        proton_root.join("proton").is_file()
            && find_named_file(&bundle_root.join("umu"), "umu-run").is_some()
    }) && bundle_root.join("gameFix").is_dir()
        && bundle_root
            .join("GamePatch/gdk/mcpatcher_core.dll")
            .is_file()
}

fn find_named_file(search_root: &Path, file_name: &str) -> Option<PathBuf> {
    if search_root
        .file_name()
        .is_some_and(|name| name == file_name)
        && search_root.is_file()
    {
        return Some(search_root.to_path_buf());
    }

    let entries = std::fs::read_dir(search_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    let mut matching_files = entries
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == file_name) && path.is_file())
        .cloned()
        .collect::<Vec<_>>();
    matching_files.sort();
    if let Some(file) = matching_files.into_iter().next() {
        return Some(file);
    }
    let mut child_directories = entries
        .into_iter()
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    child_directories.sort();
    child_directories
        .into_iter()
        .find_map(|directory| find_named_file(&directory, file_name))
}

async fn promote_staged_roundmcdev_bundle(
    staging_path: &Path,
    install_path: &Path,
) -> Result<PathBuf, String> {
    if install_path.exists() {
        return Err(format!(
            "RoundMCDev 安装目录已存在：{}",
            install_path.display()
        ));
    }
    tokio::fs::rename(staging_path, install_path)
        .await
        .map_err(|error| format!("完成 RoundMCDev 原子安装失败：{error}"))?;
    find_named_file(&install_path.join("umu"), "umu-run")
        .ok_or_else(|| "RoundMCDev 安装完成后没有找到 umu/umu-run".to_string())
}

fn sanitize_instance_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "proton-gdk".to_string()
    } else {
        sanitized.to_string()
    }
}

fn proton_gdk_install_directory_name(source: ProtonGdkSource, release_name: &str) -> String {
    format!(
        "{}-{}",
        source.config_value(),
        sanitize_instance_name(release_name)
    )
}

pub(crate) fn resolve_runner() -> Result<Runner, String> {
    if let Ok(config) = crate::config::config::read_config()
        && !config.launcher.proton_gdk_runner.trim().is_empty()
    {
        return runner_from_explicit_path(PathBuf::from(config.launcher.proton_gdk_runner));
    }

    if let Some(runner) = find_managed_runner() {
        return Ok(runner);
    }

    // Do not fall back to stock Steam/system Proton. Bedrock UWP/GDK requires
    // the patched Proton-GDK runner managed by BMCBL.
    if let Some(executable) = find_in_path("wine") {
        return Ok(Runner {
            executable,
            kind: RunnerKind::Wine,
            steam_root: None,
        });
    }

    Err("未找到 Proton-GDK。请安装兼容的 GDK-Proton，或在 BMCBL 设置中选择 runner".to_string())
}

pub(crate) fn installed_proton_gdk_runners() -> Vec<InstalledProtonGdkRunner> {
    let Ok(entries) = std::fs::read_dir(file_ops::runners_dir()) else {
        return Vec::new();
    };
    let mut runners = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let root = entry.path();
            find_named_file(&root.join("umu"), "umu-run")
                .or_else(|| find_proton_file(&root))
                .filter(|candidate| is_executable_file(candidate))
        })
        .map(installed_proton_gdk_runner)
        .collect::<Vec<_>>();
    runners.sort_by(|left, right| left.executable.cmp(&right.executable));
    runners
}

pub(crate) fn remove_managed_proton_gdk_runner(
    executable: &Path,
) -> Result<Option<PathBuf>, String> {
    let runners_root = file_ops::runners_dir();
    remove_managed_proton_gdk_runner_from_root(executable, &runners_root)
}

fn remove_managed_proton_gdk_runner_from_root(
    executable: &Path,
    runners_root: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(managed_root) = runner_install_root_for_path(executable, &runners_root) else {
        return Ok(None);
    };
    if !managed_root.is_dir() {
        return Err(format!(
            "Proton-GDK 安装目录不存在或不是目录：{}",
            managed_root.display()
        ));
    }

    std::fs::remove_dir_all(&managed_root)
        .map_err(|error| format!("删除 Proton-GDK 安装目录失败：{error}"))?;

    let root_name = managed_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "无法确定 Proton-GDK 安装目录名称".to_string())?;
    let incomplete_prefix = format!("{root_name}.incomplete-");
    let staging_prefix = format!(".{root_name}.installing-");
    let entries = std::fs::read_dir(&runners_root)
        .map_err(|error| format!("读取 Proton-GDK 安装目录失败：{error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取 Proton-GDK 残留目录失败：{error}"))?;
        let path = entry.path();
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(&incomplete_prefix)
            && !name.to_string_lossy().starts_with(&staging_prefix)
        {
            continue;
        }
        let file_type = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("读取 Proton-GDK 残留目录类型失败：{error}"))?
            .file_type();
        if file_type.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "删除 Proton-GDK 残留安装目录 {} 失败：{error}",
                    path.display()
                )
            })?;
        } else {
            std::fs::remove_file(&path).map_err(|error| {
                format!("删除 Proton-GDK 残留文件 {} 失败：{error}", path.display())
            })?;
        }
    }

    if managed_root.exists() {
        return Err(format!(
            "Proton-GDK 删除后目录仍然存在：{}",
            managed_root.display()
        ));
    }
    Ok(Some(managed_root))
}

fn runner_install_root_for_path(executable: &Path, runners_root: &Path) -> Option<PathBuf> {
    let relative = executable.strip_prefix(runners_root).ok()?;
    let root_name = relative.components().next().and_then(|component| {
        if let std::path::Component::Normal(name) = component {
            Some(name)
        } else {
            None
        }
    })?;
    Some(runners_root.join(root_name))
}

pub(crate) fn installed_proton_gdk_runner(executable: PathBuf) -> InstalledProtonGdkRunner {
    let metadata_root = proton_gdk_runner_root(&executable)
        .into_iter()
        .chain(find_bundle_root_for_path(&executable))
        .find_map(|root| read_proton_gdk_metadata(&root).map(|metadata| (root, metadata)));
    if let Some((_, metadata)) = metadata_root
        .filter(|(_, metadata)| metadata.schema_version == PROTON_GDK_METADATA_SCHEMA_VERSION)
        .filter(|(_, metadata)| ProtonGdkSource::from_stored_value(&metadata.source).is_some())
    {
        let source = ProtonGdkSource::from_stored_value(&metadata.source);
        if let Some(source) = source {
            return InstalledProtonGdkRunner {
                executable,
                display_name: metadata.release_name,
                source: Some(source),
                release_tag: Some(metadata.release_tag),
                asset_names: metadata.asset_names,
                identity: ProtonGdkIdentity::Metadata,
            };
        }
    }

    let runner_root =
        proton_gdk_runner_root(&executable).or_else(|| find_bundle_root_for_path(&executable));
    if let Some(metadata) = runner_root
        .as_deref()
        .and_then(read_proton_gdk_metadata)
        .filter(|metadata| metadata.schema_version == PROTON_GDK_METADATA_SCHEMA_VERSION)
        && let Some(source) = ProtonGdkSource::from_stored_value(&metadata.source)
    {
        return InstalledProtonGdkRunner {
            executable,
            display_name: metadata.release_name,
            source: Some(source),
            release_tag: Some(metadata.release_tag),
            asset_names: metadata.asset_names,
            identity: ProtonGdkIdentity::Metadata,
        };
    }

    let directory_name = runner_root
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("Proton-GDK")
        .to_string();
    let source = infer_proton_gdk_source_from_directory(&directory_name);
    InstalledProtonGdkRunner {
        executable,
        display_name: directory_name,
        source,
        release_tag: None,
        asset_names: Vec::new(),
        identity: if source.is_some() {
            ProtonGdkIdentity::DirectoryName
        } else {
            ProtonGdkIdentity::Unknown
        },
    }
}

fn proton_gdk_runner_root(executable: &Path) -> Option<PathBuf> {
    let parent = executable.parent()?;
    if parent.file_name().is_some_and(|name| name == "bin") {
        parent.parent().map(Path::to_path_buf)
    } else {
        Some(parent.to_path_buf())
    }
}

fn is_gdk_proton_root(path: &Path) -> bool {
    path.join("files/bin/wine").is_file() && path.join("compatibilitytool.vdf").is_file()
}

fn find_gdk_proton_root(search_root: &Path) -> Option<PathBuf> {
    if is_gdk_proton_root(search_root) {
        return Some(search_root.to_path_buf());
    }
    let mut child_directories = std::fs::read_dir(search_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    child_directories.sort();
    child_directories
        .into_iter()
        .find_map(|directory| find_gdk_proton_root(&directory))
}

fn find_proton_file(search_root: &Path) -> Option<PathBuf> {
    if !search_root.is_dir() {
        return None;
    }
    let direct_candidates = [
        search_root.join("proton"),
        search_root.join("bin").join("proton"),
    ];
    if let Some(proton) = direct_candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
    {
        return Some(proton);
    }

    let mut child_directories = std::fs::read_dir(search_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    child_directories.sort();
    child_directories
        .into_iter()
        .find_map(|directory| find_proton_file(&directory))
}

fn read_proton_gdk_metadata(runner_root: &Path) -> Option<ProtonGdkRunnerMetadata> {
    let contents = std::fs::read(runner_root.join(PROTON_GDK_METADATA_FILE)).ok()?;
    serde_json::from_slice(&contents).ok()
}

fn infer_proton_gdk_source_from_directory(directory_name: &str) -> Option<ProtonGdkSource> {
    let directory_name = directory_name.to_ascii_lowercase();
    if directory_name.starts_with("roundmcdev-") {
        Some(ProtonGdkSource::RoundMcDev)
    } else if directory_name.starts_with("weather-os-") {
        Some(ProtonGdkSource::WeatherOs)
    } else if directory_name.starts_with("lukaspah-")
        || directory_name.contains("gdk-proton-custom")
    {
        Some(ProtonGdkSource::LukasPah)
    } else {
        None
    }
}

pub(crate) fn start_linux_runtime_install(plan: LinuxInstallPlan) -> String {
    let command_preview = plan.command_preview();
    let task_id = create_task_with_details(
        None,
        "安装 Linux 兼容环境",
        Some(format!("{} · {}", plan.distribution_name, command_preview)),
        "awaiting_linux_authorization",
        None,
        false,
    );
    append_task_log(&task_id, format!("将执行：{command_preview}"));
    append_task_log(
        &task_id,
        "BMCBL 主进程保持普通用户权限，仅包管理器通过 pkexec 请求授权",
    );
    set_task_message(&task_id, Some("等待系统授权窗口确认".to_string()));

    let task_id_for_task = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        let outcome = run_install_command(&plan, &task_id_for_task).await;
        match outcome {
            Ok(()) => finish_task(
                &task_id_for_task,
                "completed",
                Some("兼容环境安装完成".to_string()),
            ),
            Err(error) => finish_task(&task_id_for_task, "error", Some(error)),
        }
    }) {
        finish_task(
            &task_id,
            "error",
            Some(format!("无法调度 Linux 兼容环境安装任务：{error}")),
        );
    }
    task_id
}

async fn run_install_command(plan: &LinuxInstallPlan, task_id: &str) -> Result<(), String> {
    if !is_executable_file(&plan.authorization_program) {
        return Err(format!(
            "授权工具不可用：{}",
            plan.authorization_program.display()
        ));
    }
    if !is_executable_file(&plan.package_manager) {
        return Err(format!(
            "包管理器不可用：{}",
            plan.package_manager.display()
        ));
    }

    info!(
        task_id,
        distribution = %plan.distribution_name,
        package_manager = %plan.package_manager.display(),
        packages = ?plan.packages,
        "requesting authorization for Linux compatibility runtime installation"
    );
    let mut command = tokio::process::Command::new(&plan.authorization_program);
    command
        .arg(&plan.package_manager)
        .args(plan.arguments.iter().map(AsRef::<str>::as_ref))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动授权安装程序：{error}"))?;
    update_progress(task_id, 0, None, Some("installing_linux_packages"));
    set_task_message(task_id, Some("系统包管理器正在安装依赖".to_string()));
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| spawn_install_output_pump(task_id.to_string(), stdout, false));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| spawn_install_output_pump(task_id.to_string(), stderr, true));
    let status = child
        .wait()
        .await
        .map_err(|error| format!("等待安装程序结束失败：{error}"))?;

    if let Some(task) = stdout_task
        && let Err(error) = task.await
    {
        warn!(task_id, %error, "failed to join package manager stdout reader");
    }
    if let Some(task) = stderr_task
        && let Err(error) = task.await
    {
        warn!(task_id, %error, "failed to join package manager stderr reader");
    }

    match status.code() {
        Some(0) => Ok(()),
        Some(126) => Err("用户取消了管理员授权".to_string()),
        Some(127) => Err("授权失败，或当前桌面会话没有可用的授权代理".to_string()),
        Some(code) => Err(format!("包管理器安装失败，退出代码 {code}")),
        None => Err("安装程序被信号终止".to_string()),
    }
}

fn spawn_install_output_pump<R>(
    task_id: String,
    reader: R,
    is_error: bool,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let stream = if is_error { "stderr" } else { "stdout" };
                    append_task_log(&task_id, format!("{stream}: {line}"));
                    update_progress(&task_id, 0, None, None);
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(task_id, %error, "failed to read package manager output");
                    append_task_log(&task_id, format!("读取安装输出失败：{error}"));
                    break;
                }
            }
        }
    })
}

fn build_proton_host_dependencies_plan(
    os_release: &OsRelease,
    distribution_name: Arc<str>,
) -> Option<LinuxInstallPlan> {
    let authorization_program = find_program(&["pkexec"])?;
    let family = format!("{} {}", os_release.id, os_release.id_like).to_ascii_lowercase();
    let (package_manager, arguments, packages): (PathBuf, Arc<[Arc<str>]>, Arc<[Arc<str>]>) =
        if contains_family(&family, &["fedora", "rhel", "centos", "rocky", "almalinux"]) {
            (
                find_program(&["dnf5", "dnf"])?,
                Arc::from([
                    Arc::<str>::from("-y"),
                    Arc::<str>::from("install"),
                    Arc::<str>::from("glibc.i686"),
                ]),
                Arc::from([Arc::<str>::from("glibc.i686")]),
            )
        } else if contains_family(&family, &["debian", "ubuntu", "mint", "pop"]) {
            (
                find_program(&["apt-get"])?,
                Arc::from([
                    Arc::<str>::from("-y"),
                    Arc::<str>::from("install"),
                    Arc::<str>::from("libc6-i386"),
                ]),
                Arc::from([Arc::<str>::from("libc6-i386")]),
            )
        } else if contains_family(&family, &["arch", "manjaro", "endeavouros"]) {
            (
                find_program(&["pacman"])?,
                Arc::from([
                    Arc::<str>::from("-S"),
                    Arc::<str>::from("--needed"),
                    Arc::<str>::from("--noconfirm"),
                    Arc::<str>::from("lib32-glibc"),
                ]),
                Arc::from([Arc::<str>::from("lib32-glibc")]),
            )
        } else if contains_family(&family, &["suse", "opensuse"]) {
            (
                find_program(&["zypper"])?,
                Arc::from([
                    Arc::<str>::from("--non-interactive"),
                    Arc::<str>::from("install"),
                    Arc::<str>::from("glibc-32bit"),
                ]),
                Arc::from([Arc::<str>::from("glibc-32bit")]),
            )
        } else {
            return None;
        };

    Some(LinuxInstallPlan {
        distribution_name,
        authorization_program,
        package_manager,
        arguments,
        packages,
    })
}

fn contains_family(family: &str, names: &[&str]) -> bool {
    family
        .split_ascii_whitespace()
        .any(|value| names.contains(&value))
}

fn detect_os_release() -> OsRelease {
    ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map(|contents| parse_os_release(&contents))
        .unwrap_or_default()
}

fn parse_os_release(contents: &str) -> OsRelease {
    let mut release = OsRelease::default();
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or_else(|| value.trim());
        match key.trim() {
            "ID" => release.id = value.to_string(),
            "ID_LIKE" => release.id_like = value.to_string(),
            "PRETTY_NAME" => release.pretty_name = value.to_string(),
            _ => {}
        }
    }
    release
}

fn runner_from_explicit_path(executable: PathBuf) -> Result<Runner, String> {
    if !is_executable_file(&executable) {
        return Err(format!(
            "配置的 Proton runner 文件不存在或不可执行：{}",
            executable.display()
        ));
    }
    let file_name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = if file_name.contains("umu") {
        RunnerKind::Umu
    } else if file_name.contains("proton") {
        RunnerKind::Proton
    } else if file_name.contains("wine") {
        RunnerKind::Wine
    } else {
        return Err("配置的 Proton runner 必须指向 proton、umu-run 或 wine 可执行文件".to_string());
    };
    Ok(Runner {
        executable,
        kind,
        steam_root: steam_roots().into_iter().next(),
    })
}

fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(configured) = env::var_os("STEAM_COMPAT_CLIENT_INSTALL_PATH") {
        roots.push(PathBuf::from(configured));
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.extend([
            home.join(".steam/root"),
            home.join(".local/share/Steam"),
            home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        ]);
    }
    roots.retain(|path| path.is_dir());
    roots.dedup();
    roots
}

fn find_steam_proton(steam_root: &Path) -> Option<PathBuf> {
    let search_directories = [
        steam_root.join("compatibilitytools.d"),
        steam_root.join("steamapps/common"),
    ];
    for search_directory in search_directories {
        let Ok(entries) = std::fs::read_dir(search_directory) else {
            continue;
        };
        let mut candidates = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("proton"))
            .filter(|path| is_executable_file(path))
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(candidate) = candidates.pop() {
            return Some(candidate);
        }
    }
    None
}

fn find_managed_runner() -> Option<Runner> {
    let root = file_ops::runners_dir();
    let entries = std::fs::read_dir(&root).ok()?;
    let mut umu_candidates = Vec::new();
    let mut proton_candidates = Vec::new();
    let mut wine_candidates = Vec::new();
    for runner_root in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if let Some(umu) = find_named_file(&runner_root.join("umu"), "umu-run")
            .filter(|candidate| is_executable_file(candidate))
        {
            umu_candidates.push(umu);
        }
        let proton = if runner_root.join("proton").is_file() {
            runner_root.join("proton")
        } else {
            runner_root.join("bin").join("proton")
        };
        if is_executable_file(&proton) {
            proton_candidates.push(proton);
        }
        let wine = runner_root.join("wine");
        if is_executable_file(&wine) {
            wine_candidates.push(wine);
        }
    }
    umu_candidates.sort();
    proton_candidates.sort();
    wine_candidates.sort();

    umu_candidates
        .pop()
        .map(|executable| Runner {
            executable,
            kind: RunnerKind::Umu,
            steam_root: steam_roots().into_iter().next(),
        })
        .or_else(|| {
            proton_candidates
                .pop()
                .map(|executable| Runner {
                    executable,
                    kind: RunnerKind::Proton,
                    steam_root: steam_roots().into_iter().next(),
                })
                .or_else(|| {
                    wine_candidates.pop().map(|executable| Runner {
                        executable,
                        kind: RunnerKind::Wine,
                        steam_root: None,
                    })
                })
        })
}

fn find_program(names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| find_in_path(name))
}

fn find_in_path(executable_name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(executable_name))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::{
        PROTON_GDK_METADATA_FILE, PROTON_GDK_METADATA_SCHEMA_VERSION, ProtonGdkIdentity,
        ProtonGdkRunnerMetadata, ProtonGdkSource, ROUNDMCDEV_ASSETS, ROUNDMCDEV_RELEASE_NAME,
        ROUNDMCDEV_RELEASE_TAG, find_gdk_proton_root, find_proton_file,
        infer_proton_gdk_source_from_directory, installed_proton_gdk_runner, parse_os_release,
        promote_staged_proton_gdk, proton_gdk_install_directory_name, proton_gdk_runner_root,
        remove_managed_proton_gdk_runner_from_root, roundmcdev_bundle_is_complete,
        runner_install_root_for_path,
    };
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_os_release_identity() {
        let release = parse_os_release(
            "ID=fedora\nID_LIKE=\"rhel centos\"\nPRETTY_NAME=\"Fedora Linux 44\"\n",
        );

        assert_eq!(release.id, "fedora");
        assert_eq!(release.id_like, "rhel centos");
        assert_eq!(release.pretty_name, "Fedora Linux 44");
    }

    #[test]
    fn proton_gdk_source_defaults_to_round_mc_dev() {
        assert_eq!(
            ProtonGdkSource::from_config(""),
            ProtonGdkSource::RoundMcDev
        );
        assert_eq!(
            ProtonGdkSource::from_config("unknown"),
            ProtonGdkSource::RoundMcDev
        );
    }

    #[test]
    fn proton_gdk_source_preserves_explicit_alternative_choices() {
        assert_eq!(
            ProtonGdkSource::from_config("weather-os"),
            ProtonGdkSource::WeatherOs
        );
        assert_eq!(
            ProtonGdkSource::from_config("lukaspah"),
            ProtonGdkSource::LukasPah
        );
    }

    #[test]
    fn proton_gdk_round_mc_dev_source_uses_fixed_login_capable_bundle() {
        let source = ProtonGdkSource::RoundMcDev;

        assert_eq!(source.config_value(), "roundmcdev");
        assert_eq!(source.repository(), "RoundMCDev/ProtonGDK-Release");
        assert_eq!(ROUNDMCDEV_RELEASE_TAG, "Release10-32");
        assert_eq!(ROUNDMCDEV_RELEASE_NAME, "GDK-Proton10-32-Kits.01");
        assert_eq!(
            ROUNDMCDEV_ASSETS.map(|asset| asset.name),
            [
                "GameRunningFixKit.tar.gz",
                "GDK-Proton-xuser.tar.gz",
                "Proton-Launch-umu.tar.gz",
                "GamePatch.zip",
            ]
        );
        assert!(
            ROUNDMCDEV_ASSETS
                .iter()
                .all(|asset| asset.url.contains("/releases/download/Release10-32/"))
        );
    }

    #[test]
    fn proton_gdk_install_directory_identifies_source_and_release() {
        assert_eq!(
            proton_gdk_install_directory_name(
                ProtonGdkSource::RoundMcDev,
                "GDK-Proton10-32 Fix.01"
            ),
            "roundmcdev-GDK-Proton10-32-Fix.01"
        );
        assert_eq!(
            proton_gdk_install_directory_name(ProtonGdkSource::WeatherOs, "Release 10/32"),
            "weather-os-Release-10-32"
        );
    }

    #[test]
    fn legacy_directory_inference_requires_unambiguous_source_name() {
        assert_eq!(
            infer_proton_gdk_source_from_directory("lukaspah-GDK-Proton-Custom"),
            Some(ProtonGdkSource::LukasPah)
        );
        assert_eq!(
            infer_proton_gdk_source_from_directory("weather-os-GDK-Proton10-32"),
            Some(ProtonGdkSource::WeatherOs)
        );
        assert_eq!(
            infer_proton_gdk_source_from_directory("GDK-Proton10-32"),
            None
        );
    }

    #[test]
    fn nested_release_directory_proton_is_discovered() {
        let install_root = unique_test_directory("proton-gdk-nested-release");
        let release_root = install_root.join("GDK-Proton10-32-Fix.01");
        std::fs::create_dir_all(&release_root).expect("nested release directory should be created");
        std::fs::write(release_root.join("proton"), b"#!/bin/sh\n")
            .expect("nested proton file should be written");

        let proton = find_proton_file(&install_root).expect("nested proton should be discovered");

        assert_eq!(proton, release_root.join("proton"));
        assert_eq!(proton_gdk_runner_root(&proton), Some(release_root));
        std::fs::remove_dir_all(&install_root).expect("test install directory should be removed");
    }

    #[test]
    fn managed_runner_root_is_resolved_from_umu_entry() {
        let runners_root = PathBuf::from("/tmp/bmcbl/runners");
        let executable = runners_root.join("roundmcdev-kit/umu/umu-run");

        assert_eq!(
            runner_install_root_for_path(&executable, &runners_root),
            Some(runners_root.join("roundmcdev-kit"))
        );
    }

    #[test]
    fn managed_runner_removal_deletes_bundle_and_install_residue() {
        let runners_root = unique_test_directory("proton-gdk-removal");
        let managed_root = runners_root.join("roundmcdev-GDK-Proton10-32-Kits.01");
        std::fs::create_dir_all(managed_root.join("umu"))
            .expect("managed runner directory should be created");
        std::fs::write(managed_root.join("umu/umu-run"), b"runner")
            .expect("runner entry should be written");
        std::fs::create_dir_all(
            runners_root.join("roundmcdev-GDK-Proton10-32-Kits.01.incomplete-123"),
        )
        .expect("incomplete directory should be created");
        std::fs::create_dir_all(
            runners_root.join(".roundmcdev-GDK-Proton10-32-Kits.01.installing-123"),
        )
        .expect("staging directory should be created");

        let removed = remove_managed_proton_gdk_runner_from_root(
            &managed_root.join("umu/umu-run"),
            &runners_root,
        )
        .expect("managed runner removal should succeed");

        assert_eq!(removed, Some(managed_root.clone()));
        assert!(!managed_root.exists());
        assert!(
            !runners_root
                .join("roundmcdev-GDK-Proton10-32-Kits.01.incomplete-123")
                .exists()
        );
        assert!(
            !runners_root
                .join(".roundmcdev-GDK-Proton10-32-Kits.01.installing-123")
                .exists()
        );
        std::fs::remove_dir_all(&runners_root).expect("test runner directory should be removed");
    }

    #[test]
    fn roundmcdev_bundle_recognizes_bedrockboot_layout() {
        let bundle_root = unique_test_directory("roundmcdev-bundle");
        let proton_root = bundle_root.join("proton/GDK-Proton-xuser");
        std::fs::create_dir_all(proton_root.join("files/bin"))
            .expect("proton directory should be created");
        std::fs::write(proton_root.join("files/bin/wine"), b"#!/bin/sh\n")
            .expect("GDK wine executable should be written");
        std::fs::write(proton_root.join("proton"), b"#!/bin/sh\n")
            .expect("GDK proton wrapper should be written");
        std::fs::write(proton_root.join("compatibilitytool.vdf"), b"Manifest\n")
            .expect("GDK compatibility manifest should be written");
        std::fs::create_dir_all(bundle_root.join("umu")).expect("umu directory should be created");
        std::fs::write(bundle_root.join("umu/umu-run"), b"#!/bin/sh\n")
            .expect("umu executable should be written");
        std::fs::create_dir_all(bundle_root.join("gameFix"))
            .expect("gameFix directory should be created");
        std::fs::create_dir_all(bundle_root.join("GamePatch/gdk"))
            .expect("GamePatch directory should be created");
        std::fs::write(
            bundle_root.join("GamePatch/gdk/mcpatcher_core.dll"),
            b"patch",
        )
        .expect("GamePatch file should be written");

        assert_eq!(
            find_gdk_proton_root(&bundle_root.join("proton")),
            Some(proton_root)
        );
        assert!(roundmcdev_bundle_is_complete(&bundle_root));
        std::fs::remove_dir_all(&bundle_root).expect("bundle test directory should be removed");
    }

    #[tokio::test]
    async fn staged_nested_release_is_promoted_to_single_install_directory() {
        let test_root = unique_test_directory("proton-gdk-promote");
        let staging_path = test_root.join("staging");
        let nested_release = staging_path.join("GDK-Proton10-32-Fix.01");
        let install_path = test_root.join("roundmcdev-GDK-Proton10-32-Fix.01");
        std::fs::create_dir_all(&nested_release)
            .expect("nested staged release directory should be created");
        std::fs::write(nested_release.join("proton"), b"#!/bin/sh\n")
            .expect("staged proton file should be written");

        let proton = promote_staged_proton_gdk(&staging_path, &install_path)
            .await
            .expect("staged release should be promoted");

        assert_eq!(proton, install_path.join("proton"));
        assert!(proton.is_file());
        assert!(!install_path.join("GDK-Proton10-32-Fix.01").exists());
        std::fs::remove_dir_all(&test_root).expect("test promotion directory should be removed");
    }

    #[test]
    fn installed_runner_uses_bmcbl_metadata_for_exact_identity() {
        let runner_root = unique_test_directory("proton-gdk-metadata");
        std::fs::create_dir_all(&runner_root).expect("test runner directory should be created");
        let metadata = ProtonGdkRunnerMetadata {
            schema_version: PROTON_GDK_METADATA_SCHEMA_VERSION,
            source: "roundmcdev".to_string(),
            repository: "RoundMCDev/ProtonGDK-Release".to_string(),
            release_tag: "Release10-32".to_string(),
            release_name: "GDK-Proton10-32-Fix.01".to_string(),
            asset_name: "GDK-Proton10-32-Fix.01.tar.gz".to_string(),
            asset_names: vec!["GDK-Proton10-32-Fix.01.tar.gz".to_string()],
        };
        let contents =
            serde_json::to_vec(&metadata).expect("test metadata should serialize successfully");
        std::fs::write(runner_root.join(PROTON_GDK_METADATA_FILE), contents)
            .expect("test metadata should be written");

        let runner = installed_proton_gdk_runner(runner_root.join("proton"));

        assert_eq!(runner.source, Some(ProtonGdkSource::RoundMcDev));
        assert_eq!(runner.display_name, "GDK-Proton10-32-Fix.01");
        assert_eq!(runner.release_tag.as_deref(), Some("Release10-32"));
        assert_eq!(runner.identity, ProtonGdkIdentity::Metadata);
        std::fs::remove_dir_all(&runner_root).expect("test runner directory should be removed");
    }

    #[test]
    fn unknown_metadata_source_is_not_reported_as_round_mc_dev() {
        let runner_root = unique_test_directory("proton-gdk-unknown-source");
        std::fs::create_dir_all(&runner_root).expect("test runner directory should be created");
        let metadata = ProtonGdkRunnerMetadata {
            schema_version: PROTON_GDK_METADATA_SCHEMA_VERSION,
            source: "unrecognized".to_string(),
            repository: "example/unknown".to_string(),
            release_tag: "test".to_string(),
            release_name: "Unknown Proton".to_string(),
            asset_name: "unknown.tar.gz".to_string(),
            asset_names: vec!["unknown.tar.gz".to_string()],
        };
        let contents =
            serde_json::to_vec(&metadata).expect("test metadata should serialize successfully");
        std::fs::write(runner_root.join(PROTON_GDK_METADATA_FILE), contents)
            .expect("test metadata should be written");

        let runner = installed_proton_gdk_runner(runner_root.join("proton"));

        assert_eq!(runner.source, None);
        assert_eq!(runner.identity, ProtonGdkIdentity::Unknown);
        std::fs::remove_dir_all(&runner_root).expect("test runner directory should be removed");
    }

    fn unique_test_directory(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("bmcbl-{label}-{}-{timestamp}", std::process::id()))
    }
}
