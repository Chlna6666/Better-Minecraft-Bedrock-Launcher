//! 视图：主页面、独立窗口页面、模态框页面和两处注入。
//!
//! 渲染阶段只读取会话状态并调用 `tr!`/`theme_snapshot()` 这类纯读取接口，因此这里不会触发
//! 磁盘、网络或配置写入。

use crate::actions::ShowcaseAction;
use crate::state::{self, TASK_TOTAL};
use crate::{PAGE_MODAL, PAGE_WINDOW, PLUGIN, RESOURCE_NOTICE};
use bmcbl_plugin_api::prelude::*;

/// 页面入口：主页面、窗口页面或模态框页面。
pub fn page(page_id: &str) -> ViewTree {
    match page_id {
        PAGE_WINDOW => window_page(),
        PAGE_MODAL => modal_page(),
        _ => main_page(),
    }
}

/// 注入入口：主窗口顶部覆盖层与首页侧边栏。
pub fn injection(slot: InjectionSlot) -> Option<ViewTree> {
    match slot {
        InjectionSlot::MainRootOverlay => Some(overlay()),
        InjectionSlot::HomeSidebar => Some(sidebar()),
        InjectionSlot::PageHeader | InjectionSlot::PageBody => None,
    }
}

fn main_page() -> ViewTree {
    view! {
        column(padding = 20, gap = 12) {
            title(tr!("showcase.title"));
            text(tr!("showcase.summary", "version" => PLUGIN.version, "authors" => PLUGIN.authors_display()));
            status_badges();
            state_section();
            io_section();
            task_section();
            navigation_section();
            footer_row();
        }
    }
}

fn status_badges() -> View {
    View::row()
        .gap(8)
        .align(Align::Start)
        .child(badge(tr!("showcase.badge.counter", "value" => state::counter())))
        .child(badge(tr!("showcase.badge.mode", "value" => state::mode())))
        .child(badge(tr!("showcase.badge.route", "value" => state::route())))
        .child(badge(tr!("showcase.badge.ping", "value" => state::ping_count())))
        .child(badge(tr!("showcase.badge.theme", "value" => theme_label())))
        .finish_view()
}

fn theme_label() -> String {
    match theme_snapshot() {
        Ok(snapshot) if snapshot.is_dark() => tr!("showcase.theme.dark"),
        Ok(_) => tr!("showcase.theme.light"),
        Err(_) => tr!("showcase.theme.unknown"),
    }
}

fn state_section() -> View {
    section(
        tr!("showcase.section.state"),
        vec![
            View::text(tr!("showcase.notes", "value" => notes_preview())),
            View::checkbox(
                tr!("showcase.toggle.advanced"),
                state::advanced(),
                ShowcaseAction::ToggleAdvanced.as_str(),
            ),
            View::select(
                tr!("showcase.select.mode"),
                ShowcaseAction::SetMode.as_str(),
                vec![
                    option(tr!("showcase.mode.balanced"), "balanced"),
                    option(tr!("showcase.mode.quiet"), "quiet"),
                    option(tr!("showcase.mode.verbose"), "verbose"),
                ],
                Some(state::mode()),
            ),
            View::button(
                tr!("showcase.button.save_notes"),
                ShowcaseAction::SaveNotes.as_str(),
            ),
            View::button_with_value(
                tr!("showcase.button.append_note"),
                ShowcaseAction::SaveNotes.as_str(),
                format!("{} {}", state::notes(), state::counter()),
            ),
            View::button(tr!("showcase.button.load_notes"), ShowcaseAction::LoadNotes.as_str()),
            View::text(tr!("showcase.config", "value" => config_preview())),
            View::button(tr!("showcase.button.save_config"), ShowcaseAction::SaveConfig.as_str()),
            View::button(
                tr!("showcase.button.reload_config"),
                ShowcaseAction::ReloadConfig.as_str(),
            ),
            View::checkbox(
                tr!("showcase.toggle.full_width"),
                state::advanced(),
                ShowcaseAction::ToggleAdvanced.as_str(),
            ),
            View::toggle(
                tr!("showcase.toggle.increment"),
                state::advanced(),
                ShowcaseAction::Increment.as_str(),
            ),
        ],
    )
}

/// 配置摘要：只显示宿主返回的字节数，避免在渲染阶段做任何解析。
fn config_preview() -> String {
    let text = state::config_text();
    if text.is_empty() {
        tr!("showcase.config.empty")
    } else {
        tr!("showcase.config.bytes", "bytes" => text.len())
    }
}

fn notes_preview() -> String {
    if state::notes().is_empty() {
        tr!("showcase.notes.empty")
    } else {
        state::notes()
    }
}

fn io_section() -> View {
    section(
        tr!("showcase.section.io"),
        vec![
            View::text(tr!("showcase.remote", "value" => remote_preview())),
            View::text(tr!("showcase.clipboard", "value" => clipboard_preview())),
            View::text(tr!("showcase.notice", "value" => notice_preview())),
            View::button(tr!("showcase.button.fetch"), ShowcaseAction::FetchSite.as_str()),
            View::button(tr!("showcase.button.copy"), ShowcaseAction::CopySummary.as_str()),
            View::button(
                tr!("showcase.button.paste"),
                ShowcaseAction::PasteClipboard.as_str(),
            ),
            View::button(tr!("showcase.button.notice"), ShowcaseAction::ReadNotice.as_str()),
        ],
    )
}

