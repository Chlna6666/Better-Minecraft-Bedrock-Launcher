use crate::ui::theme::colors::parse_hex_color_to_hsla;
use crate::ui::window::debug::state::{DebugInspectorSnapshot, DebugState};
use gpui::*;

fn format_rgba_hex(color: Hsla) -> SharedString {
    let rgba: Rgba = color.into();
    let r = (rgba.r * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = (rgba.g * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = (rgba.b * 255.0).round().clamp(0.0, 255.0) as u8;
    SharedString::from(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn format_background(background: Background) -> SharedString {
    SharedString::from(format!("{background:?}"))
}

fn snapshot_active_div_state(
    active_id: Option<&InspectorElementId>,
    window: &mut Window,
    cx: &mut App,
) -> DebugInspectorSnapshot {
    let mut snapshot = DebugInspectorSnapshot {
        enabled: cfg!(debug_assertions),
        picking: window.is_inspector_picking(cx),
        ..DebugInspectorSnapshot::default()
    };

    #[cfg(debug_assertions)]
    {
        snapshot.selected_id = active_id.cloned();
    }

    #[cfg(debug_assertions)]
    if let Some(active_id) = active_id {
        snapshot.selected_label = SharedString::from(format!(
            "instance={} global={}",
            active_id.instance_id, active_id.path.global_id
        ));
        snapshot.source_location = SharedString::from(format!(
            "{}:{}",
            active_id.path.source_location.file(),
            active_id.path.source_location.line()
        ));

        window.with_inspector_state(
            Some(active_id),
            cx,
            |state: &mut Option<gpui::DivInspectorState>, _window| {
                if let Some(state) = state.as_ref() {
                    snapshot.bounds_label = SharedString::from(format!(
                        "{:.0}x{:.0} @ {:.0},{:.0}",
                        f32::from(state.bounds.size.width),
                        f32::from(state.bounds.size.height),
                        f32::from(state.bounds.origin.x),
                        f32::from(state.bounds.origin.y)
                    ));
                    snapshot.content_size_label = SharedString::from(format!(
                        "{:.0}x{:.0}",
                        f32::from(state.content_size.width),
                        f32::from(state.content_size.height)
                    ));
                    if let Some(fill) = state.base_style.background.as_ref()
                        && let Some(background) = fill.color()
                    {
                        snapshot.background_hex = format_background(background);
                    }
                    if let Some(border) = state.base_style.border_color {
                        snapshot.border_hex = format_rgba_hex(border);
                    }
                    snapshot.opacity = state.base_style.opacity;
                }
            },
        );
    }

    #[cfg(not(debug_assertions))]
    let _ = active_id;

    snapshot
}

pub fn configure_devtools(cx: &mut App) {
    #[cfg(debug_assertions)]
    {
        cx.register_inspector_element::<gpui::DivInspectorState, _>(|_id, state, _window, _cx| {
            let background = state
                .base_style
                .background
                .as_ref()
                .and_then(|fill| fill.color())
                .map(format_background)
                .unwrap_or_else(|| SharedString::from("(none)"));
            let border = state
                .base_style
                .border_color
                .map(format_rgba_hex)
                .unwrap_or_else(|| SharedString::from("(none)"));
            let opacity = state
                .base_style
                .opacity
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "1.00".to_string());

            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .child("Div"),
                )
                .child(div().text_size(px(11.)).child(format!(
                    "bounds: {:.0}x{:.0}",
                    f32::from(state.bounds.size.width),
                    f32::from(state.bounds.size.height)
                )))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(format!("background: {background}")),
                )
                .child(div().text_size(px(11.)).child(format!("border: {border}")))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(format!("opacity: {opacity}")),
                )
        });

        cx.set_inspector_renderer(Box::new(|inspector, window, cx| {
            let active_id = inspector.active_element_id().cloned();
            let snapshot = snapshot_active_div_state(active_id.as_ref(), window, cx);
            cx.update_global(|debug: &mut DebugState, _cx| {
                debug.sync_inspector(snapshot.clone());
            });
            let _ = inspector.render_inspector_states(window, cx);
            cx.refresh_windows();
            Empty.into_any_element()
        }));
    }
}

pub fn find_window_by_id(target_id: Option<u64>, cx: &App) -> Option<AnyWindowHandle> {
    let target_id = target_id?;
    cx.windows()
        .into_iter()
        .find(|window| window.window_id().as_u64() == target_id)
}

pub fn debug_window_is_open(cx: &App) -> bool {
    find_window_by_id(
        cx.read_global(|debug: &DebugState, _cx| debug.debug_window_id),
        cx,
    )
    .is_some()
}

