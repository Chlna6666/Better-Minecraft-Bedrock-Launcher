use super::ManagePageView;
use crate::core::minecraft::nbt::NbtTag;
use crate::ui::components::code_editor::{CodeEditor, CodeEditorState};
use crate::ui::components::input::{Input, InputState};
use crate::ui::components::minecraft_text::MinecraftFormattedText;
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::tabs::{AnimatedSegmentTabs, TabItem};
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::manage::common::{
    card_title, panel_shell, primary_button, secondary_button, subtle_badge,
};
use crate::ui::views::manage::data::LevelDatDocument;
use crate::ui::views::manage::level_dat_schema;
use crate::ui::views::manage::state::{ManageAssetEntry, ManagedVersionEntry};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub struct LevelDatEditorModalState {
    pub version: ManagedVersionEntry,
    pub asset: ManageAssetEntry,
    pub document: LevelDatDocument,
    pub json_editor: Entity<CodeEditorState>,
    pub mode: LevelDatEditorMode,
    pub validation: LevelDatJsonValidation,
    pub saved_text: SharedString,
    pub saving: bool,
    pub form_inputs: HashMap<String, Entity<InputState>>,
    pub needs_form_sync: bool,
    pub visual_dirty: bool,
    pub collapsed_groups: HashSet<LevelDatFieldGroup>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LevelDatEditorMode {
    Visual,
    Json,
}

#[derive(Clone)]
pub struct LevelDatJsonValidation {
    pub valid: bool,
    pub summary: SharedString,
    pub detail: Option<SharedString>,
}

pub use crate::ui::views::manage::level_dat_schema::{
    BoolFieldSpec, ChoiceFieldSpec, LevelDatFieldGroup, LevelDatFieldSection, TagScope,
    ValueFieldKind, ValueFieldSpec,
};

pub fn render(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    _i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
    cx: &App,
) -> AnyElement {
    render_surface(state, colors, view_handle, cx, false)
}

pub fn render_page(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    _i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
    cx: &App,
) -> AnyElement {
    render_surface(state, colors, view_handle, cx, true)
}

fn render_surface(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
    cx: &App,
    page_mode: bool,
) -> AnyElement {
    let editor_text = state.json_editor.read(cx).value();
    let dirty = match state.mode {
        LevelDatEditorMode::Visual => state.visual_dirty,
        LevelDatEditorMode::Json => editor_text != state.saved_text,
    };
    let line_count = editor_text.as_ref().lines().count().max(1);
    let char_count = editor_text.chars().count();

    let content = div()
        .w_full()
        .h_full()
        .min_w(px(0.))
        .min_h(px(0.))
        .when(!page_mode, |this| this.max_w(px(1180.)).max_h(px(780.)))
        .rounded(if page_mode { px(18.) } else { px(24.) })
        .border_1()
        .border_color(Hsla {
            a: if page_mode { 0.18 } else { 0.22 },
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
        .when(!page_mode, |this| {
            this.shadow(vec![BoxShadow {
                color: Hsla { a: 0.22, ..black() },
                blur_radius: px(34.0),
                spread_radius: px(-12.0),
                offset: point(px(0.0), px(18.0)),
            }])
        })
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(render_header(
            state,
            colors,
            dirty,
            line_count,
            char_count,
            view_handle.clone(),
            page_mode,
        ))
        .child(match state.mode {
            LevelDatEditorMode::Visual => {
                render_visual_body(state, colors, view_handle.clone(), page_mode)
            }
            LevelDatEditorMode::Json => {
                render_json_body(state, colors, view_handle.clone(), page_mode)
            }
        })
        .child(render_footer(state, colors, dirty, view_handle, page_mode));

    if page_mode {
        content.into_any_element()
    } else {
        modal::modal_layer(content, colors.backdrop).into_any_element()
    }
}

fn render_header(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    dirty: bool,
    line_count: usize,
    char_count: usize,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> Div {
    let compact_json_page = page_mode && state.mode == LevelDatEditorMode::Json;
    let tabs = AnimatedSegmentTabs::new(
        "level-dat-mode-tabs",
        colors,
        vec![
            TabItem::new(
                "level-dat-mode-visual",
                "表单模式",
                state.mode == LevelDatEditorMode::Visual,
                {
                    let view_handle = view_handle.clone();
                    move |window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_level_dat_editor_mode(LevelDatEditorMode::Visual, window, cx);
                        });
                    }
                },
            ),
            TabItem::new(
                "level-dat-mode-json",
                "JSON 模式",
                state.mode == LevelDatEditorMode::Json,
                {
                    let view_handle = view_handle.clone();
                    move |window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_level_dat_editor_mode(LevelDatEditorMode::Json, window, cx);
                        });
                    }
                },
            ),
        ],
    )
    .height(if page_mode { px(34.) } else { px(38.) })
    .item_width(if page_mode { px(96.) } else { px(108.) });

    div()
        .px(if page_mode { px(16.) } else { px(22.) })
        .py(if page_mode { px(12.) } else { px(18.) })
        .border_b_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .flex()
        .items_start()
        .justify_between()
        .flex_wrap()
        .gap(px(18.))
        .child(
            div()
                .flex_1()
                .flex_basis(px(0.))
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(if page_mode { px(3.) } else { px(6.) })
                .child(
                    div()
                        .text_size(if page_mode { px(16.) } else { px(20.) })
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("Level.dat 编辑器"),
                )
                .child(
                    div().max_w(px(560.)).child(
                        MinecraftFormattedText::new(state.asset.display_name.clone(), colors)
                            .text_size(if page_mode { px(11.) } else { px(12.) })
                            .line_height(relative(1.2))
                            .color(colors.text_secondary)
                            .wrap(false),
                    ),
                )
                .when(!page_mode, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_wrap()
                            .child(subtle_badge(
                                colors,
                                format!("版本头 {}", state.document.version()),
                            ))
                            .when(!state.document.warnings.is_empty(), |child| {
                                child.child(toned_status_badge(
                                    colors,
                                    "头部长度已容错",
                                    colors.stat_orange_text,
                                ))
                            })
                            .child(subtle_badge(
                                colors,
                                format!("实例 {}", state.version.display_name()),
                            ))
                            .child(subtle_badge(colors, format!("{line_count} 行")))
                            .child(subtle_badge(colors, format!("{char_count} 字符")))
                            .when(dirty, |child| {
                                child.child(toned_status_badge(
                                    colors,
                                    "未保存",
                                    colors.stat_orange_text,
                                ))
                            })
                            .child(render_validation_badge(colors, &state.validation)),
                    )
                })
                .when(page_mode, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_wrap()
                            .child(subtle_badge(
                                colors,
                                format!("版本头 {}", state.document.version()),
                            ))
                            .when(!state.document.warnings.is_empty(), |child| {
                                child.child(toned_status_badge(
                                    colors,
                                    "头部长度已容错",
                                    colors.stat_orange_text,
                                ))
                            })
                            .child(subtle_badge(
                                colors,
                                format!("实例 {}", state.version.display_name()),
                            ))
                            .when(dirty, |child| {
                                child.child(toned_status_badge(
                                    colors,
                                    "未保存",
                                    colors.stat_orange_text,
                                ))
                            })
                            .when(!compact_json_page, |child| {
                                child.child(render_validation_badge(colors, &state.validation))
                            }),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .flex_wrap()
                .min_w(px(0.))
                .gap(px(10.))
                .when(page_mode, |this| {
                    this.child({
                        let view_handle = view_handle.clone();
                        compact_secondary_button(colors, "level-dat-back-to-list", "返回列表")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.return_from_level_dat_editor(cx);
                                });
                            })
                    })
                })
                .child({
                    let view_handle = view_handle.clone();
                    if page_mode {
                        compact_secondary_button(colors, "level-dat-launch-map", "启动地图")
                    } else {
                        secondary_button(colors, "level-dat-launch-map", "启动地图")
                    }
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.launch_level_map_from_editor(cx);
                        });
                    })
                })
                .when(compact_json_page, |this| {
                    this.child({
                        let view_handle = view_handle.clone();
                        compact_secondary_button(colors, "level-dat-format-json", "格式化")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.format_level_dat_editor(cx);
                                });
                            })
                    })
                })
                .child({
                    let view_handle = view_handle.clone();
                    if page_mode {
                        compact_secondary_button(colors, "level-dat-open-code-window", "新窗口代码")
                    } else {
                        secondary_button(colors, "level-dat-open-code-window", "新窗口代码")
                    }
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.open_level_dat_editor_code_window(cx);
                        });
                    })
                })
                .child(tabs),
        )
        .when(page_mode, |this| {
            this.child({
                let view_handle = view_handle.clone();
                compact_primary_button(
                    colors,
                    "level-dat-save-inline",
                    if state.saving {
                        "保存中..."
                    } else {
                        "保存 level.dat"
                    },
                )
                .opacity(if state.saving || !state.validation.valid {
                    0.72
                } else {
                    1.0
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.save_level_dat_editor(cx);
                    });
                })
            })
        })
}

