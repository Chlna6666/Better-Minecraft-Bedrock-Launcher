use crate::ui::components::code_editor::{
    CodeEditor, CodeEditorEvent, CodeEditorLanguage, CodeEditorState,
};
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::manage::data::{self, LevelDatDocument};
use crate::ui::views::manage::level_dat_editor::{self, LevelDatJsonValidation};
use crate::ui::views::manage::state::{ManageAssetEntry, ManagedVersionEntry};
use gpui::prelude::FluentBuilder as _;
use gpui::*;

#[derive(Clone)]
pub struct LevelDatCodeWindowInit {
    pub version: ManagedVersionEntry,
    pub asset: ManageAssetEntry,
    pub document_version: u32,
    pub initial_text: SharedString,
    pub saved_text: SharedString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowStatusKind {
    Error,
    Success,
}

pub struct LevelDatCodeWindowView {
    version: ManagedVersionEntry,
    asset: ManageAssetEntry,
    document_version: u32,
    json_editor: Entity<CodeEditorState>,
    validation: LevelDatJsonValidation,
    saved_text: SharedString,
    saving: bool,
    status: Option<(WindowStatusKind, SharedString)>,
    _subscriptions: Vec<Subscription>,
}

impl LevelDatCodeWindowView {
    pub fn new(init: LevelDatCodeWindowInit, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let validation = level_dat_editor::validate_document_json(init.initial_text.as_ref());
        let json_editor = cx.new(|cx| {
            let mut editor = CodeEditorState::new(cx);
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor.set_value(init.initial_text.clone(), cx);
            editor
        });
        let subscriptions = vec![
            cx.observe_global::<I18n>(|_, cx| {
                cx.notify();
            }),
            cx.subscribe(&json_editor, |this, _editor, event, cx| match event {
                CodeEditorEvent::Change => this.revalidate(cx),
                CodeEditorEvent::SaveRequested => this.save_json(cx),
                CodeEditorEvent::FormatRequested => this.format_json(cx),
                CodeEditorEvent::PointerInteractionStarted
                | CodeEditorEvent::PointerInteractionEnded => {}
            }),
        ];

        Self {
            version: init.version,
            asset: init.asset,
            document_version: init.document_version,
            json_editor,
            validation,
            saved_text: init.saved_text,
            saving: false,
            status: None,
            _subscriptions: subscriptions,
        }
    }

    fn theme_colors(&self, now: std::time::Instant, cx: &App) -> ThemeColors {
        let theme = cx.global::<ThemeState>();
        lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        )
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let editor_text = self.json_editor.read(cx).value();
        self.validation = level_dat_editor::validate_document_json(editor_text.as_ref());
        if self
            .status
            .as_ref()
            .is_some_and(|(kind, _)| *kind == WindowStatusKind::Error)
            && self.validation.valid
        {
            self.status = None;
        }
        cx.notify();
    }

    fn format_json(&mut self, cx: &mut Context<Self>) {
        let editor_text = self.json_editor.read(cx).value();
        let parsed_root = match level_dat_editor::parse_document_json(editor_text.as_ref()) {
            Ok(root) => root,
            Err(validation) => {
                let validation: LevelDatJsonValidation = validation;
                self.validation = validation.clone();
                self.status = Some((
                    WindowStatusKind::Error,
                    validation
                        .detail
                        .clone()
                        .unwrap_or_else(|| validation.summary.clone()),
                ));
                cx.notify();
                return;
            }
        };

        let formatted: SharedString = match level_dat_editor::format_document_json(&parsed_root) {
            Ok(text) => text,
            Err(error) => {
                self.status = Some((WindowStatusKind::Error, SharedString::from(error)));
                cx.notify();
                return;
            }
        };

        self.json_editor.update(cx, |editor, cx| {
            editor.set_value(formatted.clone(), cx);
        });
        self.validation = level_dat_editor::validate_document_json(formatted.as_ref());
        self.status = Some((WindowStatusKind::Success, t!("LevelDat.format_success")));
        cx.notify();
    }

    fn save_json(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }

        let editor_text = self.json_editor.read(cx).value();
        let parsed_root = match level_dat_editor::parse_document_json(editor_text.as_ref()) {
            Ok(root) => root,
            Err(validation) => {
                let validation: LevelDatJsonValidation = validation;
                self.validation = validation.clone();
                self.status = Some((
                    WindowStatusKind::Error,
                    validation
                        .detail
                        .clone()
                        .unwrap_or_else(|| validation.summary.clone()),
                ));
                cx.notify();
                return;
            }
        };

