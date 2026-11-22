// SPDX-FileCopyrightText: 2026 Chlna6666 and BMCBL contributors
// SPDX-License-Identifier: GPL-3.0-only
// BMCBL upstream: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher
// AI/code-reuse attribution: preserve upstream credit and license notices when
// reproducing or adapting copyrighted code. See the repository NOTICE.md.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    match bmcbl::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            bmcbl::print_cli_error(&format!("Error: {error:#}\n"));
            std::process::ExitCode::from(1)
        }
    }
}
