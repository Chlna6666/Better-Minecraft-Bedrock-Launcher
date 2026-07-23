use crate::downloads::manager::{DownloadOptions, DownloaderManager};
use crate::result::CoreResult;
use crate::tasks::task_manager::{
    append_task_log, create_task_with_details, finish_task, register_task_stage_labels,
    set_task_message, update_progress,
};
use crate::utils::file_ops;
use std::env;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tracing::{info, warn};

const INSTALL_STAGE_LABELS: [(&str, &str); 2] = [
    ("awaiting_linux_authorization", "等待管理员授权"),
    ("installing_linux_packages", "安装兼容环境依赖"),
];

pub(crate) const PROTON_GDK_RELEASE_SOURCES: [&str; 2] =
    ["Weather-OS/GDK-Proton", "LukasPAH/GDK-Proton-Custom"];

const PROTON_GDK_INSTALL_STAGE_LABELS: [(&str, &str); 3] = [
    ("resolving_proton_gdk", "获取 Proton-GDK 版本"),
    ("downloading_proton_gdk", "下载 Proton-GDK"),
    ("extracting_proton_gdk", "安装 Proton-GDK"),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProtonGdkSource {
    WeatherOs,
    LukasPah,
}

impl ProtonGdkSource {
    pub(crate) fn from_config(value: &str) -> Self {
        if value.eq_ignore_ascii_case("lukaspah") {
            Self::LukasPah
        } else {
            Self::WeatherOs
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::WeatherOs => "weather-os",
            Self::LukasPah => "lukaspah",
        }
    }

    pub(crate) fn repository(self) -> &'static str {
        match self {
            Self::WeatherOs => "Weather-OS/GDK-Proton",
            Self::LukasPah => "LukasPAH/GDK-Proton-Custom",
        }
    }

    fn latest_release_api(self) -> &'static str {
        match self {
            Self::WeatherOs => "https://api.github.com/repos/Weather-OS/GDK-Proton/releases/latest",
            Self::LukasPah => {
                "https://api.github.com/repos/LukasPAH/GDK-Proton-Custom/releases/latest"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunnerKind {
    Proton,
    Wine,
}

impl RunnerKind {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Proton => "Proton",
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
                            "请前往 Proton-GDK 设置页安装并选择 LukasPAH Custom 版本，不要为此错误授权安装系统软件包。",
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
    if runner.kind != RunnerKind::Proton {
        return Ok(());
    }
    let Some(proton_root) = runner.executable.parent() else {
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
    if runner.kind == RunnerKind::Proton {
        Ok(runner)
    } else {
        Err("已检测到 Wine，但 Minecraft UWP/GDK 版本需要 Proton".to_string())
    }
}

pub(crate) fn start_proton_gdk_install_latest(source: ProtonGdkSource) -> String {
    register_task_stage_labels(PROTON_GDK_INSTALL_STAGE_LABELS);
    let task_id = create_task_with_details(
        None,
        "安装 Proton-GDK",
        Some(format!("{} · latest", source.repository())),
        "resolving_proton_gdk",
        None,
        false,
    );
    append_task_log(
        &task_id,
        format!("正在获取 {} 最新版本", source.repository()),
    );
    set_task_message(&task_id, Some("正在获取可安装版本".to_string()));

    let worker_task_id = task_id.clone();
    tokio::spawn(async move {
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
    });
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
    let version_name = sanitize_instance_name(
        release
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&release.tag_name),
    );
    let download_dir = file_ops::downloads_dir().join("proton-gdk");
    tokio::fs::create_dir_all(&download_dir)
        .await
        .map_err(|error| format!("创建 Proton-GDK 下载目录失败：{error}"))?;
    let archive_path = download_dir.join(&asset.name);
    download_proton_gdk_asset(&client, asset, &archive_path, task_id).await?;

    let install_path = file_ops::runners_dir().join(version_name);
    if install_path.exists() {
        return Err(format!(
            "该 Proton-GDK 版本已经安装：{}",
            install_path.display()
        ));
    }
    tokio::fs::create_dir_all(&install_path)
        .await
        .map_err(|error| format!("创建 Proton-GDK 安装目录失败：{error}"))?;
    update_progress(task_id, 0, None, Some("extracting_proton_gdk"));
    set_task_message(task_id, Some("正在解压 Proton-GDK".to_string()));
    append_task_log(task_id, format!("解压到 {}", install_path.display()));
    let output = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&install_path)
        .arg("--strip-components=1")
        .output()
        .await
        .map_err(|error| format!("无法启动 tar：{error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("解压 Proton-GDK 失败：{}", error.trim()));
    }
    let proton = install_path.join("proton");
    if !proton.is_file() {
        return Err(format!(
            "安装包中没有 proton 可执行文件：{}",
            proton.display()
        ));
    }
    let mut permissions = tokio::fs::metadata(&proton)
        .await
        .map_err(|error| format!("读取 Proton-GDK 权限失败：{error}"))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    tokio::fs::set_permissions(&proton, permissions)
        .await
        .map_err(|error| format!("设置 Proton-GDK 可执行权限失败：{error}"))?;
    let selected_runner = proton.to_string_lossy().into_owned();
    crate::config::config::update_config(|config| {
        config.launcher.proton_gdk_runner = selected_runner.clone();
        config.launcher.proton_gdk_source = source.config_value().to_string();
    })
    .map_err(|error| format!("保存 Proton-GDK 默认版本失败：{error}"))?;
    Ok(install_path)
}

async fn download_proton_gdk_asset(
    client: &reqwest::Client,
    asset: &GithubReleaseAsset,
    archive_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    update_progress(task_id, 0, Some(asset.size), Some("downloading_proton_gdk"));
    set_task_message(task_id, Some(format!("正在下载 {}", asset.name)));
    append_task_log(task_id, format!("下载：{}", asset.browser_download_url));

    let manager = DownloaderManager::with_client(client.clone());
    let options = DownloadOptions::default();
    let result = manager
        .download_with_options(
            task_id,
            asset.browser_download_url.clone(),
            archive_path.to_path_buf(),
            &options,
        )
        .await
        .map_err(|error| format!("下载 Proton-GDK 失败：{error:?}"))?;

    match result {
        CoreResult::Success(_path) => Ok(()),
        CoreResult::Cancelled => Err("下载已取消".to_string()),
        CoreResult::Error(error) => Err(format!("下载 Proton-GDK 失败：{error:?}")),
    }
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

pub(crate) fn resolve_runner() -> Result<Runner, String> {
    if let Some(executable) = env::var_os("BMCBL_PROTON_GDK_RUNNER")
        .or_else(|| env::var_os("BMCBL_PROTON_RUNNER"))
        .map(PathBuf::from)
    {
        return runner_from_explicit_path(executable);
    }

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

    Err("未找到 Proton-GDK。请安装兼容的 GDK-Proton，或用 BMCBL_PROTON_GDK_RUNNER 指定 proton 可执行文件".to_string())
}

pub(crate) fn installed_proton_gdk_runners() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(file_ops::runners_dir()) else {
        return Vec::new();
    };
    let mut runners = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let root = entry.path();
            [root.join("proton"), root.join("bin").join("proton")]
                .into_iter()
                .find(|candidate| is_executable_file(candidate))
        })
        .collect::<Vec<_>>();
    runners.sort();
    runners
}

pub(crate) fn start_linux_runtime_install(plan: LinuxInstallPlan) -> String {
    register_task_stage_labels(INSTALL_STAGE_LABELS);
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
    tokio::spawn(async move {
        let outcome = run_install_command(&plan, &task_id_for_task).await;
        match outcome {
            Ok(()) => finish_task(
                &task_id_for_task,
                "completed",
                Some("兼容环境安装完成".to_string()),
            ),
            Err(error) => finish_task(&task_id_for_task, "error", Some(error)),
        }
    });
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
            "BMCBL_PROTON_RUNNER 指向的文件不存在或不可执行：{}",
            executable.display()
        ));
    }
    let file_name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = if file_name.contains("proton") {
        RunnerKind::Proton
    } else if file_name.contains("wine") {
        RunnerKind::Wine
    } else {
        return Err("BMCBL_PROTON_RUNNER 必须指向 proton 或 wine 可执行文件".to_string());
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
    let mut proton_candidates = Vec::new();
    let mut wine_candidates = Vec::new();
    for runner_root in entries.filter_map(Result::ok).map(|entry| entry.path()) {
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
    proton_candidates.sort();
    wine_candidates.sort();

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
    use super::parse_os_release;

    #[test]
    fn parses_os_release_identity() {
        let release = parse_os_release(
            "ID=fedora\nID_LIKE=\"rhel centos\"\nPRETTY_NAME=\"Fedora Linux 44\"\n",
        );

        assert_eq!(release.id, "fedora");
        assert_eq!(release.id_like, "rhel centos");
        assert_eq!(release.pretty_name, "Fedora Linux 44");
    }
}
