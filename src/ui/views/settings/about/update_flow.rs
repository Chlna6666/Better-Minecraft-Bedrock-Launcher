use crate::ui::components::toast::{self, ToastKind};
use crate::ui::state::update::UpdateState;
use crate::ui::update_check::UpdateCheckGate;
use gpui::*;
use std::time::Instant;

static MANUAL_UPDATE_CHECK_GATE: UpdateCheckGate = UpdateCheckGate::new();

pub(super) fn spawn_manual_update_check(no_update_msg: SharedString, cx: &mut App) {
    tracing::info!("manual update check requested");

    if !MANUAL_UPDATE_CHECK_GATE.try_begin() {
        tracing::info!("manual update check ignored: worker already running");
        let _ = cx.update_global(|update: &mut UpdateState, _cx| {
            update.check_started = true;
            update.checking = true;
        });
        return;
    }

    let _ = cx.update_global(|update: &mut UpdateState, _cx| {
        update.check_started = true;
        update.checking = true;
        update.last_error = None;
    });

    let result_receiver = crate::ui::update_check::spawn_update_check_thread("manual");
    cx.spawn(async move |cx| {
        tracing::info!("manual update check awaiting worker result");

        let outcome = result_receiver.await.unwrap_or_else(|error| {
            crate::ui::update_check::UpdateCheckOutcome::with_error(format!(
                "更新检查线程提前结束: {error:?}"
            ))
        });
        MANUAL_UPDATE_CHECK_GATE.finish();

        tracing::info!(
            available = outcome.available.is_some(),
            has_error = outcome.error.is_some(),
            "manual update check worker result received"
        );

        let has_update = outcome.available.is_some();
        let error = outcome.error.clone();
        let now = Instant::now();
        tracing::info!("manual update check applying UI result");
        let apply_result = cx.update_global(|update: &mut UpdateState, _cx| {
            update.check_started = true;
            update.checking = false;
            update.available = outcome.available;
            update.last_error = error.clone();
            if has_update {
                update.request_open_modal(now);
            }
            tracing::debug!(
                "manual update check applied checking={} has_update={} modal_pending={} show_modal={} cache_inflight={} cached_tag={:?}",
                update.checking,
                has_update,
                update.modal_pending_open,
                update.show_modal,
                update.markdown_cache_refresh_inflight,
                update.cached_release_tag,
            );
        });
        if let Err(error) = apply_result {
            tracing::warn!("manual update check failed to apply UI result: {error:?}");
            return;
        }
        tracing::info!("manual update check UI result applied");

        if let Some(error) = error {
            toast::push_async(
                cx,
                ToastKind::Error,
                SharedString::from(format!("检查更新失败: {error}")),
            );
        } else if !has_update {
            toast::push_async(cx, ToastKind::Info, no_update_msg);
        }
    })
    .detach();
}
