use crate::launch::{LaunchOptions, LaunchUiMode, is_import_file};
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "BMCBL",
    bin_name = "BMCBL",
    version = env!("BMCBL_BUILD_VERSION"),
    about = "BMCBL 终端命令行接口",
    long_about = "BMCBL 终端命令行接口。\n\n这里只包含终端命令。快捷方式、文件关联和更新器使用的启动参数是另一套接口，见 docs/STARTUP_ARGUMENTS.md。",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// 启动已安装的游戏版本；默认不打开主窗口。
    Launch(LaunchArgs),
    /// 导入资源包、世界、附加包或安装包，并打开启动器界面。
    Import(ImportArgs),
    /// 列出本地已安装的游戏版本。
    List(ListArgs),
    /// 显示版本、构建信息与数据目录。
    Info(InfoArgs),
}

#[derive(Debug, Args)]
struct LaunchArgs {
    /// 版本目录名、显示名或游戏版本；省略时使用排序后的第一个版本（优先总启动次数，其次版本号）。
    #[arg(value_name = "VERSION")]
    version: Option<String>,

    /// 不打开主窗口，直接启动游戏（默认）。
    #[arg(long, conflicts_with = "gui")]
    silent: bool,

    /// 打开主窗口并自动启动游戏。
    #[arg(long)]
    gui: bool,

    /// 覆盖传给游戏的启动参数。
    #[arg(long = "game-args", value_name = "ARGS", allow_hyphen_values = true)]
    game_args: Option<String>,

    /// 以 JSON 输出启动结果；仅用于静默启动。
    #[arg(long, conflicts_with = "gui")]
    json: bool,
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// 待导入的文件。
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

#[derive(Debug, Args)]
struct ListArgs {
    /// 以 JSON 输出。
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct InfoArgs {
    /// 以 JSON 输出。
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliAction {
    Launch(LaunchOptions),
    Import(PathBuf),
    List { json: bool },
    Info { json: bool },
}

/// Returns true when the process invocation belongs to the terminal CLI rather
/// than the program-startup arguments used by shortcuts and file associations.
pub(crate) fn is_cli_invocation(args: &[OsString]) -> bool {
    let Some(first) = args.get(1).and_then(|argument| argument.to_str()) else {
        return false;
    };
    matches!(
        first,
        "-h" | "--help" | "-V" | "--version" | "help" | "launch" | "import" | "list" | "info"
    )
}

pub(crate) fn parse<I, T>(args: I) -> Result<CliAction, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(args).and_then(action_from_cli)
}

fn action_from_cli(cli: Cli) -> Result<CliAction, clap::Error> {
    match cli.command {
        CliCommand::Launch(args) => Ok(CliAction::Launch(LaunchOptions {
            version: args.version,
            // Terminal launches never follow the shortcut setting; a window is
            // opened only when the caller explicitly asks for it.
            ui_mode: if args.gui {
                LaunchUiMode::Gui
            } else {
                LaunchUiMode::Silent
            },
            game_args: args.game_args,
            json: args.json,
        })),
        CliCommand::Import(args) => import_action(args.file),
        CliCommand::List(args) => Ok(CliAction::List { json: args.json }),
        CliCommand::Info(args) => Ok(CliAction::Info { json: args.json }),
    }
}

fn import_action(path: PathBuf) -> Result<CliAction, clap::Error> {
    if is_import_file(&path) {
        return Ok(CliAction::Import(path));
    }
    Err(clap::Error::raw(
        clap::error::ErrorKind::ValueValidation,
        format!(
            "不支持导入的文件类型：{}（支持：{}）",
            path.display(),
            crate::launch::IMPORT_EXTENSIONS.join("、")
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::{CliAction, is_cli_invocation, parse};
    use crate::launch::{LaunchOptions, LaunchUiMode};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn action(args: &[&str]) -> CliAction {
        parse(std::iter::once("BMCBL").chain(args.iter().copied()))
            .expect("terminal CLI arguments should parse")
    }

    fn invocation(args: &[&str]) -> bool {
        let args = std::iter::once("BMCBL")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect::<Vec<_>>();
        is_cli_invocation(&args)
    }

    #[test]
    fn detection_separates_terminal_commands_from_startup_arguments() {
        assert!(invocation(&["launch"]));
        assert!(invocation(&["import", "pack.mcpack"]));
        assert!(invocation(&["list"]));
        assert!(invocation(&["info"]));
        assert!(invocation(&["--help"]));
        assert!(invocation(&["--version"]));

        assert!(!invocation(&[]));
        assert!(!invocation(&["--launch", "1.21.0"]));
        assert!(!invocation(&["--launch-version", "1.21.0"]));
        assert!(!invocation(&["--import", "pack.mcpack"]));
        assert!(!invocation(&["pack.mcpack"]));
    }

    #[test]
    fn launch_defaults_to_silent_mode() {
        assert_eq!(
            action(&["launch", "1.21.0"]),
            CliAction::Launch(LaunchOptions {
                version: Some("1.21.0".to_string()),
                ui_mode: LaunchUiMode::Silent,
                game_args: None,
                json: false,
            })
        );
    }

    #[test]
    fn launch_supports_gui_and_game_arguments() {
        assert_eq!(
            action(&["launch", "--gui", "--game-args", "-server 127.0.0.1"]),
            CliAction::Launch(LaunchOptions {
                version: None,
                ui_mode: LaunchUiMode::Gui,
                game_args: Some("-server 127.0.0.1".to_string()),
                json: false,
            })
        );
    }

    #[test]
    fn launch_supports_json_output() {
        assert_eq!(
            action(&["launch", "1.21.0", "--silent", "--json"]),
            CliAction::Launch(LaunchOptions {
                version: Some("1.21.0".to_string()),
                ui_mode: LaunchUiMode::Silent,
                game_args: None,
                json: true,
            })
        );
    }

    #[test]
    fn launch_rejects_conflicting_window_flags() {
        assert!(parse(["BMCBL", "launch", "--silent", "--gui"]).is_err());
        assert!(parse(["BMCBL", "launch", "--gui", "--json"]).is_err());
    }

    #[test]
    fn import_accepts_supported_extensions_and_rejects_others() {
        assert_eq!(
            action(&["import", "pack.mcpack"]),
            CliAction::Import(PathBuf::from("pack.mcpack"))
        );
        assert!(parse(["BMCBL", "import", "notes.txt"]).is_err());
    }

    #[test]
    fn list_and_info_support_json_output() {
        assert_eq!(action(&["list", "--json"]), CliAction::List { json: true });
        assert_eq!(action(&["info", "--json"]), CliAction::Info { json: true });
    }
}
