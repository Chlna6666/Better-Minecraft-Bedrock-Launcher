use crate::{
    App, Bounds, DispatchPhase, GlobalElementId, Hitbox, MouseMoveEvent, Pixels, Size, Style,
    StyleRefinement, Window, px,
};

use super::state::Interactivity;

/// Interactivity state displayed an manipulated in the inspector.
#[derive(Clone)]
pub struct DivInspectorState {
    /// The inspected element's base style. This is used for both inspecting and modifying the
    /// state. In the future it will make sense to separate the read and write, possibly tracking
    /// the modifications.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub base_style: Box<StyleRefinement>,
    /// Inspects the bounds of the element.
    pub bounds: Bounds<Pixels>,
    /// Size of the children of the element, or `bounds.size` if it has no children.
    pub content_size: Size<Pixels>,
}

impl Interactivity {
    /// Create an `Interactivity`, capturing the caller location in debug mode.
    #[cfg(any(feature = "inspector", debug_assertions))]
    #[track_caller]
    pub fn new() -> Interactivity {
        Interactivity {
            source_location: Some(core::panic::Location::caller()),
            ..Default::default()
        }
    }

    /// Create an `Interactivity`, capturing the caller location in debug mode.
    #[cfg(not(any(feature = "inspector", debug_assertions)))]
    pub fn new() -> Interactivity {
        Interactivity::default()
    }

    /// Gets the source location of construction. Returns `None` when not in debug mode.
    pub fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            self.source_location
        }

        #[cfg(not(any(feature = "inspector", debug_assertions)))]
        {
            None
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn paint_debug_info(
        &self,
        global_id: Option<&GlobalElementId>,
        hitbox: &Hitbox,
        style: &Style,
        window: &mut Window,
        cx: &mut App,
    ) {
        use crate::{BorderStyle, TextAlign};

        if let Some(global_id) = global_id
            && (style.debug || style.debug_below || cx.has_global::<crate::DebugBelow>())
            && hitbox.is_hovered(window)
        {
            const FONT_SIZE: crate::Pixels = crate::Pixels(10.);
            let element_id = format!("{global_id:?}");
            let str_len = element_id.len();

            let render_debug_text = |window: &mut Window| {
                if let Some(text) = window
                    .text_system()
                    .shape_text(
                        element_id.into(),
                        FONT_SIZE,
                        &[window.text_style().to_run(str_len)],
                        None,
                        None,
                    )
                    .ok()
                    .and_then(|mut text| text.pop())
                {
                    text.paint(hitbox.origin, FONT_SIZE, TextAlign::Left, None, window, cx)
                        .ok();

                    let text_bounds = crate::Bounds {
                        origin: hitbox.origin,
                        size: text.size(FONT_SIZE),
                    };
                    if let Some(location) = self.source_location
                        && text_bounds.contains(&window.mouse_position())
                        && window.modifiers().secondary()
                    {
                        let secondary_held = window.modifiers().secondary();
                        window.on_key_event({
                            move |e: &crate::ModifiersChangedEvent, _phase, window, _cx| {
                                if e.modifiers.secondary() != secondary_held
                                    && text_bounds.contains(&window.mouse_position())
                                {
                                    window.refresh();
                                }
                            }
                        });

                        let was_hovered = hitbox.is_hovered(window);
                        let current_view = window.current_view();
                        window.on_mouse_event({
                            let hitbox = hitbox.clone();
                            move |_: &MouseMoveEvent, phase, window, cx| {
                                if phase == DispatchPhase::Capture {
                                    let hovered = hitbox.is_hovered(window);
                                    if hovered != was_hovered {
                                        cx.notify(current_view);
                                    }
                                }
                            }
                        });

                        window.on_mouse_event({
                            let hitbox = hitbox.clone();
                            move |e: &crate::MouseDownEvent, phase, window, cx| {
                                if text_bounds.contains(&e.position)
                                    && phase.capture()
                                    && hitbox.is_hovered(window)
                                {
                                    cx.stop_propagation();
                                    let Ok(dir) = std::env::current_dir() else {
                                        return;
                                    };

                                    eprintln!(
                                        "This element was created at:\n{}:{}:{}",
                                        dir.join(location.file()).to_string_lossy(),
                                        location.line(),
                                        location.column()
                                    );
                                }
                            }
                        });
                        window.paint_quad(crate::outline(
                            crate::Bounds {
                                origin: hitbox.origin
                                    + crate::point(crate::px(0.), FONT_SIZE - px(2.)),
                                size: crate::Size {
                                    width: text_bounds.size.width,
                                    height: crate::px(1.),
                                },
                            },
                            crate::red(),
                            BorderStyle::default(),
                        ));
                    }
                }
            };

            window.with_text_style(
                Some(crate::TextStyleRefinement {
                    color: Some(crate::red()),
                    line_height: Some(FONT_SIZE.into()),
                    background_color: Some(crate::white()),
                    ..Default::default()
                }),
                render_debug_text,
            );
        }
    }

    pub(crate) fn sync_inspector_layout_state(
        &mut self,
        inspector_id: Option<&crate::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(any(feature = "inspector", debug_assertions))]
        window.with_inspector_state(
            inspector_id,
            cx,
            |inspector_state: &mut Option<DivInspectorState>, _window| {
                if let Some(inspector_state) = inspector_state {
                    self.base_style = inspector_state.base_style.clone();
                } else {
                    *inspector_state = Some(DivInspectorState {
                        base_style: self.base_style.clone(),
                        bounds: Default::default(),
                        content_size: Default::default(),
                    });
                }
            },
        );
    }

    pub(crate) fn sync_inspector_prepaint_state(
        &self,
        inspector_id: Option<&crate::InspectorElementId>,
        bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(any(feature = "inspector", debug_assertions))]
        window.with_inspector_state(
            inspector_id,
            cx,
            |inspector_state: &mut Option<DivInspectorState>, _window| {
                if let Some(inspector_state) = inspector_state {
                    inspector_state.bounds = bounds;
                    inspector_state.content_size = content_size;
                }
            },
        );
    }
}
