#![cfg(target_os = "windows")]

use gpui::prelude::FluentBuilder as _;
use gpui::{AppContext as _, BorrowAppContext as _, *};
use lucide_gpui::icons as lucide_icons;

use crate::core::minecraft::uwp_registration::{MinecraftUwpChannel, SystemUwpRegistration};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::launch_prereq::PendingLaunchVersion;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::download::state::{DownloadPageState, GameDialogState};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UwpSafetyGuideTrigger {
    #[default]
    DownloadRelease,
    DownloadPreview,
    Import,
    Launch,
}

pub struct UwpSafetyGuideState {
    pub visible: bool,
    pub checking: bool,
    pub trigger: Option<UwpSafetyGuideTrigger>,
    pub system_registration: Option<SystemUwpRegistration>,
    request_id: u64,
    pending: bool,
    active_download_package: Option<SharedString>,
    suspended_download_dialog: Option<GameDialogState>,
    pending_launch: Option<PendingLaunchVersion>,
}

impl Global for UwpSafetyGuideState {}

impl Default for UwpSafetyGuideState {
    fn default() -> Self {
        Self {
            visible: false,
            checking: false,
            trigger: None,
            system_registration: None,
            request_id: 0,
            pending: false,
            active_download_package: None,
            suspended_download_dialog: None,
            pending_launch: None,
        }
    }
}

impl UwpSafetyGuideState {
    fn begin_download(
        &mut self,
        package_id: SharedString,
        version_type: i32,
    ) -> Option<(u64, UwpSafetyGuideTrigger)> {
        if self.active_download_package.as_ref() == Some(&package_id) {
            return None;
        }

        self.active_download_package = Some(package_id);
        let trigger = if version_type == 0 {
            UwpSafetyGuideTrigger::DownloadRelease
        } else {
            UwpSafetyGuideTrigger::DownloadPreview
        };
        Some(self.begin_check(trigger))
    }

    fn begin_import(&mut self) -> (u64, UwpSafetyGuideTrigger) {
        self.begin_check(UwpSafetyGuideTrigger::Import)
    }

    fn begin_launch(&mut self, version: PendingLaunchVersion) -> (u64, UwpSafetyGuideTrigger) {
        self.pending_launch = Some(version);
        self.begin_check(UwpSafetyGuideTrigger::Launch)
    }

    fn begin_check(&mut self, trigger: UwpSafetyGuideTrigger) -> (u64, UwpSafetyGuideTrigger) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        // 检查本身也使用同一张 modal。下载确认会在检查期间被挂起，
        // 避免出现“下载确认 + UWP 安全说明”双弹窗叠层。
        self.visible = true;
        self.checking = true;
        self.pending = false;
        self.trigger = Some(trigger);
        self.system_registration = None;
        (self.request_id, trigger)
    }

    fn apply_check(
        &mut self,
        request_id: u64,
        registration: Option<SystemUwpRegistration>,
        tour_visible: bool,
    ) -> bool {
        if self.request_id != request_id {
            return false;
        }

        self.checking = false;
        self.system_registration = registration;
        let should_show = self.system_registration.is_some();
        self.pending = should_show && tour_visible;
        self.visible = should_show && !tour_visible;
        if !should_show {
            self.trigger = None;
        }

        // 未检测到系统/Store UWP 时，不需要安全说明，立即恢复原下载确认。
        !should_show
    }

    fn fail_check(&mut self, request_id: u64) -> bool {
        if self.request_id != request_id {
            return false;
        }
        self.checking = false;
        self.visible = false;
        self.pending = false;
        self.trigger = None;
        self.system_registration = None;
        true
    }

    fn activate_pending(&mut self) {
        if self.pending && self.system_registration.is_some() {
            self.pending = false;
            self.visible = true;
        }
    }

    fn finish_visible_guide(&mut self) {
        self.visible = false;
        self.pending = false;
        self.trigger = None;
        self.system_registration = None;
        self.pending_launch = None;
    }

    pub fn has_pending_launch(&self) -> bool {
        self.pending_launch.is_some()
    }

    fn clear_download_context(&mut self) {
        if self.suspended_download_dialog.is_some() || self.checking || self.visible || self.pending
        {
            return;
        }

        if self.active_download_package.take().is_none() {
            return;
        }

        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.trigger = None;
        self.system_registration = None;
    }
}

