#![cfg(target_os = "windows")]

use gpui::{AppContext as _, BorrowAppContext as _, *};
use lucide_gpui::icons as lucide_icons;

use crate::core::minecraft::uwp_migration::MinecraftDataSummary;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UwpSafetyGuideTrigger {
    #[default]
    DownloadRelease,
    DownloadPreview,
    Import,
}

pub struct UwpSafetyGuideState {
    pub visible: bool,
    pub checking: bool,
    pub trigger: Option<UwpSafetyGuideTrigger>,
    pub system_registration: Option<MinecraftDataSummary>,
    request_id: u64,
    pending: bool,
    active_download_package: Option<SharedString>,
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

    fn begin_check(&mut self, trigger: UwpSafetyGuideTrigger) -> (u64, UwpSafetyGuideTrigger) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = false;
        self.checking = true;
        self.pending = false;
        self.trigger = Some(trigger);
        self.system_registration = None;
        (self.request_id, trigger)
    }

    fn apply_check(
        &mut self,
        request_id: u64,
        registration: Option<MinecraftDataSummary>,
        tour_visible: bool,
    ) {
        if self.request_id != request_id {
            return;
        }

        self.checking = false;
        self.system_registration = registration;
        let should_show = self.system_registration.is_some();
        self.pending = should_show && tour_visible;
        self.visible = should_show && !tour_visible;
        if !should_show {
            self.trigger = None;
        }
    }

    fn fail_check(&mut self, request_id: u64) {
        if self.request_id != request_id {
            return;
        }
        self.checking = false;
        self.visible = false;
        self.pending = false;
        self.trigger = None;
        self.system_registration = None;
    }

    fn activate_pending(&mut self) {
        if self.pending && self.system_registration.is_some() {
            self.pending = false;
            self.visible = true;
        }
    }

    fn dismiss(&mut self) {
        self.visible = false;
        self.pending = false;
        self.trigger = None;
        self.system_registration = None;
    }

    fn clear_download_context(&mut self) {
        if self.active_download_package.take().is_none() {
            return;
        }

        // 对话框已经结束时让同一个版本下一次下载重新实时检测。
        // 若异步检查仍未返回，同时递增 request_id 使旧结果失效。
        if matches!(
            self.trigger,
            Some(
                UwpSafetyGuideTrigger::DownloadRelease | UwpSafetyGuideTrigger::DownloadPreview
            )
        ) {
            self.request_id = self.request_id.wrapping_add(1).max(1);
            self.checking = false;
            self.visible = false;
            self.pending = false;
            self.trigger = None;
            self.system_registration = None;
        }
    }
}

pub fn request_download(
    package_id: SharedString,
    version_type: i32,
    cx: &mut App,
) {
    cx.default_global::<UwpSafetyGuideState>();
    let request = cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.begin_download(package_id, version_type)
    });
    if let Some((request_id, trigger)) = request {
        start_system_registration_check(request_id, trigger, cx);
    }
}

pub fn request_import(cx: &mut App) {
    cx.default_global::<UwpSafetyGuideState>();
    let (request_id, trigger) = cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.begin_import()
    });
    start_system_registration_check(request_id, trigger, cx);
}

pub fn clear_download_context(cx: &mut App) {
    let should_clear = cx
        .try_global::<UwpSafetyGuideState>()
        .is_some_and(|state| state.active_download_package.is_some());
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

fn start_system_registration_check(
    request_id: u64,
    trigger: UwpSafetyGuideTrigger,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            let environment = crate::core::minecraft::uwp_migration::scan_onboarding_environment();
            match trigger {
                UwpSafetyGuideTrigger::DownloadRelease => {
                    official_store_registration(environment.release)
                }
                UwpSafetyGuideTrigger::DownloadPreview => {
                    official_store_registration(environment.preview)
                }
                UwpSafetyGuideTrigger::Import => {
                    official_store_registration(environment.release)
                        .or_else(|| official_store_registration(environment.preview))
                }
            }
        })
        .await;

        cx.update(|cx| {
            match result {
                Ok(registration) => {
                    let tour_visible = cx
                        .try_global::<super::state::OnboardingTourState>()
                        .is_some_and(|tour| tour.visible);
                    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
                        state.apply_check(request_id, registration, tour_visible);
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, "检测 Microsoft Store Minecraft UWP 注册失败");
                    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
                        state.fail_check(request_id);
                    });
                }
            }
            Ok::<(), anyhow::Error>(())
        })??;

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn official_store_registration(summary: MinecraftDataSummary) -> Option<MinecraftDataSummary> {
    // Microsoft Store / 系统安装的主包不是 DevelopmentMode。
    // 外部 DevelopmentMode 与 BMCBL loose UWP 都不在下载阶段弹提示；真正替换注册时
    // 仍由 uwp_migration 的强制备份安全门负责保护数据。
    (summary.registered && !summary.development_mode).then_some(summary)
}