fn render_visual_body(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> AnyElement {
    let sections = level_dat_schema::form_sections(&state.document);

    div()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(Hsla {
            a: 0.05,
            ..colors.surface
        })
        .overflow_y_scrollbar()
        .child(
            div()
                .px(if page_mode { px(16.) } else { px(20.) })
                .py(if page_mode { px(10.) } else { px(16.) })
                .flex()
                .flex_col()
                .gap(if page_mode { px(8.) } else { px(12.) })
                .children(sections.into_iter().map(|section| {
                    render_visual_section(colors, section, state, view_handle.clone(), page_mode)
                })),
        )
        .into_any_element()
}

fn render_json_body(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> AnyElement {
    if page_mode {
        return div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .px(px(8.))
            .pb(px(8.))
            .child(
                CodeEditor::new(&state.json_editor, colors)
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .w_full()
                    .h_full()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.14,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.98,
                        ..colors.surface
                    }),
            )
            .into_any_element();
    }

    panel_shell(colors)
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .m(if page_mode { px(10.) } else { px(18.) })
        .overflow_hidden()
        .child(
            div()
                .px(if page_mode { px(12.) } else { px(16.) })
                .py(if page_mode { px(8.) } else { px(12.) })
                .border_b_1()
                .border_color(Hsla {
                    a: 0.10,
                    ..colors.border
                })
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(toned_status_badge(colors, "level.dat.json", colors.accent))
                        .child(subtle_badge(colors, "NBT JSON")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(render_validation_badge(colors, &state.validation))
                        .child({
                            let view_handle = view_handle.clone();
                            if page_mode {
                                compact_secondary_button(colors, "level-dat-format-json", "格式化")
                            } else {
                                secondary_button(colors, "level-dat-format-json", "格式化")
                            }
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_, _, cx| {
                                    let _ = view_handle.update(cx, |this, cx| {
                                        this.format_level_dat_editor(cx);
                                    });
                                },
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .p(if page_mode { px(8.) } else { px(12.) })
                .child(
                    CodeEditor::new(&state.json_editor, colors)
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .w_full()
                        .h_full()
                        .rounded(px(16.))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.16,
                            ..colors.border
                        })
                        .bg(Hsla {
                            a: 0.98,
                            ..colors.surface
                        }),
                ),
        )
        .into_any_element()
}

fn render_footer(
    state: &LevelDatEditorModalState,
    colors: &ThemeColors,
    dirty: bool,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> Div {
    if page_mode {
        if state.mode == LevelDatEditorMode::Json && state.validation.valid && !dirty {
            return div().h(px(0.));
        }

        return div()
            .px(px(16.))
            .py(px(8.))
            .border_t_1()
            .border_color(Hsla {
                a: 0.12,
                ..colors.border
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_secondary)
                    .child(state.validation.detail.clone().unwrap_or_else(|| {
                        if !state.validation.valid {
                            SharedString::from("JSON 校验失败。")
                        } else if dirty {
                            SharedString::from("有未保存的修改。")
                        } else {
                            SharedString::from("当前内容已与 level.dat 同步。")
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(dirty, |this| {
                        this.child(toned_status_badge(
                            colors,
                            "未保存",
                            colors.stat_orange_text,
                        ))
                    })
                    .when(!state.validation.valid, |this| {
                        this.child(render_validation_badge(colors, &state.validation))
                    }),
            );
    }

    div()
        .px(px(22.))
        .py(px(14.))
        .border_t_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(state.validation.detail.clone().unwrap_or_else(|| {
                    if dirty {
                        SharedString::from("有未保存的修改。")
                    } else {
                        SharedString::from("当前内容已与 level.dat 同步。")
                    }
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child({
                    let view_handle = view_handle.clone();
                    secondary_button(
                        colors,
                        "level-dat-close",
                        if page_mode { "返回列表" } else { "关闭" },
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            if page_mode {
                                this.return_from_level_dat_editor(cx);
                            } else {
                                this.close_level_dat_editor(cx);
                            }
                        });
                    })
                })
                .child(
                    primary_button(
                        colors,
                        "level-dat-save",
                        if state.saving {
                            "保存中..."
                        } else {
                            "保存 level.dat"
                        },
                    )
                    .opacity(if state.saving || !state.validation.valid {
                        0.72
                    } else {
                        1.0
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.save_level_dat_editor(cx);
                        });
                    }),
                ),
        )
}

fn compact_secondary_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .cursor_pointer()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(label.into()),
        )
}

fn compact_primary_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(8.))
        .rounded(px(10.))
        .bg(colors.accent)
        .cursor_pointer()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .child(label.into()),
        )
}