pub fn toggle_main_window_inspector(cx: &mut App) {
    tracing::debug!("debug devtools action: toggle_main_window_inspector");
    let main_window = find_window_by_id(
        cx.read_global(|debug: &DebugState, _cx| debug.main_window_id),
        cx,
    );
    if let Some(main_window) = main_window {
        let _ = main_window.update(cx, |_root, window, cx| {
            #[cfg(debug_assertions)]
            window.toggle_inspector(cx);
        });
    }
    cx.refresh_windows();
}

pub fn begin_main_window_pick(cx: &mut App) {
    tracing::debug!("debug devtools action: begin_main_window_pick");
    let main_window = find_window_by_id(
        cx.read_global(|debug: &DebugState, _cx| debug.main_window_id),
        cx,
    );
    if let Some(main_window) = main_window {
        let _ = main_window.update(cx, |_root, window, cx| {
            if !window.is_inspector_picking(cx) {
                #[cfg(debug_assertions)]
                {
                    if cx.read_global(|debug: &DebugState, _cx| debug.inspector.enabled) {
                        window.toggle_inspector(cx);
                    }
                    window.toggle_inspector(cx);
                }
            }
        });
    }
    cx.refresh_windows();
}

#[cfg(debug_assertions)]
fn with_selected_div_state(cx: &mut App, update: impl FnOnce(&mut gpui::DivInspectorState)) {
    let (main_window_id, selected_id) = cx.read_global(|debug: &DebugState, _cx| {
        (debug.main_window_id, debug.inspector.selected_id.clone())
    });
    let Some(selected_id) = selected_id else {
        return;
    };
    let main_window = find_window_by_id(main_window_id, cx);
    if let Some(main_window) = main_window {
        let _ = main_window.update(cx, |_root, window, cx| {
            window.with_inspector_state(
                Some(&selected_id),
                cx,
                |state: &mut Option<gpui::DivInspectorState>, window| {
                    if let Some(state) = state.as_mut() {
                        update(state);
                        window.refresh();
                    }
                },
            );
        });
    }
}

pub fn set_selected_element_opacity(cx: &mut App, opacity: f32) {
    tracing::debug!("debug devtools action: set_selected_element_opacity opacity={opacity:.2}");
    #[cfg(debug_assertions)]
    with_selected_div_state(cx, move |state| {
        state.base_style.opacity = Some(opacity.clamp(0.0, 1.0));
    });
    cx.refresh_windows();
}

pub fn set_selected_element_background(cx: &mut App, hex: &str) {
    tracing::debug!("debug devtools action: set_selected_element_background hex={hex}");
    #[cfg(debug_assertions)]
    if let Some(color) = parse_hex_color_to_hsla(hex) {
        with_selected_div_state(cx, move |state| {
            state.base_style.background = Some(color.into());
        });
    }
    cx.refresh_windows();
}

pub fn clear_selected_element_background(cx: &mut App) {
    tracing::debug!("debug devtools action: clear_selected_element_background");
    #[cfg(debug_assertions)]
    with_selected_div_state(cx, move |state| {
        state.base_style.background = None;
    });
    cx.refresh_windows();
}

pub fn reset_selected_element_styles(cx: &mut App) {
    tracing::debug!("debug devtools action: reset_selected_element_styles");
    #[cfg(debug_assertions)]
    with_selected_div_state(cx, move |state| {
        state.base_style.background = None;
        state.base_style.opacity = Some(1.0);
    });
    cx.refresh_windows();
}

pub fn select_inspector_history_entry(index: usize, cx: &mut App) {
    tracing::debug!("debug devtools action: select_inspector_history_entry index={index}");
    #[cfg(debug_assertions)]
    {
        let (main_window_id, selected_id) = cx.read_global(|debug: &DebugState, _cx| {
            (
                debug.main_window_id,
                debug
                    .inspector_history
                    .get(index)
                    .and_then(|entry| entry.selected_id.clone()),
            )
        });
        let Some(selected_id) = selected_id else {
            return;
        };
        let main_window = find_window_by_id(main_window_id, cx);
        if let Some(main_window) = main_window {
            let _ = main_window.update(cx, |_root, window, cx| {
                let snapshot = snapshot_active_div_state(Some(&selected_id), window, cx);
                cx.update_global(|debug: &mut DebugState, _cx| {
                    debug.sync_inspector(snapshot);
                });
            });
        }
    }
    cx.refresh_windows();

    #[cfg(not(debug_assertions))]
    let _ = (index, cx);
}
