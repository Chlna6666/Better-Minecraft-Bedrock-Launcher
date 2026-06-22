use super::{
    TASKS_PAGE_POLL_INTERVAL_MS, TaskConfirmAction, TaskConfirmDialog, TasksPageView,
    build_render_model, is_entity_released_error,
};
use crate::tasks::task_manager;
use crate::ui::components::toast;
use gpui::*;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

impl TasksPageView {
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }

        self.active = active;
        if active {
            self.apply_render_model(build_render_model(), cx);
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
            toast::error(cx, SharedString::from("当前任务状态不支持暂停或继续"));
            return;
        }
    }

    pub(crate) fn prompt_cancel_task(&mut self, task_id: Arc<str>, cx: &mut Context<Self>) {
        let subject = task_manager::get_snapshot_arc(task_id.as_ref())
            .map(|snapshot| super::task_subject(&snapshot))
            .unwrap_or_else(|| "该任务".to_string());
        self.open_confirm(
            task_id,
            "取消任务",
            format!("确定要取消 {} 吗？已产生的临时文件可能会被清理。", subject),
            TaskConfirmAction::CancelTask,
            cx,
        );
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            _subscriptions: Vec::new(),
            confirm_dialog: None,
            update_apply_task: None,
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
        this.apply_render_model(build_render_model(), cx);

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

        let update_apply_task = cx.spawn(async move |handle, cx| -> anyhow::Result<()> {
            loop {
                Timer::after(Duration::from_millis(TASKS_PAGE_POLL_INTERVAL_MS)).await;

                let next_render_model = build_render_model();
                let update_result = handle.update(cx, move |this, cx| {
                    this.apply_render_model(next_render_model, cx);
                });
                match update_result {
                    Ok(()) => {}
                    Err(error) if is_entity_released_error(&error) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        });

        this.update_apply_task = Some(update_apply_task);
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
                let task_id_for_cancel = task_id.clone();
                let _ = thread::Builder::new()
                    .name("task-cancel".to_string())
                    .spawn(move || {
                        task_manager::cancel_task(task_id_for_cancel.as_ref());
                    });
                self.close_confirm(cx);
            }
            TaskConfirmAction::RemoveTask => {
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
                    let model = super::build_task_card_model(snapshot.as_ref());
                    self.schedule_exit_motion(task_id.clone(), model, cx);
                }
                let removed = task_manager::remove_task(task_id.as_ref());
                self.close_confirm(cx);
                if removed {
                    self.apply_render_model(build_render_model(), cx);
                }
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
                            toast::push_async(
                                cx,
                                toast::ToastKind::Success,
                                SharedString::from("已删除本地下载文件"),
                            );
                        }
                        Err(error) => {
                            toast::push_async(
                                cx,
                                toast::ToastKind::Error,
                                SharedString::from(format!("删除下载文件失败: {error}")),
                            );
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