fn dismiss(cx: &mut App) {
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| state.dismiss());
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
    let card_w = (width - 28.0).clamp(350.0, 520.0);
    let card_h = (height - 40.0).clamp(400.0, 476.0);
    let trigger = state.trigger.unwrap_or_default();
    let registration = state.system_registration.as_ref();

    let context = match trigger {
        UwpSafetyGuideTrigger::DownloadRelease | UwpSafetyGuideTrigger::DownloadPreview => {
            "检测到当前 Windows 使用 Microsoft Store / 系统方式安装的 Minecraft UWP。你正在下载另一个 UWP 版本；下载本身不会卸载系统版本，只有以后实际切换注册时才会进入数据保护流程。"
        }
        UwpSafetyGuideTrigger::Import => {
            "检测到当前 Windows 使用 Microsoft Store / 系统方式安装的 Minecraft UWP。你正在导入 APPX / ZIP；导入本身不会卸载系统版本，只有以后实际切换注册时才会进入数据保护流程。"
        }
    };

    let registration_text = registration.map(|summary| {
        let channel = if summary.family_name.contains("Beta") {
            "Microsoft Store Preview"
        } else {
            "Microsoft Store 正式版"
        };
        let version = summary.registered_version.as_deref().unwrap_or("未知版本");
        if summary.data_present {
            format!(
                "{channel} · {version} · {} 个世界 · {} 个资源包",
                summary.worlds, summary.resource_packs
            )
        } else {
            format!("{channel} · {version} · 未发现 games/com.mojang 数据")
        }
    });

    let mut content = div()
        .flex_1()
        .min_h(px(0.0))
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

    if let Some(registration_text) = registration_text {
        content = content.child(
            div()
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
                .child(registration_text),
        );
    }

    content = content
        .child(safety_step(
            &colors,
            1,
            "当前系统版本不会在下载时被删除",
            "BMCBL 只是取得新的本地 UWP 版本文件；此时不会更改 Microsoft Store 注册。",
        ))
        .child(safety_step(
            &colors,
            2,
            "真正切换时先备份再替换",
            "如果后续需要替换 Store 注册并检测到 games/com.mojang 数据，会先备份并校验；失败就停止卸载。",
        ))
        .child(safety_step(
            &colors,
            3,
            "注册成功后才恢复数据",
            "目标数据目录为空时才自动恢复备份，避免覆盖已经存在的 Minecraft 数据。",
        ))
        .child(
            div()
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
                .child("此提示没有独立配置或“已确认”记录；每次 UWP 下载/导入都会按当前系统注册状态实时判断。GDK / MSIXVC 不触发。"),
        );

    let card = div()
        .w(px(card_w))
        .h(px(card_h))
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
            color: Hsla {
                a: 0.22,
                ..black()
            },
            blur_radius: px(32.0),
            spread_radius: px(-5.0),
            offset: point(px(0.0), px(14.0)),
        }])
        .flex()
        .flex_col()
        .child(
            div()
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
                                .path(lucide_icons::icon_shield_check())
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
                                .child("检测到系统安装的 UWP"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child("仅根据当前 Windows 注册状态实时提示，不保存独立配置。"),
                        ),
                ),
        )
        .child(content)
        .child(
            div()
                .px(px(20.0))
                .py(px(14.0))
                .border_t_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .flex()
                .justify_end()
                .child(
                    div()
                        .id("uwp-safety-guide-dismiss")
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
                        .child("知道了，继续")
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            dismiss(cx);
                        }),
                ),
        );

    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(Hsla {
            a: 0.24,
            ..black()
        })
        .flex()
        .items_center()
        .justify_center()
        .child(card)
}

fn safety_step(
    colors: &ThemeColors,
    number: u8,
    title: &'static str,
    detail: &'static str,
) -> impl IntoElement {
    div()
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
