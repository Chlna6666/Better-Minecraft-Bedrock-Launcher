use super::*;

impl Window {
    /// The nominal refresh interval of this window's display, cached by App.
    /// Returns `None` when the display is disconnected or its mode is unknown.
    pub fn refresh_interval(&self, cx: &App) -> Option<Duration> {
        cx.display_refresh_interval(self.display_id?)
    }

    /// Observes moves between displays and changes to the current display's properties.
    /// [`Self::refresh_interval`] returns the updated value when the callback runs.
    pub fn observe_window_display(
        &self,
        mut callback: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        self.display_observers.insert(Box::new(move |window, cx| {
            callback(window, cx);
            true
        }))
    }

    pub(crate) fn notify_display_observers(&mut self, cx: &mut App) {
        self.display_observers
            .clone()
            .retain(|callback| callback(self, cx));
    }

    pub(super) fn defer_display_observers(&mut self, cx: &mut App) {
        if self.display_notification_pending {
            return;
        }
        self.display_notification_pending = true;
        let handle = self.handle;
        cx.spawn(async move |cx| {
            let _ = cx.update(|cx| {
                // Reconcile outside the active window borrow: observers can safely
                // inspect other windows and receive the newest display metadata.
                cx.displays_changed();
                let _ = handle.update(cx, |_, window, cx| {
                    window.display_notification_pending = false;
                    window.notify_display_observers(cx);
                });
            });
        })
        .detach();
    }
}
