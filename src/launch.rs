use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

const DEFAULT_UPDATER_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportLaunchContext {
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectLaunchContext {
    pub version_folder: String,
    pub silent_override: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    Main,
    Import(ImportLaunchContext),
    Updater(UpdaterLaunchContext),
    DirectLaunch(DirectLaunchContext),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdaterLaunchContext {
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub timeout_secs: u64,
}

impl LaunchMode {
    pub fn is_main(&self) -> bool {
        matches!(self, Self::Main | Self::DirectLaunch(_))
    }

    pub fn is_import(&self) -> bool {
        matches!(self, Self::Import(_))
    }

    pub fn is_direct_launch(&self) -> bool {
        matches!(self, Self::DirectLaunch(_))
    }
}

#[derive(Debug, Parser)]
#[command(name = "BMCBL", disable_help_subcommand = true)]
struct Cli {
    #[arg(
        long = "run-updater",
        value_name = "UPDATER_ARG",
        num_args = 3,
        hide = true
    )]
    legacy_run_updater: Option<Vec<PathBuf>>,

    #[arg(long = "import-file", value_name = "FILE")]
    import_file: Option<PathBuf>,

    #[arg(long = "launch-version", value_name = "VERSION")]
    launch_version: Option<String>,

    #[arg(long = "silent")]
    silent: bool,

    #[arg(long = "gui")]
    gui: bool,

    #[arg(value_name = "FILE")]
    shell_open_target: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    RunUpdater {
        source_path: PathBuf,
        destination_path: PathBuf,
        #[arg(default_value_t = DEFAULT_UPDATER_TIMEOUT_SECS)]
        timeout_secs: u64,
    },
}

pub fn parse_launch_mode() -> LaunchMode {
    parse_launch_mode_from_cli(Cli::parse())
}

fn parse_launch_mode_from_cli(cli: Cli) -> LaunchMode {
    if let Some(args) = cli.legacy_run_updater
        && let [source_path, destination_path, timeout_path] = args.as_slice()
    {
        let timeout_secs = timeout_path
            .to_str()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_UPDATER_TIMEOUT_SECS);
        return LaunchMode::Updater(UpdaterLaunchContext {
            source_path: source_path.clone(),
            destination_path: destination_path.clone(),
            timeout_secs,
        });
    }

    if let Some(CliCommand::RunUpdater {
        source_path,
        destination_path,
        timeout_secs,
    }) = cli.command
    {
        return LaunchMode::Updater(UpdaterLaunchContext {
            source_path,
            destination_path,
            timeout_secs,
        });
    }

    if let Some(version_folder) = cli.launch_version {
        let silent_override = if cli.silent {
            Some(true)
        } else if cli.gui {
            Some(false)
        } else {
            None
        };
        return LaunchMode::DirectLaunch(DirectLaunchContext {
            version_folder,
            silent_override,
        });
    }

    let import_candidate = cli.import_file.or(cli.shell_open_target);
    if let Some(file_path) = import_candidate.filter(|path| is_import_asset_file(path)) {
        return LaunchMode::Import(ImportLaunchContext { file_path });
    }

    LaunchMode::Main
}

#[cfg(test)]
fn parse_launch_mode_from<I, T>(args: I) -> Result<LaunchMode, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Cli::try_parse_from(args).map(parse_launch_mode_from_cli)
}

fn is_import_asset_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };

    matches!(
        extension.to_ascii_lowercase().as_str(),
        "mcpack" | "mcworld" | "mcaddon" | "mctemplate"
    )
}

#[cfg(test)]
mod tests {
    use super::{ImportLaunchContext, LaunchMode, UpdaterLaunchContext, parse_launch_mode_from};
    use std::path::PathBuf;

    #[test]
    fn parse_launch_mode_returns_main_without_args() {
        let launch_mode = parse_launch_mode_from(["BMCBL"]).expect("parse launch args");

        assert_eq!(launch_mode, LaunchMode::Main);
    }

    #[test]
    fn parse_launch_mode_returns_import_for_import_file_arg() {
        let file_path = PathBuf::from("pack.mcpack");
        let launch_mode = parse_launch_mode_from(["BMCBL", "--import-file", "pack.mcpack"])
            .expect("parse launch args");

        assert_eq!(
            launch_mode,
            LaunchMode::Import(ImportLaunchContext { file_path })
        );
    }

    #[test]
    fn parse_launch_mode_ignores_non_import_shell_target() {
        let launch_mode =
            parse_launch_mode_from(["BMCBL", "notes.txt"]).expect("parse launch args");

        assert_eq!(launch_mode, LaunchMode::Main);
    }

    #[test]
    fn parse_launch_mode_returns_updater_command() {
        let launch_mode = parse_launch_mode_from([
            "BMCBL",
            "run-updater",
            "source.exe",
            "destination.exe",
            "15",
        ])
        .expect("parse launch args");

        assert_eq!(
            launch_mode,
            LaunchMode::Updater(UpdaterLaunchContext {
                source_path: PathBuf::from("source.exe"),
                destination_path: PathBuf::from("destination.exe"),
                timeout_secs: 15,
            })
        );
    }

    #[test]
    fn parse_launch_mode_accepts_legacy_updater_flag() {
        let launch_mode = parse_launch_mode_from([
            "BMCBL",
            "--run-updater",
            "source.exe",
            "destination.exe",
            "15",
        ])
        .expect("parse launch args");

        assert_eq!(
            launch_mode,
            LaunchMode::Updater(UpdaterLaunchContext {
                source_path: PathBuf::from("source.exe"),
                destination_path: PathBuf::from("destination.exe"),
                timeout_secs: 15,
            })
        );
    }
}
