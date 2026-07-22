use crate::tasks::task_manager::{
    append_task_log, create_task_with_details, finish_task, register_task_stage_labels,
};
use crate::utils::file_ops;
use std::env;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tracing::{info, warn};

const INSTALL_STAGE_LABELS: [(&str, &str); 1] = [("installing_linux_runtime", "安装兼容环境")];

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
    match resolve_runner() {
        Ok(runner) => LinuxRuntimeCheck {
            distribution_name: detect_os_release().pretty_name.into(),
            runner: Some(runner),
            missing_reason: None,
            install_plan: None,
            manual_install_hint: Arc::from(""),
        },
        Err(reason) => {
            let os_release = detect_os_release();
            let distribution_name: Arc<str> = if os_release.pretty_name.is_empty() {
                Arc::from("未知 Linux 发行版")
            } else {
                Arc::from(os_release.pretty_name.as_str())
            };
            let install_plan = build_install_plan(&os_release, distribution_name.clone());
            let manual_install_hint = if install_plan.is_some() {
                Arc::from("也可以自行安装 Proton 或 Wine，然后重新检测。")
            } else {
                Arc::from(
                    "当前发行版没有可用的自动安装方案，请使用系统包管理器安装 Proton 或 Wine。",
                )
            };
            LinuxRuntimeCheck {
                runner: None,
                missing_reason: Some(Arc::from(reason)),
                distribution_name,
                install_plan,
                manual_install_hint,
            }
        }
    }
}

pub(crate) fn resolve_runner() -> Result<Runner, String> {
    if let Some(executable) = env::var_os("BMCBL_PROTON_RUNNER").map(PathBuf::from) {
        return runner_from_explicit_path(executable);
    }

    if let Some(runner) = find_managed_runner() {
        return Ok(runner);
    }

    for steam_root in steam_roots() {
        if let Some(executable) = find_steam_proton(&steam_root) {
            return Ok(Runner {
                executable,
                kind: RunnerKind::Proton,
                steam_root: Some(steam_root),
            });
        }
    }

    if let Some(executable) = find_in_path("proton") {
        return Ok(Runner {
            executable,
            kind: RunnerKind::Proton,
            steam_root: steam_roots().into_iter().next(),
        });
    }
    if let Some(executable) = find_in_path("wine") {
        return Ok(Runner {
            executable,
            kind: RunnerKind::Wine,
            steam_root: None,
        });
    }

    Err(
        "未找到 Proton 或 Wine。可安装兼容环境，或用 BMCBL_PROTON_RUNNER 指定可执行文件"
            .to_string(),
    )
}

pub(crate) async fn install_linux_runtime(plan: LinuxInstallPlan) -> Result<(), String> {
    register_task_stage_labels(INSTALL_STAGE_LABELS);
    let command_preview = plan.command_preview();
    let task_id = create_task_with_details(
        None,
        "安装 Linux 兼容环境",
        Some(format!("{} · {}", plan.distribution_name, command_preview)),
        "installing_linux_runtime",
        None,
        false,
    );
    append_task_log(&task_id, format!("将执行：{command_preview}"));
    append_task_log(
        &task_id,
        "BMCBL 主进程保持普通用户权限，仅包管理器通过 pkexec 请求授权",
    );

    let outcome = run_install_command(&plan, &task_id).await;
    match &outcome {
        Ok(()) => finish_task(&task_id, "completed", Some("兼容环境安装完成".to_string())),
        Err(error) => finish_task(&task_id, "error", Some(error.clone())),
    }
    outcome
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

fn build_install_plan(
    os_release: &OsRelease,
    distribution_name: Arc<str>,
) -> Option<LinuxInstallPlan> {
    let authorization_program = find_program(&["pkexec"])?;
    let family = format!("{} {}", os_release.id, os_release.id_like).to_ascii_lowercase();
    let packages: Arc<[Arc<str>]> = Arc::from([Arc::<str>::from("wine")]);
    let (package_manager, arguments): (PathBuf, Arc<[Arc<str>]>) =
        if contains_family(&family, &["fedora", "rhel", "centos", "rocky", "almalinux"]) {
            (
                find_program(&["dnf5", "dnf"])?,
                Arc::from([
                    Arc::<str>::from("-y"),
                    Arc::<str>::from("install"),
                    Arc::<str>::from("wine"),
                ]),
            )
        } else if contains_family(&family, &["debian", "ubuntu", "mint", "pop"]) {
            (
                find_program(&["apt-get"])?,
                Arc::from([
                    Arc::<str>::from("-y"),
                    Arc::<str>::from("install"),
                    Arc::<str>::from("wine"),
                ]),
            )
        } else if contains_family(&family, &["arch", "manjaro", "endeavouros"]) {
            (
                find_program(&["pacman"])?,
                Arc::from([
                    Arc::<str>::from("-S"),
                    Arc::<str>::from("--needed"),
                    Arc::<str>::from("--noconfirm"),
                    Arc::<str>::from("wine"),
                ]),
            )
        } else if contains_family(&family, &["suse", "opensuse"]) {
            (
                find_program(&["zypper"])?,
                Arc::from([
                    Arc::<str>::from("--non-interactive"),
                    Arc::<str>::from("install"),
                    Arc::<str>::from("wine"),
                ]),
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
    let entries = std::fs::read_dir(file_ops::runners_dir()).ok()?;
    let mut proton_candidates = Vec::new();
    let mut wine_candidates = Vec::new();
    for runner_root in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        let proton = runner_root.join("proton");
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