fn render_visual_section(
    colors: &ThemeColors,
    section: LevelDatFieldSection,
    state: &LevelDatEditorModalState,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> Div {
    let collapsed = state.collapsed_groups.contains(&section.group);
    let group = section.group;
    let mut items = Vec::new();

    for choice in section.choices {
        items.push(
            render_choice_card(colors, choice, state, view_handle.clone(), page_mode)
                .into_any_element(),
        );
    }
    for value in section.values {
        items.push(
            render_value_card(
                colors,
                value,
                state,
                value.key == "FlatWorldLayers",
                page_mode,
            )
            .into_any_element(),
        );
    }
    for field in section.bools {
        items.push(
            render_bool_card(colors, field, state, view_handle.clone(), page_mode)
                .into_any_element(),
        );
    }

    div()
        .rounded(if page_mode { px(8.) } else { px(16.) })
        .border_1()
        .border_color(if page_mode {
            Hsla {
                a: 0.08,
                ..colors.border
            }
        } else {
            Hsla {
                a: 0.16,
                ..colors.border
            }
        })
        .bg(if page_mode {
            Hsla {
                a: 0.30,
                ..colors.surface
            }
        } else {
            colors.settings_panel_bg
        })
        .p(if page_mode { px(10.) } else { px(14.) })
        .flex()
        .flex_col()
        .gap(if page_mode { px(8.) } else { px(12.) })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .cursor_pointer()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(group.title()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .child(group.description()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_muted)
                        .child(if collapsed { "展开" } else { "收起" }),
                )
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.toggle_level_dat_group(group, cx);
                    });
                }),
        )
        .when(!collapsed, |this| this.child(render_field_grid(items)))
}

