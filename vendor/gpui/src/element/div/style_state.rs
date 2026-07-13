use crate::{
    App, DispatchPhase, Global, GlobalElementId, Hitbox, HitboxId, MouseMoveEvent, SharedString,
    Style, Window, record_style_refine,
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

use super::frame_state::InteractiveElementState;
use super::state::Interactivity;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ComputedStyleKey {
    pub(crate) in_focus: bool,
    pub(crate) focused: bool,
    pub(crate) hovered: bool,
    pub(crate) group_hovered: bool,
    pub(crate) active: bool,
    pub(crate) group_active: bool,
    pub(crate) has_active_drag: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ComputedStyleCache {
    pub(crate) key: ComputedStyleKey,
    pub(crate) style: Style,
}

impl Interactivity {
    fn computed_style_key(
        &self,
        hitbox: Option<&Hitbox>,
        element_state: Option<&mut InteractiveElementState>,
        window: &mut Window,
        cx: &mut App,
    ) -> ComputedStyleKey {
        let mut key = ComputedStyleKey {
            has_active_drag: cx.has_active_drag(),
            ..Default::default()
        };

        if let Some(focus_handle) = self.tracked_focus_handle.as_ref() {
            key.in_focus = self.in_focus_style.is_some() && focus_handle.within_focused(window, cx);
            key.focused = self.focus_style.is_some() && focus_handle.is_focused(window);
        }

        if let Some(hitbox) = hitbox {
            if !key.has_active_drag {
                key.group_hovered = self
                    .group_hover_style
                    .as_ref()
                    .and_then(|group_hover| GroupHitboxes::get(&group_hover.group, cx))
                    .is_some_and(|group_hitbox_id| group_hitbox_id.is_hovered(window));
                key.hovered = self.hover_style.is_some() && hitbox.is_hovered(window);
            }
        }

        if let Some(element_state) = element_state
            && let Some(clicked_state) = element_state.clicked_state.as_ref()
        {
            let clicked_state = clicked_state.borrow();
            key.group_active = self.group_active_style.is_some() && clicked_state.group;
            key.active = self.active_style.is_some() && clicked_state.element;
        }

        key
    }

    pub(crate) fn paint_hover_group_handler(&self, window: &mut Window, cx: &mut App) {
        let group_hitbox = self
            .group_hover_style
            .as_ref()
            .and_then(|group_hover| GroupHitboxes::get(&group_hover.group, cx));

        if let Some(group_hitbox) = group_hitbox {
            let was_hovered = group_hitbox.is_hovered(window);
            let current_view = window.current_view();
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let hovered = group_hitbox.is_hovered(window);
                if phase == DispatchPhase::Capture && hovered != was_hovered {
                    cx.notify(current_view);
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
            let mut element_state =
                element_state.map(|element_state| element_state.unwrap_or_default());
            let style = self.compute_style_internal(hitbox, element_state.as_mut(), window, cx);
            (style, element_state)
        })
    }

    /// Called from internal methods that have already called with_element_state.
    pub(crate) fn compute_style_internal(
        &mut self,
        hitbox: Option<&Hitbox>,
        mut element_state: Option<&mut InteractiveElementState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Style {
        let cache_key = self.computed_style_key(hitbox, element_state.as_deref_mut(), window, cx);

        if !cache_key.has_active_drag
            && let Some(ComputedStyleCache { key, style }) = &self.computed_style_cache
            && *key == cache_key
        {
            return style.clone();
        }

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
                    style.refine(hover_style);
                }
            }

            self.apply_drag_over_styles(hitbox, &mut style, window, cx);
        }

        if let Some(element_state) = element_state {
            let clicked_state_handle = element_state.ensure_clicked_state();
            let clicked_state = clicked_state_handle.borrow();
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

        self.computed_style_cache = (!cache_key.has_active_drag).then_some(ComputedStyleCache {
            key: cache_key,
            style: style.clone(),
        });

        style
    }
}

#[cfg(test)]
mod tests;