fn remote_preview() -> String {
    preview_or(&state::remote(), "showcase.remote.idle")
}

fn clipboard_preview() -> String {
    preview_or(&state::clipboard_preview(), "showcase.clipboard.empty")
}

fn notice_preview() -> String {
    preview_or(&state::notice(), "showcase.notice.idle")
}

fn preview_or(value: &str, empty_key: &str) -> String {
    if value.is_empty() {
        tr!(empty_key)
    } else {
        value.to_string()
    }
}

fn task_section() -> View {
    let done = state::task_done();
    let stage = if state::task_id().is_some() {
        tr!("showcase.task.running")
    } else {
        tr!("showcase.task.idle")
    };
    section(
        tr!("showcase.section.task"),
        vec![
            View::progress(tr!("showcase.progress.task"), done, Some(TASK_TOTAL)),
            View::text(tr!("showcase.task.stage", "stage" => stage, "done" => done, "total" => TASK_TOTAL)),
            View::button(tr!("showcase.button.task_start"), ShowcaseAction::StartTask.as_str()),
            View::button(tr!("showcase.button.task_step"), ShowcaseAction::StepTask.as_str()),
            View::button(
                tr!("showcase.button.task_finish"),
                ShowcaseAction::FinishTask.as_str(),
            ),
        ],
    )
}

fn navigation_section() -> View {
    section(
        tr!("showcase.section.navigation"),
        vec![
            View::button(tr!("showcase.button.window"), ShowcaseAction::OpenWindow.as_str()),
            View::button(tr!("showcase.button.modal"), ShowcaseAction::OpenModal.as_str()),
            View::button(tr!("showcase.button.docs"), ShowcaseAction::OpenDocs.as_str()),
            View::button(
                tr!("showcase.button.settings"),
                ShowcaseAction::GoSettings.as_str(),
            ),
            View::button(tr!("showcase.button.ping"), ShowcaseAction::EmitPing.as_str()),
            View::button(
                tr!("showcase.button.invalidate"),
                ShowcaseAction::InvalidateAll.as_str(),
            ),
        ],
    )
}

fn footer_row() -> View {
    View::row()
        .gap(8)
        .align(Align::Center)
        .child(icon("star"))
        .child(link(tr!("showcase.link.docs"), "https://bmcbl.com/"))
        .child(spacer(8))
        .child(image_with_options(
            RESOURCE_NOTICE,
            tr!("showcase.image.alt"),
            ImageOptions::new()
                .caption(tr!("showcase.image.caption"))
                .placeholder(tr!("showcase.image.placeholder"))
                .fallback(tr!("showcase.image.fallback"))
                .height(48)
                .corner_radius(8)
                .fit(ImageFit::Cover),
        ))
        .finish_view()
}

fn window_page() -> ViewTree {
    view! {
        column(padding = 18, gap = 10) {
            title(tr!("showcase.window.title"));
            text(tr!("showcase.window.body"));
            View::row()
                .gap(8)
                .align(Align::Start)
                .child(badge(tr!("showcase.badge.counter", "value" => state::counter())))
                .child(badge(tr!("showcase.badge.mode", "value" => state::mode())))
                .finish_view();
            card(vec![
                View::text(tr!("showcase.notes", "value" => notes_preview())),
                View::button(
                    tr!("showcase.button.increment"),
                    ShowcaseAction::Increment.as_str(),
                ),
                View::button(tr!("showcase.button.reset"), ShowcaseAction::Reset.as_str()),
            ]);
        }
    }
}

fn modal_page() -> ViewTree {
    view! {
        column(padding = 18, gap = 10) {
            title(tr!("showcase.modal.title"));
            text(tr!("showcase.modal.body"));
            progress(tr!("showcase.progress.task"), state::task_done(), Some(TASK_TOTAL));
            button(tr!("showcase.button.task_step"), ShowcaseAction::StepTask.as_str());
            button(tr!("showcase.button.invalidate"), ShowcaseAction::InvalidateAll.as_str());
        }
    }
}

fn overlay() -> ViewTree {
    view! {
        column(padding = 10, gap = 6) {
            View::row()
                .gap(6)
                .align(Align::Center)
                .child(icon("plug"))
                .child(badge(tr!("showcase.overlay.counter", "value" => state::counter())))
                .child(badge(tr!("showcase.overlay.ping", "value" => state::ping_count())))
                .finish_view();
            text(tr!("showcase.overlay.hint", "route" => state::route()));
            button(tr!("showcase.button.increment"), ShowcaseAction::Increment.as_str());
        }
    }
}

fn sidebar() -> ViewTree {
    view! {
        column(padding = 12, gap = 8) {
            title(tr!("showcase.sidebar.title"));
            text(tr!("showcase.sidebar.hint"));
            badge(tr!("showcase.badge.mode", "value" => state::mode()));
            text(tr!("showcase.notes", "value" => notes_preview()));
            button(tr!("showcase.button.increment"), ShowcaseAction::Increment.as_str());
            button(tr!("showcase.button.window"), ShowcaseAction::OpenWindow.as_str());
        }
    }
}

