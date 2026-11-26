use crate::{
    App, Bounds, DispatchPhase, Global, GlobalElementId, Hitbox, HitboxId,
    MouseExitEvent, MouseMoveEvent, Pixels, SharedString, Style, Window, record_style_refine,
};
use collections::HashMap;
use refineable::Refineable;
use smallvec::SmallVec;

#[derive(Default)]
pub(crate) struct GroupHitboxes(pub(crate) HashMap<SharedString, SmallVec<[HitboxId; 1]>>);

impl Global for GroupHitboxes {}

impl GroupHitboxes {
    pub(crate) fn get(name: &SharedString, cx: &mut App) -> Option<HitboxId> {
        cx.default_global::<Self>()
            .0
            .get(name)
            .and_then(|bounds_stack| bounds_stack.last())
            .cloned()
    }

    pub(crate) fn push(name: SharedString, hitbox_id: HitboxId, cx: &mut App) {
        cx.default_global::<Self>()
            .0
            .entry(name)
            .or_default()
            .push(hitbox_id);
    }

    pub(crate) fn pop(name: &SharedString, cx: &mut App) {
        cx.default_global::<Self>().0.get_mut(name).unwrap().pop();
    }
}

use super::interactivity::Interactivity;
use super::state::InteractiveElementState;

impl Interactivity {
    pub(crate) fn paint_hover_group_handler(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut group_hitboxes = SmallVec::<[HitboxId; 4]>::new();
        if let Some(group_hitbox) = self
            .group_hover_style
            .as_ref()
            .and_then(|group_hover| GroupHitboxes::get(&group_hover.group, cx))
        {
            group_hitboxes.push(group_hitbox);
        }

        // Once a drag is active, group drag-over styles have the same hit-test dependency as
        // ordinary group-hover styles. Register their exact group hitboxes so pointer movement can
        // invalidate only affected retained regions instead of requiring a Window::refresh.
        if let Some(active_drag) = cx.active_drag.as_ref() {
            let drag_type = active_drag.value.as_ref().type_id();
            for (state_type, group_drag_style) in &self.group_drag_over_styles {
                if *state_type != drag_type {
                    continue;
                }
                if let Some(group_hitbox) = GroupHitboxes::get(&group_drag_style.group, cx)
                    && !group_hitboxes.contains(&group_hitbox)
                {
                    group_hitboxes.push(group_hitbox);
                }
            }
        }

        let current_view = window.rendered_entity_stack.last().copied();

        for group_hitbox in group_hitboxes {
            let was_hovered = group_hitbox.is_hovered(window);


            if was_hovered {

                window.on_mouse_event(move |_: &MouseExitEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        if let Some(current_view) = current_view {
                            window.notify_interactive_region(
                                current_view,

                                bounds,

                                cx,
                            );
                        } else {
                            window.refresh();
                        }
                    }
                });
            }

            window.on_mouse_hit_test_transition(move |_: &MouseMoveEvent, phase, window, cx| {
                let hovered = group_hitbox.is_hovered(window);
                if phase == DispatchPhase::Capture && hovered != was_hovered {
                    if let Some(current_view) = current_view {
                        window.notify_interactive_region(
                            current_view,

                            bounds,

                            cx,
                        );
                    } else {
                        window.refresh();
                    }
                }
            });
        }
    }

    /// Compute the visual style for this element, based on the current bounds and the element's state.
    pub fn compute_style(
        &mut self,
        global_id: Option<&GlobalElementId>,
        hitbox: Option<&Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) -> Style {
        window.with_optional_element_state(global_id, |element_state, window| {
            let element_state =
                element_state.map(|element_state| element_state.unwrap_or_default());
            let style = self.compute_style_internal(hitbox, element_state.as_ref(), window, cx);
            (style, element_state)
        })
    }

    /// Called from internal methods that have already called with_element_state.
    pub(crate) fn compute_style_internal(
        &self,
        hitbox: Option<&Hitbox>,
        element_state: Option<&InteractiveElementState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Style {
        // Follow GPUI Fast's direct style resolution. The Div stores its
        // refinements and resolves them against the current interaction state.
        let mut style = Style::default();
        record_style_refine(1);
        style.refine(&self.base_style);

        if let Some(focus_handle) = self.tracked_focus_handle.as_ref() {
            if let Some(in_focus_style) = self.in_focus_style.as_ref()
                && focus_handle.within_focused(window, cx)
            {
                record_style_refine(1);
                style.refine(in_focus_style);
            }

            if let Some(focus_style) = self.focus_style.as_ref()
                && focus_handle.is_focused(window)
            {
                record_style_refine(1);
                style.refine(focus_style);
            }
        }

        if let Some(hitbox) = hitbox {
            if !cx.has_active_drag() {
                if let Some(group_hover) = self.group_hover_style.as_ref()
                    && let Some(group_hitbox_id) = GroupHitboxes::get(&group_hover.group, cx)
                    && group_hitbox_id.is_hovered(window)
                {
                    record_style_refine(1);
                    style.refine(&group_hover.style);
                }

                if let Some(hover_style) = self.hover_style.as_ref()
                    && hitbox.is_hovered(window)
                {
                    record_style_refine(1);
                    style.refine(hover_style)
                }
            }

            self.apply_drag_over_styles(hitbox, &mut style, window, cx);
        }

        if let Some(clicked_state) = element_state.and_then(|state| state.clicked_state.as_ref()) {
            let clicked_state = clicked_state.borrow();
            if clicked_state.group
                && let Some(group) = self.group_active_style.as_ref()
            {
                record_style_refine(1);
                style.refine(&group.style)
            }

            if let Some(active_style) = self.active_style.as_ref()
                && clicked_state.element
            {
                record_style_refine(1);
                style.refine(active_style)
            }
        }

        style
    }
}

#[cfg(test)]
mod tests;
