use super::{TaskConfirmAction, TaskConfirmDialog, TasksPageView};
use crate::tasks::task_manager;
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use gpui::*;
use std::path::Path;
use std::sync::Arc;

impl TasksPageView {
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }

        self.active = active;
        if active {
            self.apply_render_model(super::build_render_model(), cx);
        }
    }

    pub(crate) fn toggle_pause_task(&mut self, task_id: Arc<str>, cx: &mut Context<Self>) {
        let snapshot = task_manager::get_snapshot_arc(task_id.as_ref());
        let success = match snapshot.as_ref().map(|snapshot| snapshot.status.as_ref()) {
            Some("paused") => task_manager::resume_task(task_id.as_ref()),
            Some("running") => task_manager::pause_task(task_id.as_ref()),
            _ => false,
        };

        if !success {
            let message = cx
                .global::<I18n>()
                .t_key(crate::i18n_key!("Tasks.pause_unavailable"));
            toast::error(cx, message);
            return;
        }
    }

    pub(crate) fn prompt_cancel_task(&mut self, task_id: Arc<str>, cx: &mut Context<Self>) {
        let subject = task_manager::get_snapshot_arc(task_id.as_ref())
            .map(|snapshot| super::task_subject(&snapshot))
            .unwrap_or_else(|| {
                cx.global::<I18n>()
                    .t_key(crate::i18n_key!("Tasks.this_task"))
                    .to_string()
            });
        let i18n = cx.global::<I18n>();
        self.open_confirm(
            task_id,
            t!("Tasks.cancel_title"),
            t!("Tasks.cancel_description", subject = subject),
            TaskConfirmAction::CancelTask,
            cx,
        );
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let task_events = task_manager::task_event_stream();
        let mut this = Self {
            _subscriptions: Vec::new(),
            confirm_dialog: None,
            task_snapshots: task_manager::snapshot_arcs_map(),
            render_model: super::TasksPageRenderModel::loading(),
            card_motions: Default::default(),
            finished_hold_until: Default::default(),
            hidden_finished_ids: Default::default(),
            user_cancelled_ids: Default::default(),
            pending_exit_motions: Default::default(),
            transition_cards: Default::default(),
            motion_sequence: 0,
            active: true,
        };
        this.apply_render_model(super::build_render_model(), cx);

        this._subscriptions
            .push(
                cx.observe_global::<crate::ui::state::theme::ThemeState>(|_, cx| {
                    cx.notify();
                }),
            );
        this._subscriptions.push(
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(|_, cx| {
                cx.notify();
            }),
        );

        let stream_task = cx.spawn_stream(task_events, |this, delivery, cx| {
            match delivery {
                task_manager::TaskEventDelivery::Event(task_manager::TaskEvent::Updated(
                    snapshot,
                )) => {
                    this.apply_task_snapshot(snapshot);
                }
                task_manager::TaskEventDelivery::Event(task_manager::TaskEvent::Removed(
                    task_id,
                )) => {
                    this.remove_task_snapshot(task_id.as_ref());
                }
                task_manager::TaskEventDelivery::Batch(events) => {
                    for event in events {
                        match event {
                            task_manager::TaskEvent::Updated(snapshot) => {
                                this.apply_task_snapshot(snapshot);
                            }
                            task_manager::TaskEvent::Removed(task_id) => {
                                this.remove_task_snapshot(task_id.as_ref());
                            }
                        }
                    }
                }
                task_manager::TaskEventDelivery::ResyncRequired => {
                    this.replace_task_snapshots_from_manager();
                }
            }

            if this.active {
                this.apply_render_model(super::build_render_model(), cx);
            }
        });
        this._subscriptions.push(Subscription::new(move || {
            drop(stream_task);
        }));

        this
    }

    pub(crate) fn open_confirm(
        &mut self,
        task_id: Arc<str>,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        action: TaskConfirmAction,
        cx: &mut Context<Self>,
    ) {
        self.confirm_dialog = Some(TaskConfirmDialog {
            task_id,
            title: title.into(),
            description: description.into(),
            action,
        });
        cx.notify();
    }

    pub(crate) fn close_confirm(&mut self, cx: &mut Context<Self>) {
        self.confirm_dialog = None;
        cx.notify();
    }

    pub(crate) fn perform_confirm_action(
        &mut self,
        task_id: Arc<str>,
        action: TaskConfirmAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            TaskConfirmAction::CancelTask => {
                self.mark_user_cancelled(task_id.clone());
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
                    let model = super::build_task_card_model(snapshot.as_ref());
                    self.schedule_exit_motion(task_id.clone(), model, cx);
                }
                task_manager::cancel_task(task_id.as_ref());
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
                    self.apply_task_snapshot(snapshot);
                }
                self.apply_render_model(self.local_render_model(), cx);
                self.close_confirm(cx);
            }
            TaskConfirmAction::RemoveTask => {
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
                    let model = super::build_task_card_model(snapshot.as_ref());
                    self.schedule_exit_motion(task_id.clone(), model, cx);
                }
                task_manager::remove_task(task_id.as_ref());
                self.remove_task_snapshot(task_id.as_ref());
                self.apply_render_model(self.local_render_model(), cx);
                self.close_confirm(cx);
            }
            TaskConfirmAction::DeleteDownloadFile => {
                let path = task_manager::get_snapshot_arc(task_id.as_ref())
                    .and_then(|snapshot| snapshot.message.clone());
                self.close_confirm(cx);

                let Some(path) = path else {
                    return;
                };
                let Some(file_name) = Path::new(path.as_ref())
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(ToOwned::to_owned)
                else {
                    return;
                };

                cx.spawn(async move |_handle, cx| {
                    match crate::downloads::api::delete_local_download(file_name).await {
                        Ok(()) => {
                            let message = cx
                                .read_global(|i18n: &I18n, _cx| {
                                    i18n.t_key(crate::i18n_key!("Tasks.download_deleted"))
                                })
                                .unwrap_or_else(|_| SharedString::from("Local download deleted"));
                            toast::push_async(cx, toast::ToastKind::Success, message);
                        }
                        Err(error) => {
                            let message = cx
                                .read_global(|i18n: &I18n, _cx| {
                                    t!("Tasks.download_delete_failed", error = error)
                                })
                                .unwrap_or_else(|_| {
                                    SharedString::from("Failed to delete download")
                                });
                            toast::push_async(cx, toast::ToastKind::Error, message);
                            return Err(anyhow::Error::msg(error));
                        }
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .detach_and_log_err(cx);
            }
        }
    }
}
