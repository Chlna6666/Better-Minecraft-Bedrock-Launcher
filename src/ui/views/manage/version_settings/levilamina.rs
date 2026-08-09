use super::VersionSettingsModalState;
use crate::core::levilamina::{LeviLaminaInstallRequest, LeviLaminaInstallStage};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::icon::themed_icon;
use crate::ui::components::toast;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::manage::ManagePageView;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::path::PathBuf;
use tracing::warn;

pub(super) fn render_card(
    state: &VersionSettingsModalState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let installed_version = state.levilamina_installation.loader_version.clone();
    let status = if state.levilamina_loading {
        SharedString::from("正在检查支持状态...")
    } else if let Some(version) = &installed_version {
        let preloader = state
            .levilamina_installation
            .preloader_version
            .as_deref()
            .unwrap_or("未检测到");
        let runtime = if state.levilamina_installation.has_runtime_data {
            "运行时数据已就绪"
        } else {
            "缺少运行时数据"
        };
        SharedString::from(format!(
            "Loader {version} · PreLoader {preloader} · {runtime}"
        ))
    } else if state.levilamina_versions.is_empty() {
        SharedString::from("当前游戏版本不受 LeviLamina 支持")
    } else {
        SharedString::from("尚未安装")
    };

    let version_options = state
        .levilamina_versions
        .iter()
        .cloned()
        .map(DropdownOption::from)
        .collect::<Vec<_>>();
    let selected_index = state
        .levilamina_versions
        .iter()
        .position(|version| version == &state.selected_levilamina_version)
        .unwrap_or(0);
    let select_handle = view_handle.clone();
    let version_select = Dropdown::new(
        "manage-levilamina-version",
        colors,
        px(180.),
        if state.selected_levilamina_version.is_empty() {
            SharedString::from("无可用版本")
        } else {
            state.selected_levilamina_version.clone()
        },
        version_options,
        selected_index,
        !state.levilamina_loading
            && !state.levilamina_busy
            && !state.levilamina_versions.is_empty(),
        move |index, _window, cx| {
            if let Err(error) = select_handle.update(cx, |this, cx| {
                this.select_levilamina_version(index, cx);
            }) {
                warn!(%error, "LeviLamina 版本选择目标已释放");
            }
        },
    )
    .with_height(px(34.))
    .rounded(px(crate::ui::theme::tokens::radius::SM));

    let install_handle = view_handle.clone();
    let uninstall_handle = view_handle;
    div()
        .w_full()
        .p(px(14.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.settings_card_bg)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(38.))
                                .h(px(38.))
                                .rounded(px(crate::ui::theme::tokens::radius::SM))
                                .bg(Hsla {
                                    a: 0.11,
                                    ..colors.accent
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(themed_icon(
                                    lucide_icons::icon_layers(),
                                    19.0,
                                    colors.accent,
                                )),
                        )
                        .child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child("LeviLamina"),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(11.))
                                        .text_color(colors.text_secondary)
                                        .child(status),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(version_select)
                        .when(installed_version.is_some(), |this| {
                            this.child(
                                action_button(colors, "manage-levilamina-remove", "删除", false)
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        if let Err(error) =
                                            uninstall_handle.update(cx, |this, cx| {
                                                this.uninstall_levilamina(cx);
                                            })
                                        {
                                            warn!(%error, "LeviLamina 删除目标已释放");
                                        }
                                    }),
                            )
                        })
                        .child(
                            action_button(
                                colors,
                                "manage-levilamina-install",
                                if state.levilamina_busy {
                                    "处理中..."
                                } else if installed_version.is_some() {
                                    "更新"
                                } else {
                                    "安装"
                                },
                                true,
                            )
                            .opacity(
                                if state.levilamina_busy
                                    || state.levilamina_loading
                                    || state.levilamina_versions.is_empty()
                                {
                                    0.5
                                } else {
                                    1.0
                                },
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_, _, cx| {
                                    if let Err(error) = install_handle.update(cx, |this, cx| {
                                        this.install_levilamina(cx);
                                    }) {
                                        warn!(%error, "LeviLamina 安装目标已释放");
                                    }
                                },
                            ),
                        ),
                ),
        )
        .children(state.levilamina_error.clone().map(|error| {
            div()
                .pl(px(48.))
                .text_size(px(11.))
                .text_color(colors.danger)
                .child(error)
                .into_any_element()
        }))
        .into_any_element()
}

fn action_button(
    colors: &ThemeColors,
    id: &'static str,
    label: &'static str,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(8.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(if primary {
            colors.accent
        } else {
            colors.surface
        })
        .border_1()
        .border_color(if primary {
            colors.accent
        } else {
            colors.border
        })
        .cursor_pointer()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if primary {
                    colors.btn_primary_text
                } else {
                    colors.danger
                })
                .child(label),
        )
}

