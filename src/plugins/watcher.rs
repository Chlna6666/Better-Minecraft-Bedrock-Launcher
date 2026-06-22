use crate::plugins::manifest::{
    PLUGIN_MANIFEST_FILE, PLUGIN_PACKAGE_EXTENSION, PLUGIN_USER_CONFIG_FILE,
};
use anyhow::{Context, Result};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::{App, Timer};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tracing::warn;

pub const HOT_RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);
pub type PluginWatcherSender = UnboundedSender<PluginWatcherMessage>;

#[derive(Clone, Debug)]
pub enum PluginWatcherMessage {
    Changed(PathBuf),
    HttpRefresh,
    Stop,
}

pub fn spawn_plugin_watcher(
    plugins_dir: PathBuf,
    cx: &mut App,
) -> Result<(PluginWatcherSender, gpui::Task<()>)> {
    std::fs::create_dir_all(&plugins_dir)
        .with_context(|| format!("create plugins dir {}", plugins_dir.display()))?;

    let (tx, rx) = unbounded::<PluginWatcherMessage>();
    let watcher_tx = tx.clone();
    let watched_plugins_dir = plugins_dir.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<Event>| match result {
            Ok(event) => {
                if !should_consider_event_kind(event.kind) {
                    return;
                }

                if let Some(path) = event
                    .paths
                    .into_iter()
                    .find(|path| should_reload_for_path(&watched_plugins_dir, path))
                {
                    if let Err(error) =
                        watcher_tx.unbounded_send(PluginWatcherMessage::Changed(path))
                    {
                        warn!("plugin watcher channel closed: {error:?}");
                    }
                }
            }
            Err(error) => warn!("plugin watcher error: {error}"),
        },
        Config::default(),
    )?;
    watcher.watch(&plugins_dir, RecursiveMode::Recursive)?;

    let task = cx.spawn(async move |cx| {
        run_watcher_loop(rx, watcher, cx).await;
    });

    Ok((tx, task))
}

async fn run_watcher_loop(
    mut rx: UnboundedReceiver<PluginWatcherMessage>,
    _watcher: RecommendedWatcher,
    cx: &mut gpui::AsyncApp,
) {
    while let Some(message) = rx.next().await {
        let (mut change_count, mut last_path) = match message {
            PluginWatcherMessage::Stop => return,
            PluginWatcherMessage::HttpRefresh => {
                let refreshed = cx.update(|cx| crate::plugins::runtime::drain_http_refreshes(cx));
                match refreshed {
                    Ok(true) => {
                        if let Err(error) = cx.update(|cx| cx.refresh_windows()) {
                            warn!(
                                error = %crate::plugins::manifest::format_error_chain(&error),
                                "plugin HTTP refresh repaint failed"
                            );
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        warn!(
                            error = %crate::plugins::manifest::format_error_chain(&error),
                            "plugin HTTP refresh notification failed"
                        );
                    }
                }
                continue;
            }
            PluginWatcherMessage::Changed(path) => (1usize, path),
        };

        loop {
            Timer::after(HOT_RELOAD_DEBOUNCE).await;

            let mut saw_more_changes = false;
            loop {
                match rx.next().now_or_never() {
                    Some(Some(PluginWatcherMessage::Stop)) => return,
                    Some(Some(PluginWatcherMessage::HttpRefresh)) => {
                        let refreshed =
                            cx.update(|cx| crate::plugins::runtime::drain_http_refreshes(cx));
                        match refreshed {
                            Ok(true) => {
                                if let Err(error) = cx.update(|cx| cx.refresh_windows()) {
                                    warn!(
                                        error = %crate::plugins::manifest::format_error_chain(&error),
                                        "plugin HTTP refresh repaint failed"
                                    );
                                }
                            }
                            Ok(false) => {}
                            Err(error) => {
                                warn!(
                                    error = %crate::plugins::manifest::format_error_chain(&error),
                                    "plugin HTTP refresh notification failed"
                                );
                            }
                        }
                    }
                    Some(Some(PluginWatcherMessage::Changed(path))) => {
                        change_count += 1;
                        last_path = path;
                        saw_more_changes = true;
                    }
                    Some(None) => return,
                    None => break,
                }
            }

            if !saw_more_changes {
                break;
            }
        }

        tracing::debug!(
            path = %last_path.display(),
            count = change_count,
            "plugin watcher changes debounced"
        );
        if let Err(error) = cx.update(|cx| crate::plugins::runtime::reload_all(cx)) {
            warn!(
                error = %crate::plugins::manifest::format_error_chain(&error),
                "plugin hot reload failed"
            );
        }
    }
}

fn should_consider_event_kind(kind: EventKind) -> bool {
    kind.is_create() || kind.is_modify() || kind.is_remove()
}

pub(crate) fn should_reload_for_path(plugins_dir: &Path, path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }

    if file_name_eq(path, PLUGIN_USER_CONFIG_FILE)
        || file_name_eq(path, "config.tmp")
        || extension_eq(path, "tmp")
        || file_name_starts_with(path, ".")
    {
        return false;
    }

    let Ok(relative_path) = path.strip_prefix(plugins_dir) else {
        return false;
    };

    if relative_path.components().count() == 0 {
        return false;
    }

    if extension_eq(relative_path, PLUGIN_PACKAGE_EXTENSION) || extension_eq(relative_path, "wasm")
    {
        return true;
    }

    if file_name_eq(relative_path, PLUGIN_MANIFEST_FILE)
        || is_readme_markdown_path(relative_path)
        || ends_with_components(relative_path, &["config", "default.toml"])
        || ends_with_components(relative_path, &["config", "schema.toml"])
    {
        return true;
    }

    if extension_eq(relative_path, "lang") && has_component(relative_path, "lang") {
        return true;
    }

    has_component(relative_path, "assets")
}