fn suspend_download_dialog(cx: &mut App) {
    let dialog = cx
        .global::<DownloadPageState>()
        .game_dialog
        .as_ref()
        .cloned();
    let Some(dialog) = dialog else {
        return;
    };

    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        if state.suspended_download_dialog.is_none() {
            state.suspended_download_dialog = Some(dialog);
        }
    });
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.game_dialog = None;
    });
}

fn restore_download_dialog(cx: &mut App) {
    let dialog = cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.suspended_download_dialog.take()
    });
    let Some(dialog) = dialog else {
        return;
    };

    cx.update_global(|state: &mut DownloadPageState, _cx| {
        if state.game_dialog.is_none() {
            state.game_dialog = Some(dialog);
        }
    });
}

pub fn request_download(package_id: SharedString, version_type: i32, cx: &mut App) {
    cx.default_global::<UwpSafetyGuideState>();
    let request = cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.begin_download(package_id, version_type)
    });
    if let Some((request_id, trigger)) = request {
        // 先保存确认对话框，再从下载页拿掉。异步检查完成后按结果恢复或显示安全说明。
        suspend_download_dialog(cx);
        start_system_registration_check(request_id, trigger, cx);
    }
}

pub fn request_import(cx: &mut App) {
    cx.default_global::<UwpSafetyGuideState>();
    let (request_id, trigger) =
        cx.update_global(|state: &mut UwpSafetyGuideState, _cx| state.begin_import());
    start_system_registration_check(request_id, trigger, cx);
}

pub fn request_launch(version: PendingLaunchVersion, cx: &mut App) -> u64 {
    cx.default_global::<UwpSafetyGuideState>();
    let (request_id, trigger) =
        cx.update_global(|state: &mut UwpSafetyGuideState, _cx| state.begin_launch(version));
    start_system_registration_check(request_id, trigger, cx);
    request_id
}

pub fn clear_download_context(cx: &mut App) {
    let should_clear = cx.try_global::<UwpSafetyGuideState>().is_some_and(|state| {
        state.active_download_package.is_some()
            && state.suspended_download_dialog.is_none()
            && !state.checking
            && !state.visible
            && !state.pending
    });
    if should_clear {
        cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
            state.clear_download_context();
        });
    }
}

pub fn activate_pending(cx: &mut App) {
    cx.default_global::<UwpSafetyGuideState>();
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.activate_pending();
    });
}

fn start_system_registration_check(request_id: u64, trigger: UwpSafetyGuideTrigger, cx: &mut App) {
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || match trigger {
            UwpSafetyGuideTrigger::DownloadRelease => {
                crate::core::minecraft::uwp_registration::system_registration(
                    MinecraftUwpChannel::Release,
                )
            }
            UwpSafetyGuideTrigger::DownloadPreview => {
                crate::core::minecraft::uwp_registration::system_registration(
                    MinecraftUwpChannel::Preview,
                )
            }
            UwpSafetyGuideTrigger::Import => {
                crate::core::minecraft::uwp_registration::any_system_registration()
            }
            UwpSafetyGuideTrigger::Launch => {
                crate::core::minecraft::uwp_registration::any_system_registration()
            }
        })
        .await;

        cx.update(|cx| {
            let restore_dialog = match result {
                Ok(registration) => {
                    let tour_visible = cx
                        .try_global::<super::state::OnboardingTourState>()
                        .is_some_and(|tour| tour.visible);
                    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
                        state.apply_check(request_id, registration, tour_visible)
                    })
                }
                Err(error) => {
                    tracing::warn!(%error, "检测 Microsoft Store Minecraft UWP 注册失败");
                    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
                        state.fail_check(request_id)
                    })
                }
            };

            if restore_dialog {
                continue_without_guide(cx);
            }
        })?;

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn continue_without_guide(cx: &mut App) {
    let pending_launch = cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        let pending_launch = state.pending_launch.take();
        state.finish_visible_guide();
        pending_launch
    });
    if let Some(version) = pending_launch {
        crate::ui::hooks::use_launcher::continue_launcher_after_uwp_check(version, cx);
    } else {
        restore_download_dialog(cx);
    }
}