fn render_field_grid(items: Vec<AnyElement>) -> Div {
    div().flex().flex_wrap().gap(px(8.)).children(items)
}

fn render_choice_card(
    colors: &ThemeColors,
    field: ChoiceFieldSpec,
    state: &LevelDatEditorModalState,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> Div {
    let current_value = read_i32_value(&state.document, field.scope, field.key);
    let missing = current_value.is_none();

    field_group_shell(colors, page_mode)
        .flex_1()
        .flex_basis(px(220.))
        .min_w(px(0.))
        .p(if page_mode { px(9.) } else { px(12.) })
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(render_field_title(
            colors,
            field.label,
            field.key,
            field.description,
            missing,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(field.options.iter().map(|option| {
                    let selected = current_value.is_some_and(|value| value == option.value);
                    let option_label = option.label;
                    let option_value = option.value;
                    div()
                        .px(px(10.))
                        .h(px(30.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if selected {
                            Hsla {
                                a: 0.38,
                                ..colors.accent
                            }
                        } else {
                            Hsla {
                                a: 0.18,
                                ..colors.border
                            }
                        })
                        .bg(if selected {
                            Hsla {
                                a: 0.12,
                                ..colors.accent
                            }
                        } else {
                            Hsla {
                                a: 0.72,
                                ..colors.settings_field_bg
                            }
                        })
                        .text_size(px(12.))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if selected {
                            colors.text_primary
                        } else {
                            colors.text_secondary
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(option_label)
                        .on_mouse_down(MouseButton::Left, {
                            let view_handle = view_handle.clone();
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.set_level_dat_choice(field, option_value, cx);
                                });
                            }
                        })
                })),
        )
}

fn render_value_card(
    colors: &ThemeColors,
    field: ValueFieldSpec,
    state: &LevelDatEditorModalState,
    multiline: bool,
    page_mode: bool,
) -> Div {
    let input = state.form_inputs.get(&form_input_key(field)).cloned();
    let missing = !has_tag(&state.document, field.scope, field.key);

    field_group_shell(colors, page_mode)
        .flex_1()
        .flex_basis(px(if multiline { 460. } else { 220. }))
        .min_w(px(0.))
        .p(if page_mode { px(9.) } else { px(12.) })
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(render_field_title(
            colors,
            field.label,
            field.key,
            field.description,
            missing,
        ))
        .child(input.map_or_else(
            || render_missing_input_shell(colors, multiline).into_any_element(),
            |input| {
                Input::new(&input)
                    .w_full()
                    .h(if multiline { px(50.) } else { px(34.) })
                    .appearance(true)
                    .bordered(true)
                    .focus_bordered(true)
                    .into_any_element()
            },
        ))
}

fn render_bool_card(
    colors: &ThemeColors,
    field: BoolFieldSpec,
    state: &LevelDatEditorModalState,
    view_handle: WeakEntity<ManagePageView>,
    page_mode: bool,
) -> Div {
    let missing = !has_tag(&state.document, field.scope, field.key);

    field_group_shell(colors, page_mode)
        .flex_1()
        .flex_basis(px(260.))
        .min_w(px(0.))
        .p(if page_mode { px(9.) } else { px(12.) })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(card_title(colors, field.label))
                                .when(missing, |this| {
                                    this.child(
                                        div()
                                            .px(px(6.))
                                            .py(px(2.))
                                            .rounded(px(999.))
                                            .text_size(px(10.))
                                            .text_color(colors.text_muted)
                                            .bg(Hsla {
                                                a: 0.08,
                                                ..colors.border
                                            })
                                            .child("未包含"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .child(field.description),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(colors.text_muted)
                                .child(field.key),
                        ),
                )
                .child(ToggleSwitch::new(
                    SharedString::from(format!("level-dat-bool-{}", field.key)),
                    colors,
                    bool_enabled(&state.document, field),
                    {
                        let view_handle = view_handle.clone();
                        move |cx| {
                            let _ = view_handle.update(cx, |this, cx| {
                                this.toggle_level_dat_field(field, cx);
                            });
                        }
                    },
                )),
        )
}

fn render_missing_input_shell(colors: &ThemeColors, multiline: bool) -> Div {
    div()
        .w_full()
        .h(if multiline { px(50.) } else { px(34.) })
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
}

fn field_group_shell(colors: &ThemeColors, page_mode: bool) -> Div {
    div()
        .rounded(if page_mode { px(10.) } else { px(14.) })
        .border_1()
        .border_color(Hsla {
            a: if page_mode { 0.08 } else { 0.16 },
            ..colors.border
        })
        .bg(if page_mode {
            Hsla {
                a: 0.48,
                ..colors.surface
            }
        } else {
            colors.settings_panel_bg
        })
}

fn render_field_title(
    colors: &ThemeColors,
    label: &'static str,
    key: &'static str,
    description: &'static str,
    missing: bool,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(label),
                        )
                        .when(missing, |this| {
                            this.child(
                                div()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(999.))
                                    .text_size(px(10.))
                                    .text_color(colors.text_muted)
                                    .bg(Hsla {
                                        a: 0.08,
                                        ..colors.border
                                    })
                                    .child("未包含"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(colors.text_muted)
                        .child(format!("({key})")),
                ),
        )
        .child(
            div()
                .text_size(px(11.))
                .line_height(relative(1.2))
                .text_color(colors.text_muted)
                .child(description),
        )
}

fn render_validation_badge(colors: &ThemeColors, validation: &LevelDatJsonValidation) -> Div {
    if validation.valid {
        toned_status_badge(colors, "JSON 正确", colors.stat_green_text)
    } else {
        toned_status_badge(colors, "JSON 错误", colors.danger)
    }
}

fn toned_status_badge(_colors: &ThemeColors, label: &'static str, accent: Hsla) -> Div {
    div()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(999.))
        .bg(Hsla { a: 0.12, ..accent })
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(accent)
        .child(label)
}

pub fn form_value_fields(document: &LevelDatDocument) -> Vec<ValueFieldSpec> {
    level_dat_schema::form_value_fields(document)
}

pub fn form_input_key(spec: ValueFieldSpec) -> String {
    format!("{}:{}", scope_prefix(spec.scope), spec.key)
}

pub fn input_placeholder(spec: ValueFieldSpec) -> &'static str {
    match spec.kind {
        ValueFieldKind::String => "未包含时留空不写入；换行写为 \\n",
        ValueFieldKind::Int => "整数；留空表示不写入该字段",
        ValueFieldKind::Long => "长整数；留空表示不写入该字段",
        ValueFieldKind::Float => "小数；留空表示不写入该字段",
        ValueFieldKind::Version => "如 1.21.0；留空表示不写入",
    }
}

