use std::rc::Rc;

use crate::{DisplayId, PlatformDisplay, WindowAppearance};

use super::App;

impl App {
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