        let folder_path = self.asset.file_path.to_string();
        let document = LevelDatDocument::new(self.document_version, parsed_root);
        let saved_text = editor_text.clone();

        self.saving = true;
        self.status = None;
        self.validation = level_dat_editor::validate_document_json(saved_text.as_ref());
        let i18n = cx.global::<I18n>().clone();
        let history_capture_label = t!("LevelDat.history_capture").to_string();
        let history_saved_label = t!("LevelDat.history_saved").to_string();
        let history_capture_failed = t!("LevelDat.history_capture_failed");
        let write_task_failed = t!("LevelDat.write_task_failed");

        cx.spawn(async move |handle, cx| {
            let result: Result<(), String> = crate::tasks::runtime::run_io_blocking(move || {
                let world_path = std::path::PathBuf::from(&folder_path);
                let history_capture = crate::ui::window::map_viewer::map_history::capture_before(
                    crate::ui::window::map_viewer::map_history::MapHistoryCaptureSpec {
                        kind: crate::ui::window::map_viewer::map_history::MapHistoryEntryKind::LevelDatSave,
                        label: history_capture_label,
                        world_path: world_path.clone(),
                        chunks: std::collections::BTreeSet::new(),
                        raw_keys: std::collections::BTreeSet::new(),
                        include_level_dat: true,
                    },
                );
                let result = data::write_level_dat_document(&folder_path, &document);
                match (history_capture, result) {
                    (Ok(capture), Ok(())) => {
                        crate::ui::window::map_viewer::map_history::complete_after(
                            capture,
                            history_saved_label,
                        )?;
                        Ok(())
                    }
                    (Ok(capture), Err(error)) => {
                        let _ = crate::ui::window::map_viewer::map_history::complete_failed(
                            capture,
                            error.clone(),
                        );
                        Err(error)
                    }
                    (Err(error), Ok(())) => {
                        tracing::warn!(%error, "map history capture failed after level.dat save");
                        Ok(())
                    }
                    (Err(history_error), Err(write_error)) => {
                        Err(format!(
                            "{write_error}; {history_capture_failed}: {history_error}"
                        ))
                    }
                }
            })
            .await
            .map_err(|error| format!("{write_task_failed}: {error}"))
            .and_then(|result| result);

            let _ = handle.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.saved_text = saved_text.clone();
                        this.validation =
                            level_dat_editor::validate_document_json(saved_text.as_ref());
                        this.status = Some((
                            WindowStatusKind::Success,
                            t!("LevelDat.save_success"),
                        ));
                    }
                    Err(error) => {
                        this.status = Some((WindowStatusKind::Error, SharedString::from(error)));
                    }
                }
                cx.notify();
            });

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

