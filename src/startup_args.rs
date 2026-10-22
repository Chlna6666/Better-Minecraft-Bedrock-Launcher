use crate::launch::{DEFAULT_UPDATER_TIMEOUT_SECS, LaunchOptions, LaunchUiMode, is_import_file};
use clap::{ArgGroup, Parser};
use std::ffi::OsString;
use std::path::PathBuf;

const STARTUP_MODES: [&str; 8] = [
    "launch",
    "legacy_launch_version",
    "import",
    "legacy_import_file",
    "updater",
    "legacy_run_updater",
    "terminal_host",
    "shell_open_target",
];

#[derive(Debug, Parser)]
#[command(
    name = "BMCBL",
    bin_name = "BMCBL",
    disable_version_flag = true,
    group = ArgGroup::new("startup_mode").args(STARTUP_MODES).multiple(false)
)]
struct StartupCli {
    /// 启动指定版本；窗口行为由实例的“快捷方式静默启动”设置决定。
    #[arg(long = "launch", value_name = "VERSION", group = "startup_mode")]
    launch: Option<String>,

    /// 已弃用：等价于 --launch。将从 0.3.0 开始标记弃用。
    #[arg(
        long = "launch-version",
        value_name = "VERSION",
        hide = true,
        group = "startup_mode"
    )]
    legacy_launch_version: Option<String>,

    /// 导入指定的资源包、世界、附加包或安装包并打开启动器。
    #[arg(long = "import", value_name = "FILE", group = "startup_mode")]
    import: Option<PathBuf>,

    /// 已弃用：等价于 --import。将从 0.3.0 开始标记弃用。
    #[arg(
        long = "import-file",
        value_name = "FILE",
        hide = true,
        group = "startup_mode"
    )]
    legacy_import_file: Option<PathBuf>,

    /// 内部：以更新器子进程运行。
    #[arg(
        long = "updater",
        value_name = "UPDATER_ARG",
        num_args = 2..=3,
        hide = true,
        group = "startup_mode"
    )]
    updater: Option<Vec<PathBuf>>,

    /// 已弃用：等价于 --updater。将从 0.3.0 开始标记弃用。
    #[arg(
        long = "run-updater",
        value_name = "UPDATER_ARG",
        num_args = 3,
        hide = true,
        group = "startup_mode"
    )]
    legacy_run_updater: Option<Vec<PathBuf>>,

    /// 内部：Windows Terminal 控制台宿主。
    #[arg(
        long = "terminal-host",
        value_name = "ARG",
        num_args = 2,
        hide = true,
        group = "startup_mode"
    )]
    terminal_host: Option<Vec<PathBuf>>,

    /// 兼容系统“打开方式”：直接传入待导入文件。
    #[arg(value_name = "FILE", hide = true, group = "startup_mode")]
    shell_open_target: Option<PathBuf>,

    /// 不打开主窗口，直接启动游戏。
    #[arg(long)]
    silent: bool,

    /// 打开主窗口并自动启动游戏。
    #[arg(long, conflicts_with = "silent")]
    gui: bool,

    /// 覆盖传给游戏的启动参数。
    #[arg(long = "game-args", value_name = "ARGS", allow_hyphen_values = true)]
    game_args: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdaterCommand {
    pub(crate) source_path: PathBuf,
    pub(crate) destination_path: PathBuf,
    pub(crate) timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalHostCommand {
    pub(crate) command_path: PathBuf,
    pub(crate) response_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupAction {
    Gui,
    Launch(LaunchOptions),
    Import(PathBuf),
    Updater(UpdaterCommand),
    TerminalHost(TerminalHostCommand),
}

pub(crate) fn parse<I, T>(args: I) -> Result<StartupAction, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    StartupCli::try_parse_from(args).and_then(action_from_cli)
}

fn action_from_cli(cli: StartupCli) -> Result<StartupAction, clap::Error> {
    let launch_version = cli.launch.or(cli.legacy_launch_version);
    if cli.silent || cli.gui || cli.game_args.is_some() {
        if launch_version.is_none() {
            return Err(usage_error(
                "--silent、--gui 和 --game-args 只能与 --launch 或 --launch-version 一起使用",
            ));
        }
    }

    if let Some(version) = launch_version {
        let ui_mode = if cli.silent {
            LaunchUiMode::Silent
        } else if cli.gui {
            LaunchUiMode::Gui
        } else {
            LaunchUiMode::Auto
        };
        return Ok(StartupAction::Launch(LaunchOptions {
            version: Some(version),
            ui_mode,
            game_args: cli.game_args,
            json: false,
        }));
    }

    if let Some(args) = cli.updater {
        return updater_action(args);
    }
    if let Some(args) = cli.legacy_run_updater {
        return legacy_updater_action(args);
    }
    if let Some(args) = cli.terminal_host {
        let [command_path, response_path] = args.as_slice() else {
            return Err(usage_error("--terminal-host 需要 2 个路径参数"));
        };
        return Ok(StartupAction::TerminalHost(TerminalHostCommand {
            command_path: command_path.clone(),
            response_path: response_path.clone(),
        }));
    }
    if let Some(path) = cli.import {
        return import_action(path, false);
    }
    if let Some(path) = cli.legacy_import_file {
        return import_action(path, true);
    }
    if let Some(path) = cli.shell_open_target {
        return import_action(path, true);
    }

    Ok(StartupAction::Gui)
}

fn updater_action(args: Vec<PathBuf>) -> Result<StartupAction, clap::Error> {
    let (source_path, destination_path, timeout) = match args.as_slice() {
        [source_path, destination_path] => (source_path.clone(), destination_path.clone(), None),
        [source_path, destination_path, timeout] => {
            (source_path.clone(), destination_path.clone(), Some(timeout))
        }
        _ => return Err(usage_error("--updater 需要 2 或 3 个参数")),
    };
    let timeout_secs = match timeout {
        Some(timeout) => parse_timeout(timeout)?,
        None => DEFAULT_UPDATER_TIMEOUT_SECS,
    };
    Ok(StartupAction::Updater(UpdaterCommand {
        source_path,
        destination_path,
        timeout_secs,
    }))
}

fn legacy_updater_action(args: Vec<PathBuf>) -> Result<StartupAction, clap::Error> {
    let [source_path, destination_path, timeout] = args.as_slice() else {
        return Err(usage_error("--run-updater 需要 3 个参数"));
    };
    Ok(StartupAction::Updater(UpdaterCommand {
        source_path: source_path.clone(),
        destination_path: destination_path.clone(),
        // The released 0.2.x updater child always passed a number here; tolerate
        // a malformed legacy value instead of breaking in-place updates.
        timeout_secs: parse_timeout(timeout).unwrap_or(DEFAULT_UPDATER_TIMEOUT_SECS),
    }))
}

fn parse_timeout(timeout: &PathBuf) -> Result<u64, clap::Error> {
    let text = timeout
        .to_str()
        .ok_or_else(|| usage_error("更新器超时秒数必须是有效文本"))?;
    text.parse::<u64>()
        .map_err(|_| usage_error(format!("更新器超时秒数必须是整数：{text}")))
}

fn import_action(path: PathBuf, legacy: bool) -> Result<StartupAction, clap::Error> {
    if is_import_file(&path) {
        return Ok(StartupAction::Import(path));
    }
    if legacy {
        // File associations and "open with" may hand us arbitrary files; keep
        // the old fallback of opening the main window.
        return Ok(StartupAction::Gui);
    }
    Err(usage_error(format!(
        "不支持导入的文件类型：{}（支持：{}）",
        path.display(),
        crate::launch::IMPORT_EXTENSIONS.join("、")
    )))
}

fn usage_error(message: impl Into<String>) -> clap::Error {
    clap::Error::raw(clap::error::ErrorKind::ValueValidation, message.into())
}

#[cfg(test)]
mod tests {
    use super::{StartupAction, TerminalHostCommand, UpdaterCommand, parse};
    use crate::launch::{LaunchOptions, LaunchUiMode};
    use std::path::PathBuf;

    fn action(args: &[&str]) -> StartupAction {
        parse(std::iter::once("BMCBL").chain(args.iter().copied()))
            .expect("startup arguments should parse")
    }

    #[test]
    fn no_startup_arguments_open_the_launcher() {
        assert_eq!(action(&[]), StartupAction::Gui);
    }

    #[test]
    fn launch_defaults_to_the_instance_silent_setting() {
        assert_eq!(
            action(&["--launch", "1.21.0"]),
            StartupAction::Launch(LaunchOptions {
                version: Some("1.21.0".to_string()),
                ui_mode: LaunchUiMode::Auto,
                game_args: None,
                json: false,
            })
        );
    }

    #[test]
    fn launch_accepts_window_and_game_argument_overrides() {
        assert_eq!(
            action(&["--launch", "1.21.0", "--silent"]),
            StartupAction::Launch(LaunchOptions {
                version: Some("1.21.0".to_string()),
                ui_mode: LaunchUiMode::Silent,
                game_args: None,
                json: false,
            })
        );
        assert_eq!(
            action(&[
                "--launch",
                "1.21.0",
                "--gui",
                "--game-args",
                "-server 127.0.0.1"
            ]),
            StartupAction::Launch(LaunchOptions {
                version: Some("1.21.0".to_string()),
                ui_mode: LaunchUiMode::Gui,
                game_args: Some("-server 127.0.0.1".to_string()),
                json: false,
            })
        );
    }

    #[test]
    fn legacy_launch_version_remains_compatible() {
        assert_eq!(
            action(&["--launch-version", "1.21.0", "--silent"]),
            StartupAction::Launch(LaunchOptions {
                version: Some("1.21.0".to_string()),
                ui_mode: LaunchUiMode::Silent,
                game_args: None,
                json: false,
            })
        );
    }

    #[test]
    fn launch_modifiers_require_a_launch_mode() {
        assert!(parse(["BMCBL", "--silent"]).is_err());
        assert!(parse(["BMCBL", "--gui"]).is_err());
        assert!(parse(["BMCBL", "--game-args", "-x"]).is_err());
        assert!(parse(["BMCBL", "--launch", "1.21.0", "--silent", "--gui"]).is_err());
    }

    #[test]
    fn startup_modes_are_mutually_exclusive() {
        assert!(parse(["BMCBL", "--launch", "1.21.0", "--launch-version", "1.20.0"]).is_err());
        assert!(parse(["BMCBL", "--import", "a.mcpack", "--launch", "1.21.0"]).is_err());
    }

    #[test]
    fn import_accepts_canonical_legacy_and_shell_paths() {
        assert_eq!(
            action(&["--import", "pack.mcpack"]),
            StartupAction::Import(PathBuf::from("pack.mcpack"))
        );
        assert_eq!(
            action(&["--import-file", "world.mcworld"]),
            StartupAction::Import(PathBuf::from("world.mcworld"))
        );
        assert_eq!(
            action(&["pack.mcaddon"]),
            StartupAction::Import(PathBuf::from("pack.mcaddon"))
        );
    }

    #[test]
    fn canonical_import_rejects_unsupported_files_but_legacy_paths_open_the_launcher() {
        assert!(parse(["BMCBL", "--import", "notes.txt"]).is_err());
        assert_eq!(action(&["--import-file", "notes.txt"]), StartupAction::Gui);
        assert_eq!(action(&["notes.txt"]), StartupAction::Gui);
    }

    #[test]
    fn updater_accepts_two_or_three_arguments() {
        let source = PathBuf::from("source.exe");
        let destination = PathBuf::from("destination.exe");
        assert_eq!(
            action(&["--updater", "source.exe", "destination.exe"]),
            StartupAction::Updater(UpdaterCommand {
                source_path: source.clone(),
                destination_path: destination.clone(),
                timeout_secs: 60,
            })
        );
        assert_eq!(
            action(&["--updater", "source.exe", "destination.exe", "15"]),
            StartupAction::Updater(UpdaterCommand {
                source_path: source,
                destination_path: destination,
                timeout_secs: 15,
            })
        );
        assert!(
            parse([
                "BMCBL",
                "--updater",
                "source.exe",
                "destination.exe",
                "soon"
            ])
            .is_err()
        );
    }

    #[test]
    fn legacy_updater_flag_remains_compatible() {
        assert_eq!(
            action(&["--run-updater", "source.exe", "destination.exe", "15"]),
            StartupAction::Updater(UpdaterCommand {
                source_path: PathBuf::from("source.exe"),
                destination_path: PathBuf::from("destination.exe"),
                timeout_secs: 15,
            })
        );
        // 0.2.x tolerated a malformed legacy timeout and used the default.
        assert_eq!(
            action(&["--run-updater", "source.exe", "destination.exe", "soon"]),
            StartupAction::Updater(UpdaterCommand {
                source_path: PathBuf::from("source.exe"),
                destination_path: PathBuf::from("destination.exe"),
                timeout_secs: 60,
            })
        );
    }

    #[test]
    fn terminal_host_parses_two_paths() {
        assert_eq!(
            action(&["--terminal-host", "command.json", "response.json"]),
            StartupAction::TerminalHost(TerminalHostCommand {
                command_path: PathBuf::from("command.json"),
                response_path: PathBuf::from("response.json"),
            })
        );
        assert!(parse(["BMCBL", "--terminal-host", "command.json"]).is_err());
    }
}
