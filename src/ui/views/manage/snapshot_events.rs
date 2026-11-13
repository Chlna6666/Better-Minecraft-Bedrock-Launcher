use super::*;

impl ManagePageView {
    pub(super) fn observe_snapshots(cx: &mut Context<Self>) -> Task<()> {
        let mut changes = snapshots::subscribe();
        cx.spawn(async move |handle, cx| {
            while changes.changed().await.is_ok() {
                let debounce = snapshots::Debounce::new();
                loop {
                    let delay = debounce.delay();
                    if delay.is_zero() {
                        break;
                    }
                    cx.background_executor().timer(delay).await;
                }
                changes.borrow_and_update();
                if handle
                    .update(cx, |this, cx| {
                        let state = cx.global::<ManagePageState>();
                        let Some(observation) = this.snapshot_observation.as_ref() else {
                            this.snapshot_refresh_pending = true;
                            return;
                        };
                        if !observation.has_changes() {
                            return;
                        }
                        let visible = observation
                            .preview(state.tab, state.pack_subtype)
                            .has_visible_changes(state.tab);
                        this.snapshot_refresh_pending = true;
                        if this.active && visible {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    pub(super) fn refresh_changed_snapshots(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(observation) = &mut self.snapshot_observation else {
            return;
        };
        let refresh = observation.take(state.tab, state.pack_subtype);
        if refresh.config {
            self.invalidate_version_dependent_data(cx);
            return;
        }
        if refresh.assets {
            self.last_assets_signature = None;
        }
        if refresh.screenshots {
            self.last_screenshots_signature = None;
        }
        if refresh.servers {
            self.last_servers_signature = None;
        }
        if refresh.users {
            self.last_gdk_users_signature = None;
        }
        if !refresh.assets && !refresh.screenshots && !refresh.servers && !refresh.users {
            return;
        }
        cx.update_global(|state: &mut ManagePageState, _cx| {
            if refresh.assets {
                state.assets_loaded = false;
                state.assets_error = None;
            }
            if refresh.screenshots {
                state.screenshots_loaded = false;
                state.screenshots_error = None;
            }
            if refresh.servers {
                state.servers_loaded = false;
                state.servers_error = None;
            }
            if refresh.users {
                state.gdk_users_error = None;
            }
        });
    }
}