fn continue_after_confirmation(cx: &mut App) {
    let is_launch = cx
        .global::<UwpSafetyGuideState>()
        .trigger
        .is_some_and(|trigger| trigger == UwpSafetyGuideTrigger::Launch);
    if is_launch {
        continue_without_guide(cx);
        return;
    }

    // 先恢复原下载确认，再关闭安全说明。这样 DownloadPageState 的观察器
    // 不会在两个状态更新之间误判“对话框已经结束”并清除下载上下文。
    restore_download_dialog(cx);
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.finish_visible_guide();
    });
}

fn cancel_launch(cx: &mut App) {
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.finish_visible_guide();
    });
}

pub fn render_uwp_safety_guide(
    state: &UwpSafetyGuideState,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    );
    let size = window.bounds().size;
    let width = size.width / px(1.0);
    let height = size.height / px(1.0);
    let card_w = (width - 32.0).max(280.0).min(560.0);
    let desired_h = if state.checking { 250.0 } else { 500.0 };
    let card_h = (height - 32.0).max(220.0).min(desired_h);
    let trigger = state.trigger.unwrap_or_default();
    let registration = state.system_registration.as_ref();

    let launch_confirmation = trigger == UwpSafetyGuideTrigger::Launch;
    let title = if state.checking {
        "正在检查 Windows UWP 注册"
    } else if launch_confirmation {
        "启动前将自动保护当前 UWP 数据"
    } else {
        "检测到系统安装的 UWP"
    };
    let subtitle = if state.checking {
        "只读取 Windows 包注册元数据，不扫描存档或修改系统。"
    } else {
        if launch_confirmation {
            "确认后由 BMCBL 自动备份、切换注册并恢复数据，无需手动导出。"
        } else {
            "下载不会更改现有注册；首次启动下载版本时会自动保护数据。"
        }
    };

    let body = if state.checking {
        render_checking_body(&colors).into_any_element()
    } else {
        render_safety_body(trigger, registration, &colors).into_any_element()
    };

    let card = div()
        .w(px(card_w))
        .h(px(card_h))
        .max_w(relative(1.0))
        .max_h(relative(1.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .overflow_hidden()
        .occlude()
        .bg(Hsla {
            a: 0.985,
            ..colors.bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .shadow(vec![BoxShadow {
            color: Hsla { a: 0.22, ..black() },
            blur_radius: px(32.0),
            spread_radius: px(-5.0),
            offset: point(px(0.0), px(14.0)),
        }])
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .px(px(20.0))
                .pt(px(18.0))
                .pb(px(14.0))
                .border_b_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_none()
                        .size(px(42.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.13,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(if state.checking {
                                    lucide_icons::icon_search()
                                } else {
                                    lucide_icons::icon_shield_check()
                                })
                                .size(px(21.0))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(17.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(colors.text_secondary)
                                .child(subtitle),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .child(body),
        )
        .when(!state.checking, |this| {
            this.child(
                div()
                    .flex_none()
                    .px(px(20.0))
                    .py(px(14.0))
                    .border_t_1()
                    .border_color(Hsla {
                        a: 0.18,
                        ..colors.border
                    })
                    .flex()
                    .justify_between()
                    .when(launch_confirmation, |this| {
                        this.child(
                            div()
                                .id("uwp-safety-cancel-launch")
                                .h(px(38.0))
                                .px(px(14.0))
                                .rounded(px(crate::ui::theme::tokens::radius::SM))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_secondary)
                                .hover(|this| this.bg(colors.surface_hover))
                                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                    cancel_launch(cx);
                                })
                                .child("取消启动"),
                        )
                    })
                    .child(
                        div()
                            .id("uwp-safety-guide-continue")
                            .h(px(38.0))
                            .px(px(18.0))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .bg(colors.accent)
                            .cursor_pointer()
                            .hover(|this| this.bg(colors.accent_hover))
                            .active(|this| {
                                this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.btn_primary_text)
                            .child(if launch_confirmation {
                                "由 BMCBL 自动保护并继续启动"
                            } else {
                                "知道了，继续"
                            })
                            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                continue_after_confirmation(cx);
                            }),
                    ),
            )
        });

    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(Hsla { a: 0.24, ..black() })
        .p(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(card)
}

fn render_checking_body(colors: &ThemeColors) -> Div {
    div()
        .size_full()
        .p(px(20.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .child(
            div()
                .size(px(48.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.11,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_package_search())
                        .size(px(22.0))
                        .text_color(colors.accent),
                ),
        )
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child("正在确认当前 Minecraft UWP 的注册来源…"),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_center()
                .line_height(px(17.0))
                .text_color(colors.text_secondary)
                .child(
                    "检查完成后，如果没有 Microsoft Store / 系统 UWP，会自动回到原来的下载确认。",
                ),
        )
}

fn render_safety_body(
    trigger: UwpSafetyGuideTrigger,
    registration: Option<&SystemUwpRegistration>,
    colors: &ThemeColors,
) -> Div {
    let context = match trigger {
        UwpSafetyGuideTrigger::DownloadRelease | UwpSafetyGuideTrigger::DownloadPreview => {
            "检测到当前 Windows 使用 Microsoft Store / 系统方式安装的 Minecraft UWP。你正在下载另一个 UWP 版本；下载本身不会卸载系统版本，只有以后实际切换注册时才会进入数据保护流程。"
        }
        UwpSafetyGuideTrigger::Import => {
            "检测到当前 Windows 使用 Microsoft Store / 系统方式安装的 Minecraft UWP。你正在导入 APPX / ZIP；导入本身不会卸载系统版本，只有以后实际切换注册时才会进入数据保护流程。"
        }
        UwpSafetyGuideTrigger::Launch => {
            "检测到当前 Windows 使用 Microsoft Store / 系统方式安装的 Minecraft UWP。启动本地下载版本时，Windows 可能需要移除当前 Store 包注册，再注册本地版本路径；整个数据保护流程由 BMCBL 自动完成。"
        }
    };

    let mut content = div()
        .w_full()
        .p(px(20.0))
        .flex()
        .flex_col()
        .gap(px(11.0))
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(19.0))
                .text_color(colors.text_secondary)
                .child(context),
        );

    if let Some(summary) = registration {
        let channel = match summary.channel {
            MinecraftUwpChannel::Release => "Microsoft Store 正式版",
            MinecraftUwpChannel::Preview => "Microsoft Store Preview",
        };
        let version = summary.version.as_deref().unwrap_or("未知版本");
        content = content.child(
            div()
                .w_full()
                .px(px(11.0))
                .py(px(9.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(format!("当前系统注册：{channel} · {version}")),
        );
    }

    content
        .child(safety_step(
            colors,
            1,
            "下载和导入阶段不会更改注册",
            "BMCBL 只保存新的本地 UWP 版本文件；真正启动且注册路径不同时才进行切换。",
        ))
        .child(safety_step(
            colors,
            2,
            "移除 Store 注册前自动备份并校验",
            "BMCBL 自动备份 LocalState 中的 Minecraft 数据并核对文件统计；备份或校验失败会在移除前停止，无需手动导出。",
        ))
        .child(safety_step(
            colors,
            3,
            "注册本地路径后自动恢复",
            "本地包注册和路径校验通过后，仅在目标数据目录为空时恢复并复核数据；迁移备份会继续保留。",
        ))
        .child(
            div()
                .w_full()
                .px(px(11.0))
                .py(px(8.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.07,
                    ..colors.surface
                })
                .text_size(px(11.0))
                .line_height(px(17.0))
                .text_color(colors.text_secondary)
                .child("切换由 Windows Package Manager 执行。游戏退出后不会自动切回 Microsoft Store 注册；以后启动其他 UWP 路径时会再次检查。GDK / MSIXVC 不触发。"),
        )
}

fn safety_step(colors: &ThemeColors, number: u8, title: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .size(px(26.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(17.0))
                        .text_color(colors.text_secondary)
                        .child(detail),
                ),
        )
}
