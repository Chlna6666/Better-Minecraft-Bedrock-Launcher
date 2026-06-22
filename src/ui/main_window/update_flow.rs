use super::*;
use crate::tasks::task_manager::TaskSnapshot;
use crate::ui::update_check::UpdateCheckGate;

static STARTUP_UPDATE_CHECK_GATE: UpdateCheckGate = UpdateCheckGate::new();

fn download_snapshot_meaningfully_changed(
    previous: Option<&Arc<TaskSnapshot>>,
    next: &TaskSnapshot,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };

    previous.sequence != next.sequence
        || previous.status != next.status
        || previous.stage != next.stage
        || previous.done != next.done
        || previous.total != next.total
        || previous.percent != next.percent
        || previous.eta != next.eta
        || previous.speed_bytes_per_sec != next.speed_bytes_per_sec
        || previous.cancel_requested != next.cancel_requested
        || previous.message != next.message
}

impl MainWindowView {
    fn spawn_update_markdown_cache(release: ReleaseSummary, now: Instant, cx: &mut Context<Self>) {
        let release_tag = release.tag.clone();
        let release_body = release.body.clone().unwrap_or_default();

        cx.spawn(async move |handle, cx| {
            tracing::debug!("update markdown cache task started release_tag={release_tag}");
            let parsed = tokio::task::spawn_blocking(move || {
                crate::ui::components::markdown_renderer::warm_highlighter_assets();
                crate::ui::components::markdown_renderer::parse_markdown_document(&release_body)
            })
            .await;

            let _ = cx.update_global(|update_state: &mut UpdateState, _cx: &mut App| {
                update_state.markdown_cache_refresh_inflight = false;

                match parsed {
                    Ok(document) => {
                        tracing::debug!(
                            "update markdown cache task finished release_tag={} blocks={}",
                            release_tag,
                            document.blocks.len()
                        );
                        update_state.cached_release_tag = Some(release_tag.clone());
                        update_state.cached_md_document = document.into();

                        if update_state.modal_pending_open {
                            tracing::debug!(
                                "update markdown cache ready for pending modal release_tag={release_tag}"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::warn!("update markdown parse task failed: {error:?}");
                        update_state.modal_pending_open = false;
                        update_state.show_modal = false;
                        update_state.last_error = Some(format!("解析更新日志失败: {error:?}"));
                    }
                }
            });

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn spawn_startup_update_check(
        startup_check_updates_enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !startup_check_updates_enabled {
            return;
        }

        tracing::info!("startup update check requested");
        if !STARTUP_UPDATE_CHECK_GATE.try_begin() {
            tracing::info!("startup update check ignored: worker already running");
            return;
        }

        let _ = cx.update_global(|update_state: &mut UpdateState, _cx| {
            update_state.check_started = true;
            update_state.checking = true;
            update_state.last_error = None;
        });

        let result_receiver = crate::ui::update_check::spawn_update_check_thread("startup");
        cx.spawn(async move |handle, cx| {
            tracing::info!("startup update check awaiting worker result");
            let outcome = result_receiver.await.unwrap_or_else(|error| {
                crate::ui::update_check::UpdateCheckOutcome::with_error(format!(
                    "更新检查线程提前结束: {error:?}"
                ))
            });
            STARTUP_UPDATE_CHECK_GATE.finish();

            tracing::info!(
                available = outcome.available.is_some(),
                has_error = outcome.error.is_some(),
                "startup update check worker result received"
            );

            let has_update = outcome.available.is_some();
            let error = outcome.error.clone();
            let now = Instant::now();
            tracing::info!("startup update check applying UI result");
            match cx.update_global(|update_state: &mut UpdateState, _cx| {
                update_state.check_started = true;
                update_state.checking = false;
                update_state.available = outcome.available;
                update_state.last_error = error;
                if has_update {
                    update_state.request_open_modal(now);
                }
            }) {
                Ok(()) => tracing::info!("startup update check UI result applied"),
                Err(error) => {
                    tracing::warn!("startup update check failed to apply UI result: {error:?}");
                }
            }
            if let Err(error) = handle.update(cx, |_this, cx| {
                cx.notify();
            }) {
                tracing::warn!("startup update check failed to notify main window: {error:?}");
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn sync_update_state(&mut self, now: Instant, cx: &mut Context<Self>) -> bool {
        let (modal_pending_open, needs_markdown_cache, cache_inflight, release, should_hide_modal) = {
            let update_state = cx.global::<UpdateState>();
            let needs_markdown_cache = update_state.needs_md_cache_refresh();
            let should_hide_modal = update_state.show_modal
                && !update_state.modal_visible
                && !update_state.is_modal_animating(now);
            (
                update_state.modal_pending_open,
                needs_markdown_cache,
                update_state.markdown_cache_refresh_inflight,
                update_state.available.clone(),
                should_hide_modal,
            )
        };

        if !modal_pending_open && !should_hide_modal {
            return false;
        }

        let mut changed = false;
        if modal_pending_open
            && needs_markdown_cache
            && !cache_inflight
            && let Some(release) = release
        {
            let scheduled = cx.update_global(|update_state: &mut UpdateState, _cx| {
                if update_state.markdown_cache_refresh_inflight
                    || !update_state.needs_md_cache_refresh()
                {
                    return false;
                }
                tracing::info!(
                    release_tag = %release.tag,
                    "update markdown cache requested before opening modal"
                );
                update_state.markdown_cache_refresh_inflight = true;
                true
            });
            if scheduled {
                tracing::debug!(
                    "update markdown cache scheduled release_tag={}",
                    release.tag
                );
                Self::spawn_update_markdown_cache(release, now, cx);
                changed = true;
            }
        }

        let can_open_pending_modal = modal_pending_open && !cache_inflight && !needs_markdown_cache;
        if !can_open_pending_modal && !should_hide_modal {
            return changed;
        }

        changed |= cx.update_global(|update_state: &mut UpdateState, _cx| {
            let mut changed = false;
            if modal_pending_open
                && update_state.modal_pending_open
                && !update_state.markdown_cache_refresh_inflight
                && !update_state.needs_md_cache_refresh()
            {
                update_state.modal_pending_open = false;
                update_state.set_show_modal(true, now);
                tracing::debug!(
                    modal_visible = update_state.modal_visible,
                    modal_animating = update_state.is_modal_render_animating(now),
                    "update modal opened from cached markdown"
                );
                changed = true;
            }

            if should_hide_modal && update_state.finish_close_if_elapsed(now) {
                tracing::debug!("update modal close animation finished");
                changed = true;
            }

            changed
        });

        changed
    }

    pub(super) fn ensure_update_download_listener(&mut self, cx: &mut Context<Self>) -> bool {
        if self.update_download_listener_running {
            return false;
        }

        let should_start_listener = {
            let update_state = cx.global::<UpdateState>();
            update_state.downloading
                && update_state.task_id.is_some()
                && update_state.task_updates.is_some()
        };
        if !should_start_listener {
            return false;
        }

        let (task_id, mut rx) = match cx.update_global(|update_state: &mut UpdateState, _cx| {
            if !update_state.downloading {
                return None;
            }

            let task_id = update_state.task_id.clone()?;
            let rx = update_state.task_updates.take()?;
            Some((task_id, rx))
        }) {
            Some(value) => value,
            None => return false,
        };

        tracing::debug!(task_id = %task_id, "update download listener started");
        self.update_download_listener_running = true;
        cx.spawn(async move |handle, cx| {
            loop {
                match rx.recv().await {
                    Ok(snapshot) => {
                        if snapshot.id.as_ref() != task_id {
                            continue;
                        }

                        let task_id = task_id.clone();
                        let update_result = handle.update(cx, |this, cx| {
                            let mut still_downloading = true;
                            let mut should_notify = false;
                            cx.update_global(|update_state: &mut UpdateState, _cx| {
                                if snapshot.id.as_ref() != task_id {
                                    return;
                                }

                                let previous_snapshot = update_state.last_task_snapshot.as_ref();
                                if !download_snapshot_meaningfully_changed(
                                    previous_snapshot,
                                    snapshot.as_ref(),
                                ) {
                                    return;
                                }

                                if snapshot.status.as_ref() == "error" {
                                    update_state.fail_download(
                                        snapshot
                                            .message
                                            .clone()
                                            .map(|message| message.to_string())
                                            .unwrap_or_else(|| "下载失败".to_string()),
                                    );
                                } else if snapshot.status.as_ref() == "cancelled" {
                                    update_state.cancel_download();
                                } else if snapshot.status.as_ref() == "completed" {
                                    update_state.finish_download();
                                } else {
                                    update_state.last_task_snapshot = Some(snapshot.clone());
                                }

                                still_downloading = update_state.downloading;
                                should_notify = true;
                            });

                            if !still_downloading {
                                this.update_download_listener_running = false;
                            }

                            if should_notify {
                                cx.notify();
                            }

                            still_downloading
                        });

                        match update_result {
                            Ok(true) => {}
                            Ok(false) => return Ok::<(), anyhow::Error>(()),
                            Err(error) => {
                                let _ = handle.update(cx, |this, _cx| {
                                    this.update_download_listener_running = false;
                                });
                                return Err(error);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let _ = handle.update(cx, |this, _cx| {
                            this.update_download_listener_running = false;
                        });
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            }
        })
        .detach_and_log_err(cx);

        true
    }

    pub(super) fn read_update_render_state(
        &self,
        now: Instant,
        debug_enabled: bool,
        cx: &App,
    ) -> UpdateRenderState {
        let suppress_background_animation_frames = {
            let update_state: &UpdateState = cx.global::<UpdateState>();
            update_state.show_modal
                && !debug_enabled
                && (update_state.modal_pending_open
                    || update_state.modal_visible
                    || update_state.is_modal_render_animating(now))
        };

        UpdateRenderState {
            suppress_background_animation_frames,
        }
    }
}
