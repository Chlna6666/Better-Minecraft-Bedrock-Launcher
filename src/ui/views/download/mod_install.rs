use std::path::PathBuf;

use gpui::{App, AppContext as _, BorrowAppContext as _, SharedString};
use tracing::warn;

use crate::core::levilamina::{
    LeviLaminaModEntry, cached_support_database, inspect_installation, mod_release_supports_loader,
};
use crate::ui::state::local_versions::LocalVersionsState;

use super::state::{DownloadPageState, LeviLaminaModInstallTarget};

struct InstallCandidate {
    folder: String,
    path: PathBuf,
    game_version: String,
}

pub(super) fn open_modal(mod_entry: LeviLaminaModEntry, cx: &mut App) {
    let candidates = cx.read_global(|versions: &LocalVersionsState, _cx| {
        versions
            .versions
            .iter()
            .map(|version| InstallCandidate {
                folder: version.folder.to_string(),
                path: PathBuf::from(version.path.as_ref()),
                game_version: version.version.to_string(),
            })
            .collect::<Vec<_>>()
    });
    let package_id = mod_entry.package_id.clone();
    let request_id = cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.levilauncher_install_targets_request_id = state
            .levilauncher_install_targets_request_id
            .wrapping_add(1);
        state.levilauncher_selected_mod = Some(mod_entry.clone());
        state.levilauncher_selected_version = SharedString::from("");
        state.levilauncher_modal_open = true;
        state.levilauncher_install_target_path = None;
        state.levilauncher_install_target_version = SharedString::from("");
        state.levilauncher_install_targets.clear();
        state.levilauncher_install_targets_loading = true;
        state.levilauncher_install_error = None;
        state.levilauncher_install_targets_request_id
    });

    cx.spawn(async move |cx| {
        let task = gpui_tokio::Tokio::spawn_result(cx, async move {
            discover_targets(candidates, &mod_entry)
                .await
                .map_err(anyhow::Error::msg)
        });
        let result = task.await;
        cx.update_global(|state: &mut DownloadPageState, _cx| {
            if state.levilauncher_install_targets_request_id != request_id
                || state
                    .levilauncher_selected_mod
                    .as_ref()
                    .is_none_or(|entry| entry.package_id != package_id)
            {
                return;
            }
            state.levilauncher_install_targets_loading = false;
            match result {
                Ok(targets) => {
                    state.levilauncher_install_targets = targets;
                    if let Some(target) = state.levilauncher_install_targets.first() {
                        state.levilauncher_install_target_path = Some(target.path.clone());
                        state.levilauncher_install_target_version = target.game_version.clone();
                        state.levilauncher_selected_version = state
                            .levilauncher_selected_mod
                            .as_ref()
                            .and_then(|entry| {
                                compatible_releases(entry, target.loader_version.as_ref())
                                    .into_iter()
                                    .next()
                            })
                            .map_or_else(|| SharedString::from(""), SharedString::from);
                    }
                }
                Err(error) => {
                    state.levilauncher_install_error = Some(SharedString::from(error.to_string()));
                }
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach_and_log_err(cx);
}

pub(super) fn compatible_releases(
    mod_entry: &LeviLaminaModEntry,
    loader_version: &str,
) -> Vec<String> {
    mod_entry
        .client_versions
        .iter()
        .filter(|release| mod_release_supports_loader(mod_entry, release, loader_version))
        .cloned()
        .collect()
}

async fn discover_targets(
    candidates: Vec<InstallCandidate>,
    mod_entry: &LeviLaminaModEntry,
) -> Result<Vec<LeviLaminaModInstallTarget>, String> {
    let support = cached_support_database().await?;
    let mut targets = Vec::new();
    let mut failures = Vec::new();
    for candidate in candidates {
        let installation = match inspect_installation(candidate.path.clone()).await {
            Ok(installation) => installation,
            Err(error) => {
                warn!(folder = %candidate.folder, %error, "检查 LeviLamina 安装状态失败");
                failures.push(format!("{}: {error}", candidate.folder));
                continue;
            }
        };
        let Some(loader_version) = installation.loader_version else {
            continue;
        };
        if !support.supports_loader(&candidate.game_version, &loader_version)
            || compatible_releases(mod_entry, &loader_version).is_empty()
        {
            continue;
        }
        targets.push(LeviLaminaModInstallTarget {
            path: SharedString::from(candidate.path.to_string_lossy().into_owned()),
            game_version: SharedString::from(candidate.game_version.clone()),
            loader_version: SharedString::from(loader_version.clone()),
            label: SharedString::from(format!(
                "{} ({}) · LeviLamina {}",
                candidate.folder, candidate.game_version, loader_version
            )),
        });
    }
    if targets.is_empty() && !failures.is_empty() {
        return Err(format!("部分游戏实例检查失败：{}", failures.join("；")));
    }
    Ok(targets)
}