fn escape_input_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn unescape_input_string(value: &str) -> String {
    let mut unescaped = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }

        match characters.next() {
            Some('\\') => unescaped.push('\\'),
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            Some('t') => unescaped.push('\t'),
            Some(other) => {
                unescaped.push('\\');
                unescaped.push(other);
            }
            None => unescaped.push('\\'),
        }
    }
    unescaped
}

pub fn document_to_json_text(document: &LevelDatDocument) -> Result<SharedString, String> {
    serde_json::to_string_pretty(&document.root)
        .map(SharedString::from)
        .map_err(|error| format!("序列化 level.dat JSON 失败: {error}"))
}

pub fn format_document_json(tag: &NbtTag) -> Result<SharedString, String> {
    serde_json::to_string_pretty(tag)
        .map(SharedString::from)
        .map_err(|error| format!("格式化 JSON 失败: {error}"))
}

pub fn parse_document_json(text: &str) -> Result<NbtTag, LevelDatJsonValidation> {
    let tag: NbtTag = match serde_json::from_str(text) {
        Ok(tag) => tag,
        Err(error) => {
            return Err(LevelDatJsonValidation {
                valid: false,
                summary: SharedString::from("JSON 语法错误"),
                detail: Some(SharedString::from(format!(
                    "第 {} 行，第 {} 列: {error}",
                    error.line(),
                    error.column()
                ))),
            });
        }
    };

    if !matches!(tag, NbtTag::Compound(_)) {
        return Err(LevelDatJsonValidation {
            valid: false,
            summary: SharedString::from("根节点必须是 Compound"),
            detail: Some(SharedString::from(
                "level.dat 保存要求根节点使用 {\"Compound\": {...}} 结构。",
            )),
        });
    }

    Ok(tag)
}

