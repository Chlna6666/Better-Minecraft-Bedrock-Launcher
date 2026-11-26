use std::{rc::Rc, time::Duration};

use crate::{
    Bounds, DisplayChanges, DisplayEvent, DisplayId, Pixels, Platform, PlatformDisplay,
    Subscription, WindowAppearance,
};
use collections::FxHashMap;
use util::ResultExt;

use super::App;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct DisplayState {
    bounds: Bounds<Pixels>,
    refresh_interval: Option<Duration>,
}

pub(super) fn read_displays(platform: &dyn Platform) -> FxHashMap<DisplayId, DisplayState> {
    platform
        .displays()
        .into_iter()
        .map(|display| {
            (
                display.id(),
                DisplayState {
                    bounds: display.bounds(),
                    refresh_interval: display.refresh_interval(),
                },
            )
        })
        .collect()
}

fn display_events(
    previous: &FxHashMap<DisplayId, DisplayState>,
    current: &FxHashMap<DisplayId, DisplayState>,
) -> Vec<DisplayEvent> {
    let removed = previous
        .keys()
        .filter(|id| !current.contains_key(id))
        .map(|id| DisplayEvent::Removed(*id));
    let added_or_changed = current.iter().filter_map(|(id, state)| {
        let Some(previous) = previous.get(id) else {
            return Some(DisplayEvent::Added(*id));
        };
        let mut changes = DisplayChanges::empty();
        changes.set(DisplayChanges::BOUNDS, state.bounds != previous.bounds);
        changes.set(
            DisplayChanges::REFRESH_INTERVAL,
            state.refresh_interval != previous.refresh_interval,
        );
        (!changes.is_empty()).then_some(DisplayEvent::Changed { id: *id, changes })
    });
    removed.chain(added_or_changed).collect()
}

impl App {
    pub(crate) fn displays_changed(&mut self) {
        let current = read_displays(self.platform.as_ref());
        let events = display_events(&self.display_states, &current);
        self.display_states = current;
        if events.is_empty() {
            return;
        }
        self.notify_display_windows(&events);
        for event in events {
            self.display_observers
                .clone()
                .retain(|callback| callback(event, self));
        }
    }

    fn notify_display_windows(&mut self, events: &[DisplayEvent]) {
        // A native notification can arrive while one window is being updated.
        // That window reconciles its display before notifying its own observers.
        for handle in self.windows() {
            if !self
                .windows
                .get(handle.window_id())
                .is_some_and(Option::is_some)
            {
                continue;
            }
            handle
                .update(self, |_, window, cx| {
                    let previous_id = window.display_id;
                    window.window_origin_changed(cx);
                    if previous_id == window.display_id
                        && !window.display_notification_pending
                        && events.iter().any(|event| match event {
                            DisplayEvent::Changed { id, .. }
                            | DisplayEvent::Removed(id)
                            | DisplayEvent::Added(id) => Some(*id) == window.display_id,
                        })
                    {
                        window.notify_display_observers(cx);
                    }
                })
                .log_err();
        }
    }

    pub(crate) fn display_refresh_interval(&self, id: DisplayId) -> Option<Duration> {
        self.display_states.get(&id)?.refresh_interval
    }

    /// Observes display connection, disconnection, bounds and refresh interval changes.
    /// Cached display state is updated before observers run.
    pub fn observe_displays(
        &self,
        mut callback: impl FnMut(DisplayEvent, &mut App) + 'static,
    ) -> Subscription {
        self.display_observers.insert(Box::new(move |event, cx| {
            callback(event, cx);
            true
        }))
    }
    /// Return the currently active displays.
    pub fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.platform.displays()
    }

    /// Return the primary display used for new windows.
    pub fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.platform.primary_display()
    }

    /// Return the display with the given ID.
    pub fn find_display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        self.displays()
            .into_iter()
            .find(|display| display.id() == id)
    }

    /// Return the appearance of application windows.
    pub fn window_appearance(&self) -> WindowAppearance {
        self.platform.window_appearance()
    }

    /// Return the Linux compositor name, or an empty string on other platforms.
    pub fn compositor_name(&self) -> &'static str {
        self.platform.compositor_name()
    }

    /// Return whether platform scrollbars are configured to auto-hide.
    pub fn should_auto_hide_scrollbars(&self) -> bool {
        self.platform.should_auto_hide_scrollbars()
    }
}