fn extension_eq(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|file_name| file_name.eq_ignore_ascii_case(expected))
}

fn is_readme_markdown_path(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|file_name| {
            file_name.eq_ignore_ascii_case("README.md")
                || (file_name.starts_with("README.") && file_name.ends_with(".md"))
        })
}

fn file_name_starts_with(path: &Path, prefix: &str) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|file_name| file_name.starts_with(prefix))
}

fn has_component(path: &Path, expected: &str) -> bool {
    path.components()
        .any(|component| component_eq(component, expected))
}

fn ends_with_components(path: &Path, expected: &[&str]) -> bool {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();

    components.len() >= expected.len()
        && components
            .iter()
            .rev()
            .zip(expected.iter().rev())
            .all(|(actual, expected)| {
                actual
                    .to_str()
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
            })
}

fn component_eq(component: Component<'_>, expected: &str) -> bool {
    match component {
        Component::Normal(value) => value
            .to_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(expected)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_is_short_enough_for_interactive_reload() {
        assert!(HOT_RELOAD_DEBOUNCE <= Duration::from_millis(250));
    }

    #[test]
    fn watcher_ignores_runtime_config_writes() {
        let plugins_dir = PathBuf::from(r"C:\BMCBL\plugins");

        assert!(!should_reload_for_path(
            &plugins_dir,
            &plugins_dir.join(r"bmcbl-essentials\config.toml")
        ));
        assert!(!should_reload_for_path(
            &plugins_dir,
            &plugins_dir.join(r"bmcbl-essentials\config.tmp")
        ));
        assert!(!should_reload_for_path(
            &plugins_dir,
            &plugins_dir.join(r"bmcbl-essentials\manifest.tmp")
        ));
        assert!(!should_reload_for_path(
            &plugins_dir,
            &plugins_dir.join("bmcbl-essentials")
        ));
    }

    #[test]
    fn watcher_ignores_non_mutating_access_events() {
        assert!(!should_consider_event_kind(EventKind::Access(
            notify::event::AccessKind::Any
        )));
        assert!(!should_consider_event_kind(EventKind::Any));
        assert!(!should_consider_event_kind(EventKind::Other));
    }

    #[test]
    fn watcher_accepts_plugin_code_package_and_resource_changes() {
        let plugins_dir = PathBuf::from(r"C:\BMCBL\plugins");

        for path in [
            plugins_dir.join("bmcbl-essentials-0.1.0.bmcblx"),
            plugins_dir.join(r"bmcbl-essentials\plugin.toml"),
            plugins_dir.join(r"bmcbl-essentials\plugin.wasm"),
            plugins_dir.join(r"bmcbl-essentials\README.md"),
            plugins_dir.join(r"bmcbl-essentials\README.zh-CN.md"),
            plugins_dir.join(r"bmcbl-essentials\lang\zh-CN.lang"),
            plugins_dir.join(r"bmcbl-essentials\config\default.toml"),
            plugins_dir.join(r"bmcbl-essentials\config\schema.toml"),
            plugins_dir.join(r"bmcbl-essentials\assets\icon.png"),
        ] {
            assert!(
                should_reload_for_path(&plugins_dir, &path),
                "{} should trigger reload",
                path.display()
            );
        }
    }
}
