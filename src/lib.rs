// SPDX-FileCopyrightText: 2026 Chlna6666 and BMCBL contributors
// SPDX-License-Identifier: GPL-3.0-only
// BMCBL upstream: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher
// AI/code-reuse attribution: preserve upstream credit and license notices when
// reproducing or adapting copyrighted code. See the repository NOTICE.md.
#![recursion_limit = "512"]
// Application-layer dead code is retained on purpose: unreached feature
// scaffolding and in-progress work stay compiled so they keep passing check
// without each feature committing to a release path.
#![allow(dead_code)]

#[macro_use]
mod i18n;

mod app;
mod archive;
mod assets;
mod cli;
mod config;
mod core;
mod downloads;
mod frame_rate;
mod github;
mod http;
mod launch;
mod plugins;
mod result;

/// 插件系统性能基准使用的入口。
///
/// 只在 cargo bench --features bench-support 下存在，正常构建不会暴露这些 re-export。
#[cfg(feature = "bench-support")]
pub mod bench_support;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod startup;
mod startup_args;
mod tasks;
mod ui;
mod utils;

pub use app::APP_ID;

/// Render process-level failures through the Unicode-aware console boundary.
pub fn print_cli_error(message: &str) {
    let _ = utils::console_output::write_stderr(message);
}

fn exit_parse_error(error: clap::Error) -> ! {
    // Clap's default .exit() writes UTF-8 through CRT stdout/stderr, which is
    // unreadable in legacy OEM code pages. Render without ANSI and preserve
    // Clap's normal destination and exit code.
    let message = error.render().to_string();
    let exit_code = error.exit_code();
    let output = if error.use_stderr() {
        utils::console_output::write_stderr(&message)
    } else {
        utils::console_output::write_stdout(&message)
    };
    let _ = output;
    std::process::exit(exit_code);
}


pub fn run() -> anyhow::Result<()> {
    let args = migrate_legacy_invocation(std::env::args_os().collect());
    let is_cli = cli::is_cli_invocation(&args);

    if is_cli {
        return match cli::parse(args) {
            Ok(action) => {
                #[cfg(target_os = "windows")]
                if cli_action_uses_console(&action) {
                    utils::console::attach_parent_console();
                }
                startup::run(startup::Invocation::Cli(action))
            }
            Err(error) => {
                #[cfg(target_os = "windows")]
                utils::console::attach_parent_console();
                exit_parse_error(error)
            }
        };
    }

    match startup_args::parse(args) {
        Ok(action) => {
            #[cfg(target_os = "windows")]
            if startup_action_uses_console(&action) {
                utils::console::attach_parent_console();
            }
            startup::run(startup::Invocation::Startup(action))
        }
        Err(error) => {
            #[cfg(target_os = "windows")]
            utils::console::attach_parent_console();
            exit_parse_error(error)
        }
    }
}

/// Keeps the 0.2.x internal invocations working until the documented 0.3.0
/// deprecation: the removed subcommands/flags are rewritten to canonical
/// startup parameters before dispatch.
fn migrate_legacy_invocation(mut args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let Some(first) = args.get(1).and_then(|argument| argument.to_str()) else {
        return args;
    };
    let replacement = match first {
        "--bmcbl-terminal-host" | "terminal-host" => Some("--terminal-host"),
        "run-updater" => Some("--updater"),
        _ => None,
    };
    if let Some(replacement) = replacement {
        args[1] = replacement.into();
    }
    args
}

#[cfg(target_os = "windows")]
fn cli_action_uses_console(action: &cli::CliAction) -> bool {
    match action {
        cli::CliAction::Launch(options) => options.ui_mode != launch::LaunchUiMode::Gui,
        cli::CliAction::Import(_) => false,
        cli::CliAction::List { .. } | cli::CliAction::Info { .. } => true,
    }
}

#[cfg(target_os = "windows")]
fn startup_action_uses_console(action: &startup_args::StartupAction) -> bool {
    match action {
        startup_args::StartupAction::Launch(options) => {
            options.ui_mode == launch::LaunchUiMode::Silent
        }
        startup_args::StartupAction::Gui
        | startup_args::StartupAction::Import(_)
        | startup_args::StartupAction::Updater(_)
        | startup_args::StartupAction::TerminalHost(_) => false,
    }
}
#[cfg(test)]
mod invocation_tests {
    use super::migrate_legacy_invocation;
    use std::ffi::OsString;

    fn migrate(args: &[&str]) -> Vec<String> {
        migrate_legacy_invocation(args.iter().map(OsString::from).collect())
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn rewrites_removed_internal_invocations() {
        assert_eq!(
            migrate(&["BMCBL.exe", "run-updater", "src", "dst", "15"]),
            ["BMCBL.exe", "--updater", "src", "dst", "15"]
        );
        assert_eq!(
            migrate(&["BMCBL.exe", "terminal-host", "command", "response"]),
            ["BMCBL.exe", "--terminal-host", "command", "response"]
        );
        assert_eq!(
            migrate(&["BMCBL.exe", "--bmcbl-terminal-host", "command", "response"]),
            ["BMCBL.exe", "--terminal-host", "command", "response"]
        );
    }

    #[test]
    fn leaves_current_arguments_untouched() {
        assert_eq!(
            migrate(&["BMCBL.exe", "launch", "1.21.0"]),
            ["BMCBL.exe", "launch", "1.21.0"]
        );
        assert_eq!(
            migrate(&["BMCBL.exe", "--launch", "1.21.0"]),
            ["BMCBL.exe", "--launch", "1.21.0"]
        );
    }
}
