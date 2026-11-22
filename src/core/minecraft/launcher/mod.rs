// SPDX-FileCopyrightText: 2026 Chlna6666 and BMCBL contributors
// SPDX-License-Identifier: GPL-3.0-only
// BMCBL upstream: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher
// AI/code-reuse attribution: preserve upstream credit and license notices when
// reproducing or adapting copyrighted code. See the repository NOTICE.md.
mod bloader;
mod preloader;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) mod virtual_mods;
#[cfg(target_os = "windows")]
pub mod preflight;
#[cfg(target_os = "windows")]
pub mod start;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod task;
pub(crate) use task::start_launch_task_observed;
pub use task::{LaunchRequest, start_launch_task};