pub fn validate_document_json(text: &str) -> LevelDatJsonValidation {
    match parse_document_json(text) {
        Ok(_) => LevelDatJsonValidation {
            valid: true,
            summary: SharedString::from("JSON 校验通过"),
            detail: Some(SharedString::from(
                "语法正确，且根节点可直接写回 level.dat。",
            )),
        },
        Err(validation) => validation,
    }
}

pub fn value_text(document: &LevelDatDocument, spec: ValueFieldSpec) -> SharedString {
    match spec.kind {
        ValueFieldKind::String => SharedString::from(escape_input_string(
            &read_string_value(document, spec.scope, spec.key).unwrap_or_default(),
        )),
        ValueFieldKind::Int => SharedString::from(
            read_i32_value(document, spec.scope, spec.key)
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        ValueFieldKind::Long => SharedString::from(
            read_i64_value(document, spec.scope, spec.key)
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        ValueFieldKind::Float => SharedString::from(
            read_f32_value(document, spec.scope, spec.key)
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        ValueFieldKind::Version => SharedString::from(
            read_version_value(document, spec.scope, spec.key).unwrap_or_default(),
        ),
    }
}

pub fn bool_enabled(document: &LevelDatDocument, spec: BoolFieldSpec) -> bool {
    read_bool_value(document, spec.scope, spec.key).unwrap_or(false)
}

pub fn toggle_bool(document: &mut LevelDatDocument, spec: BoolFieldSpec) {
    let next = !bool_enabled(document, spec);
    set_bool_value(document, spec.scope, spec.key, next);
}

pub fn set_choice_value(document: &mut LevelDatDocument, spec: ChoiceFieldSpec, value: i32) {
    set_i32_value(document, spec.scope, spec.key, value);
}

pub fn apply_value_text(
    document: &mut LevelDatDocument,
    spec: ValueFieldSpec,
    input: &str,
) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        if spec.kind == ValueFieldKind::String && has_tag(document, spec.scope, spec.key) {
            set_string_value(document, spec.scope, spec.key, unescape_input_string(input));
        } else {
            remove_tag(document, spec.scope, spec.key);
        }
        return Ok(());
    }

    match spec.kind {
        ValueFieldKind::String => {
            set_string_value(document, spec.scope, spec.key, unescape_input_string(input));
            Ok(())
        }
        ValueFieldKind::Int => {
            let parsed = trimmed
                .parse::<i32>()
                .map_err(|error| format!("{} 输入无效: {error}", spec.label))?;
            set_i32_value(document, spec.scope, spec.key, parsed);
            Ok(())
        }
        ValueFieldKind::Long => {
            let parsed = trimmed
                .parse::<i64>()
                .map_err(|error| format!("{} 输入无效: {error}", spec.label))?;
            set_i64_value(document, spec.scope, spec.key, parsed);
            Ok(())
        }
        ValueFieldKind::Float => {
            let parsed = trimmed
                .parse::<f32>()
                .map_err(|error| format!("{} 输入无效: {error}", spec.label))?;
            set_f32_value(document, spec.scope, spec.key, parsed);
            Ok(())
        }
        ValueFieldKind::Version => {
            let mut parts = Vec::new();
            for part in trimmed.split('.') {
                parts.push(
                    part.parse::<i32>()
                        .map_err(|error| format!("{} 版本段无效: {error}", spec.label))?,
                );
            }
            while parts.len() < 5 {
                parts.push(0);
            }
            let tags = parts
                .into_iter()
                .take(5)
                .map(NbtTag::Int)
                .collect::<Vec<_>>();
            set_tag_value(document, spec.scope, spec.key, NbtTag::List(tags));
            Ok(())
        }
    }
}

fn scope_prefix(scope: TagScope) -> &'static str {
    match scope {
        TagScope::Root => "root",
        TagScope::Abilities => "abilities",
    }
}

