use crate::utils::file_ops;
use anyhow::{Context, Result};
use futures::{Stream, channel::mpsc::unbounded};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;

const WATCHED_EXTENSIONS: &[&str] = &["m4a", "mp3", "wav", "flac", "ogg", "aac", "ncm"];

pub(crate) fn library_changes() -> Result<impl Stream<Item = ()>> {
    let music_directory = file_ops::bmcbl_subdir("music");
    std::fs::create_dir_all(&music_directory)
        .with_context(|| format!("create music directory {}", music_directory.display()))?;

    let (sender, receiver) = unbounded();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<Event>| match result {
            Ok(event)
                if is_relevant_event(event.kind)
                    && event.paths.iter().any(|path| is_music_path(path)) =>
            {
                if let Err(error) = sender.unbounded_send(()) {
                    tracing::debug!(%error, "music: watcher receiver closed");
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "music: directory watcher error"),
        },
        Config::default(),
    )?;
    watcher
        .watch(&music_directory, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch music directory {}", music_directory.display()))?;

    Ok(futures::stream::unfold(
        (receiver, watcher),
        |(mut receiver, watcher)| async move {
            use futures::StreamExt as _;
            receiver
                .next()
                .await
                .map(|event| (event, (receiver, watcher)))
        },
    ))
}

fn is_relevant_event(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

fn is_music_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            WATCHED_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}
