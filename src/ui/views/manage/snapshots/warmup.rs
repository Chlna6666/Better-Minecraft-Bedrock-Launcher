use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{ManagePackSubtype, ManageTab, ManageVersionConfig, ManagedVersionEntry};
use super::{data, load_assets, load_external_servers, load_screenshots};
use crate::core::version::launch_versions::{LaunchVersionEntry, compare_versions_desc};
use crate::tasks::runtime;

const HOT_VERSIONS: usize = 4;
const MAX_USERS: usize = 1;
const MAX_REQUESTS: usize = 32;
static SCHEDULED: AtomicBool = AtomicBool::new(false);

pub(crate) fn prewarm(versions: Arc<[LaunchVersionEntry]>, locale: String) {
    if SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = runtime::spawn_io(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let mut budget = MAX_REQUESTS;
        for (version, public) in targets(&versions) {
            if budget == 0 {
                break;
            }
            let config = if public {
                ManageVersionConfig::default()
            } else {
                match data::load_version_config(&version).await {
                    Ok(config) => config,
                    Err(error) => {
                        tracing::debug!(%error, "Manage warmup config failed");
                        continue;
                    }
                }
            };
            warm_version(&version, &config, public, &locale, &mut budget).await;
        }
    })
    .map(drop)
    {
        SCHEDULED.store(false, Ordering::Release);
        tracing::warn!(%error, "Manage warmup was not scheduled");
    }
}

fn targets(versions: &[LaunchVersionEntry]) -> Vec<(ManagedVersionEntry, bool)> {
    // Public roots have no instance identity. The empty folder asks the existing
    // resolver for the original path without applying an installed instance config.
    let mut sorted = versions.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        right
            .game_info
            .total_sessions
            .cmp(&left.game_info.total_sessions)
            .then_with(|| compare_versions_desc(&left.version, &right.version))
            .then_with(|| left.folder.cmp(&right.folder))
    });
    let mut public = HashSet::new();
    let mut result = Vec::new();
    if let Some(version) = sorted.first() {
        result.push((managed(version), false));
    }
    for version in &sorted {
        let source = (
            version.kind.eq_ignore_ascii_case("gdk"),
            version.name.contains("Preview") || version.name.contains("Beta"),
        );
        if public.insert(source) {
            let mut original = managed(version);
            original.folder = "".into();
            result.push((original, true));
        }
    }
    result.extend(
        sorted
            .iter()
            .skip(1)
            .take(HOT_VERSIONS.saturating_sub(1))
            .map(|version| (managed(version), false)),
    );
    result
}

fn managed(version: &LaunchVersionEntry) -> ManagedVersionEntry {
    ManagedVersionEntry {
        folder: version.folder.to_string().into(),
        name: version.name.to_string().into(),
        version: version.version.to_string().into(),
        manifest_version: version.manifest_version.to_string().into(),
        path: version.path.to_string().into(),
        kind: version.kind.to_string().into(),
        icon_path: None,
        mod_loaders: Arc::from([]),
        game_info: Default::default(),
    }
}

async fn warm_version(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    public: bool,
    locale: &str,
    budget: &mut usize,
) {
    // Sequential background scans provide backpressure while each parser can use
    // the bounded scan pool. No task/queue is created for every installed version.
    for (tab, subtype) in [
        (ManageTab::Mod, ManagePackSubtype::Resource),
        (ManageTab::ResourcePack, ManagePackSubtype::Resource),
        (ManageTab::ResourcePack, ManagePackSubtype::Behavior),
        (ManageTab::Map, ManagePackSubtype::Resource),
        (ManageTab::SkinPack, ManagePackSubtype::Resource),
        (ManageTab::Cape, ManagePackSubtype::Resource),
    ] {
        if (public && tab == ManageTab::Mod) || (version.is_gdk() && tab == ManageTab::Map) {
            continue;
        }
        if !take_request(budget) {
            return;
        }
        report(load_assets(version, config, tab, subtype, None, locale).await);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    warm_accounts(version, config, locale, budget).await;
}

async fn warm_accounts(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    locale: &str,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    if version.is_gdk() {
        match data::load_gdk_users(version, config).await {
            Ok(mut users) => {
                users.sort_by(|left, right| right.last_modified.cmp(&left.last_modified));
                for user in users.iter().take(MAX_USERS) {
                    warm_user(
                        version,
                        config,
                        Some(user.folder_name.as_ref()),
                        locale,
                        budget,
                    )
                    .await;
                }
            }
            Err(error) => tracing::debug!(%error, "Manage user warmup failed"),
        }
    } else {
        warm_user(version, config, None, locale, budget).await;
    }
}

async fn warm_user(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    user: Option<&str>,
    locale: &str,
    budget: &mut usize,
) {
    if version.is_gdk() {
        if !take_request(budget) {
            return;
        }
        report(
            load_assets(
                version,
                config,
                ManageTab::Map,
                ManagePackSubtype::Resource,
                user,
                locale,
            )
            .await,
        );
    }
    if take_request(budget) {
        report(load_screenshots(version, config, user).await);
    }
    if take_request(budget) {
        report(load_external_servers(version, config, user).await);
    }
}

fn take_request(budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    true
}

fn report<T>(result: Result<T, String>) {
    if let Err(error) = result {
        tracing::debug!(%error, "Manage snapshot warmup failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warmup_bounds_isolated_versions_and_deduplicates_public_roots() {
        let versions = (0..200)
            .map(|index| LaunchVersionEntry {
                folder: format!("v{index}").into(),
                name: "Release".into(),
                version: format!("1.{index}").into(),
                manifest_version: "1".into(),
                path: format!("versions/v{index}").into(),
                kind: "uwp".into(),
                custom_icon_path: None,
                mod_loaders: Arc::from([]),
                game_info: crate::core::version::game_info::GameInfo {
                    total_sessions: u64::from(index == 3) * 100,
                    ..Default::default()
                },
            })
            .collect::<Vec<_>>();
        let targets = targets(&versions);
        assert_eq!(targets.len(), HOT_VERSIONS + 1);
        assert_eq!(targets[0].0.folder.as_ref(), "v3");
        assert!(targets[1].1);
        assert_eq!(targets[2].0.folder.as_ref(), "v199");
    }
}