fn root_compound(document: &LevelDatDocument) -> Option<&IndexMap<String, NbtTag>> {
    match &document.root {
        NbtTag::Compound(map) => Some(map),
        _ => None,
    }
}

fn root_compound_mut(document: &mut LevelDatDocument) -> &mut IndexMap<String, NbtTag> {
    if !matches!(document.root, NbtTag::Compound(_)) {
        document.root = NbtTag::Compound(IndexMap::new());
    }
    match &mut document.root {
        NbtTag::Compound(map) => map,
        _ => unreachable!(),
    }
}

fn abilities_compound_mut(document: &mut LevelDatDocument) -> &mut IndexMap<String, NbtTag> {
    let root = root_compound_mut(document);
    let abilities = root
        .entry("abilities".to_string())
        .or_insert_with(|| NbtTag::Compound(IndexMap::new()));
    if !matches!(abilities, NbtTag::Compound(_)) {
        *abilities = NbtTag::Compound(IndexMap::new());
    }
    match abilities {
        NbtTag::Compound(map) => map,
        _ => unreachable!(),
    }
}

fn has_tag(document: &LevelDatDocument, scope: TagScope, key: &str) -> bool {
    read_tag(document, scope, key).is_some()
}

fn read_tag<'a>(document: &'a LevelDatDocument, scope: TagScope, key: &str) -> Option<&'a NbtTag> {
    match scope {
        TagScope::Root => root_compound(document)?.get(key),
        TagScope::Abilities => match root_compound(document)?.get("abilities")? {
            NbtTag::Compound(map) => map.get(key),
            _ => None,
        },
    }
}