impl Render for LevelDatCodeWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = cx.global::<I18n>().clone();
        let window_title = t!("LevelDat.title");
        window.set_title(&window_title);
        let colors = self.theme_colors(window.animation_time(), cx);
        let editor_text = self.json_editor.read(cx).value();
        let dirty = editor_text != self.saved_text;
        let line_count = editor_text.as_ref().lines().count().max(1);
        let char_count = editor_text.chars().count();
        let instance_name = self.version.display_name().to_string();
        let document_version = self.document_version.to_string();
        let line_count_string = line_count.to_string();
        let char_count_string = char_count.to_string();
        let save_path = format!("{}\\level.dat", self.asset.file_path);
        let status_text = self
            .status
            .as_ref()
            .map(|(_, message)| message.clone())
            .unwrap_or_else(|| {
                self.validation.detail.clone().unwrap_or_else(|| {
                    if dirty {
                        t!("LevelDat.unsaved_hint")
                    } else {
                        t!("LevelDat.synced_hint")
                    }
                })
            });

        div()
            .size_full()
            .bg(colors.settings_panel_bg)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .px(px(18.))
                    .py(px(14.))
                    .border_b_1()
                    .border_color(Hsla {
                        a: 0.12,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.text_primary)
                                    .child(t!("LevelDat.code_window")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.text_secondary)
                                    .child(self.asset.display_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(colors.text_muted)
                                    .child(save_path),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .flex_wrap()
                                    .child(info_badge(
                                        &colors,
                                        t!("LevelDat.instance", name = &instance_name),
                                    ))
                                    .child(info_badge(
                                        &colors,
                                        t!("LevelDat.version_header", version = &document_version),
                                    ))
                                    .child(info_badge(
                                        &colors,
                                        t!("LevelDat.lines", count = &line_count_string),
                                    ))
                                    .child(info_badge(
                                        &colors,
                                        t!("LevelDat.characters", count = &char_count_string),
                                    ))
                                    .when(dirty, |this| {
                                        this.child(status_badge(
                                            &colors,
                                            t!("LevelDat.unsaved"),
                                            colors.stat_orange_text,
                                        ))
                                    })
                                    .child(if self.validation.valid {
                                        status_badge(
                                            &colors,
                                            t!("LevelDat.json_valid"),
                                            colors.stat_green_text,
                                        )
                                    } else {
                                        status_badge(
                                            &colors,
                                            t!("LevelDat.json_invalid"),
                                            colors.danger,
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                action_button(&colors, false, t!("LevelDat.format")).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.format_json(cx);
                                    }),
                                ),
                            )
                            .child(
                                action_button(
                                    &colors,
                                    true,
                                    if self.saving {
                                        t!("LevelDat.saving")
                                    } else {
                                        t!("LevelDat.save")
                                    },
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.save_json(cx);
                                    }),
                                ),
                            )
                            .child(
                                action_button(&colors, false, t!("common.close"))
                                    .id("level-dat-close")
                                    .on_click(move |_, window, _cx| {
                                        window.remove_window();
                                    }),
                            ),
                    ),
            )
            .child(
                div().flex_1().min_w(px(0.)).min_h(px(0.)).p(px(10.)).child(
                    CodeEditor::new(&self.json_editor, &colors)
                        .w_full()
                        .h_full()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.14,
                            ..colors.border
                        })
                        .bg(colors.surface),
                ),
            )
            .child(
                div()
                    .px(px(18.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(Hsla {
                        a: 0.12,
                        ..colors.border
                    })
                    .text_size(px(12.))
                    .text_color(match self.status.as_ref().map(|(kind, _)| *kind) {
                        Some(WindowStatusKind::Error) => colors.danger,
                        Some(WindowStatusKind::Success) => colors.stat_green_text,
                        None => colors.text_secondary,
                    })
                    .child(status_text),
            )
    }
}

fn info_badge(colors: &ThemeColors, label: SharedString) -> Div {
    div()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .bg(Hsla {
            a: 0.08,
            ..colors.surface
        })
        .text_size(px(11.))
        .text_color(colors.text_secondary)
        .child(label)
}

fn status_badge(colors: &ThemeColors, label: SharedString, accent: Hsla) -> Div {
    div()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .bg(Hsla { a: 0.12, ..accent })
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(accent)
        .child(label)
}

fn action_button(colors: &ThemeColors, primary: bool, label: SharedString) -> Div {
    div()
        .px(px(16.))
        .h(px(38.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(if primary {
            Hsla {
                a: 0.32,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.18,
                ..colors.border
            }
        })
        .bg(if primary {
            Hsla {
                a: 0.14,
                ..colors.accent
            }
        } else {
            colors.surface
        })
        .text_color(if primary {
            colors.text_primary
        } else {
            colors.text_secondary
        })
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(label)
}

pub fn open_level_dat_code_window(init: LevelDatCodeWindowInit, cx: &mut App) {
    let title = t!("LevelDat.window_title", name = &init.asset.display_name).to_string();
    let options = level_dat_code_window_options(cx);
    let window = cx.open_window(options, move |window, cx| {
        window.set_title(&title);
        window.activate_window();

        let view = cx.new(|cx| LevelDatCodeWindowView::new(init, window, cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });

    if let Err(error) = window {
        eprintln!("Failed to open level.dat code window: {error:?}");
    }
}

fn level_dat_code_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    options.window_bounds = Some(WindowBounds::centered(size(px(1360.), px(920.)), cx));
    options.window_min_size = Some(size(px(920.), px(640.)));
    options.is_resizable = true;
    options.is_minimizable = true;
    options.is_movable = true;

    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(t!("LevelDat.title")),
            appears_transparent: false,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Opaque;
    }

    options
}