impl ManagePageView {
    pub(in crate::ui::views::manage) fn load_levilamina_settings(
        &mut self,
        version: crate::ui::views::manage::state::ManagedVersionEntry,
        cx: &mut Context<Self>,
    ) {
        let game_directory = PathBuf::from(version.path.as_ref());
        let game_version = version.version.to_string();
        cx.spawn(async move |handle, cx| {
            let task = gpui_tokio::Tokio::spawn_result(cx, async move {
                let database = crate::core::levilamina::cached_support_database()
                    .await
                    .map_err(anyhow::Error::msg)?;
                let installation = crate::core::levilamina::inspect_installation(game_directory)
                    .await
                    .map_err(anyhow::Error::msg)?;
                Ok::<_, anyhow::Error>((database.loader_versions(&game_version), installation))
            });
            let result = task.await;
            if let Err(error) = handle.update(cx, |this, cx| {
                if let Some(state) = this.version_settings_modal.as_mut() {
                    state.levilamina_loading = false;
                    match result {
                        Ok((versions, installation)) => {
                            state.levilamina_versions =
                                versions.into_iter().map(SharedString::from).collect();
                            let installed_version = installation.loader_version.clone();
                            state.selected_levilamina_version = state
                                .levilamina_versions
                                .iter()
                                .find(|version| {
                                    installed_version.as_deref() == Some(version.as_ref())
                                })
                                .cloned()
                                .and_then(|installed| (!installed.is_empty()).then_some(installed))
                                .or_else(|| state.levilamina_versions.first().cloned())
                                .unwrap_or_else(|| SharedString::from(""));
                            state.levilamina_installation = installation;
                            state.levilamina_error = None;
                        }
                        Err(error) => {
                            state.levilamina_error = Some(SharedString::from(error.to_string()));
                        }
                    }
                }
                cx.notify();
            }) {
                warn!(%error, "LeviLamina 状态加载目标已释放");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn select_levilamina_version(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(state) = self.version_settings_modal.as_mut()
            && let Some(version) = state.levilamina_versions.get(index)
        {
            state.selected_levilamina_version = version.clone();
            state.levilamina_error = None;
            cx.notify();
        }
    }

    pub(super) fn install_levilamina(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.version_settings_modal.as_mut() else {
            return;
        };
        if state.levilamina_busy
            || state.levilamina_loading
            || state.selected_levilamina_version.is_empty()
        {
            return;
        }
        state.levilamina_busy = true;
        state.levilamina_error = None;
        let request = LeviLaminaInstallRequest::Loader {
            game_directory: PathBuf::from(state.version.path.as_ref()),
            game_version: state.version.version.to_string(),
            loader_version: state.selected_levilamina_version.to_string(),
        };
        self.start_levilamina_operation(request, cx);
    }

    pub(super) fn uninstall_levilamina(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.version_settings_modal.as_mut() else {
            return;
        };
        if state.levilamina_busy || state.levilamina_installation.loader_version.is_none() {
            return;
        }
        state.levilamina_busy = true;
        state.levilamina_error = None;
        let game_directory = PathBuf::from(state.version.path.as_ref());
        let handle = crate::core::levilamina::start_uninstall(game_directory);
        self.consume_levilamina_operation(handle, cx);
    }

    fn start_levilamina_operation(
        &mut self,
        request: LeviLaminaInstallRequest,
        cx: &mut Context<Self>,
    ) {
        let handle = crate::core::levilamina::start_install(request);
        self.consume_levilamina_operation(handle, cx);
    }

    fn consume_levilamina_operation(
        &mut self,
        handle: Result<crate::core::levilamina::LeviLaminaInstallHandle, String>,
        cx: &mut Context<Self>,
    ) {
        let mut updates = match handle {
            Ok(handle) => handle.updates,
            Err(error) => {
                if let Some(state) = self.version_settings_modal.as_mut() {
                    state.levilamina_busy = false;
                    state.levilamina_error = Some(SharedString::from(error));
                }
                cx.notify();
                return;
            }
        };
        cx.spawn(async move |handle, cx| {
            loop {
                let stage = updates.borrow_and_update().stage.clone();
                match stage {
                    LeviLaminaInstallStage::Completed { message } => {
                        if let Err(error) = handle.update(cx, |this, cx| {
                            let version = this
                                .version_settings_modal
                                .as_ref()
                                .map(|state| state.version.clone());
                            if let Some(state) = this.version_settings_modal.as_mut() {
                                state.levilamina_busy = false;
                            }
                            this.invalidate_version_dependent_data(cx);
                            crate::ui::hooks::use_local_versions::ensure_local_versions_loaded(
                                true, cx,
                            );
                            toast::success(cx, SharedString::from(message.to_string()));
                            if let Some(version) = version {
                                if let Some(state) = this.version_settings_modal.as_mut() {
                                    state.levilamina_loading = true;
                                }
                                this.load_levilamina_settings(version, cx);
                            }
                            cx.notify();
                        }) {
                            warn!(%error, "LeviLamina 完成状态目标已释放");
                        }
                        return Ok::<(), anyhow::Error>(());
                    }
                    LeviLaminaInstallStage::Failed { message } => {
                        if let Err(error) = handle.update(cx, |this, cx| {
                            if let Some(state) = this.version_settings_modal.as_mut() {
                                state.levilamina_busy = false;
                                state.levilamina_error =
                                    Some(SharedString::from(message.to_string()));
                            }
                            cx.notify();
                        }) {
                            warn!(%error, "LeviLamina 失败状态目标已释放");
                        }
                        return Ok(());
                    }
                    _ => {}
                }
                if updates.changed().await.is_err() {
                    return Ok(());
                }
            }
        })
        .detach();
    }
}