fn set_tag_value(document: &mut LevelDatDocument, scope: TagScope, key: &str, value: NbtTag) {
    match scope {
        TagScope::Root => {
            root_compound_mut(document).insert(key.to_string(), value);
        }
        TagScope::Abilities => {
            abilities_compound_mut(document).insert(key.to_string(), value);
        }
    }
}

fn remove_tag(document: &mut LevelDatDocument, scope: TagScope, key: &str) {
    match scope {
        TagScope::Root => {
            root_compound_mut(document).shift_remove(key);
        }
        TagScope::Abilities => {
            abilities_compound_mut(document).shift_remove(key);
        }
    }
}

fn read_bool_value(document: &LevelDatDocument, scope: TagScope, key: &str) -> Option<bool> {
    match read_tag(document, scope, key)? {
        NbtTag::Byte(value) => Some(*value != 0),
        NbtTag::Short(value) => Some(*value != 0),
        NbtTag::Int(value) => Some(*value != 0),
        NbtTag::Long(value) => Some(*value != 0),
        _ => None,
    }
}

fn set_bool_value(document: &mut LevelDatDocument, scope: TagScope, key: &str, value: bool) {
    set_tag_value(
        document,
        scope,
        key,
        NbtTag::Byte(if value { 1 } else { 0 }),
    );
}

fn read_i32_value(document: &LevelDatDocument, scope: TagScope, key: &str) -> Option<i32> {
    match read_tag(document, scope, key)? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn set_i32_value(document: &mut LevelDatDocument, scope: TagScope, key: &str, value: i32) {
    set_tag_value(document, scope, key, NbtTag::Int(value));
}

fn read_i64_value(document: &LevelDatDocument, scope: TagScope, key: &str) -> Option<i64> {
    match read_tag(document, scope, key)? {
        NbtTag::Byte(value) => Some(i64::from(*value)),
        NbtTag::Short(value) => Some(i64::from(*value)),
        NbtTag::Int(value) => Some(i64::from(*value)),
        NbtTag::Long(value) => Some(*value),
        _ => None,
    }
}

fn set_i64_value(document: &mut LevelDatDocument, scope: TagScope, key: &str, value: i64) {
    set_tag_value(document, scope, key, NbtTag::Long(value));
}

fn read_f32_value(document: &LevelDatDocument, scope: TagScope, key: &str) -> Option<f32> {
    match read_tag(document, scope, key)? {
        NbtTag::Float(value) => Some(*value),
        NbtTag::Double(value) => Some(*value as f32),
        NbtTag::Int(value) => Some(*value as f32),
        NbtTag::Long(value) => Some(*value as f32),
        _ => None,
    }
}

fn set_f32_value(document: &mut LevelDatDocument, scope: TagScope, key: &str, value: f32) {
    set_tag_value(document, scope, key, NbtTag::Float(value));
}

fn read_string_value(document: &LevelDatDocument, scope: TagScope, key: &str) -> Option<String> {
    match read_tag(document, scope, key)? {
        NbtTag::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn set_string_value(document: &mut LevelDatDocument, scope: TagScope, key: &str, value: String) {
    set_tag_value(document, scope, key, NbtTag::String(value));
}

fn read_version_value(document: &LevelDatDocument, scope: TagScope, key: &str) -> Option<String> {
    match read_tag(document, scope, key)? {
        NbtTag::List(values) => Some(
            values
                .iter()
                .map(|value| match value {
                    NbtTag::Int(value) => value.to_string(),
                    NbtTag::Short(value) => value.to_string(),
                    NbtTag::Byte(value) => value.to_string(),
                    NbtTag::Long(value) => value.to_string(),
                    _ => "0".to_string(),
                })
                .collect::<Vec<_>>()
                .join("."),
        ),
        _ => None,
    }
}
