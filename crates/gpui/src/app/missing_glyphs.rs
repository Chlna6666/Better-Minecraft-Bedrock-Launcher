use super::App;
use crate::{MissingGlyph, Subscription};
use std::{cell::RefCell, rc::Rc};

type MissingGlyphCallback = Box<dyn FnMut(&[MissingGlyph], &mut App) + 'static>;

struct MissingGlyphCallbackEntry {
    registration: Rc<()>,
    callback: Option<MissingGlyphCallback>,
}

#[derive(Default)]
pub(super) struct MissingGlyphCallbackSlot {
    entry: RefCell<Option<MissingGlyphCallbackEntry>>,
}

impl MissingGlyphCallbackSlot {
    fn replace(&self, callback: MissingGlyphCallback) -> Rc<()> {
        let registration = Rc::new(());
        self.entry.borrow_mut().replace(MissingGlyphCallbackEntry {
            registration: registration.clone(),
            callback: Some(callback),
        });
        registration
    }

    fn invoke(&self, missing_glyphs: &[MissingGlyph], cx: &mut App) {
        let Some((registration, mut callback)) =
            self.entry.borrow_mut().as_mut().and_then(|entry| {
                entry
                    .callback
                    .take()
                    .map(|callback| (entry.registration.clone(), callback))
            })
        else {
            return;
        };

        callback(missing_glyphs, cx);

        let mut entry = self.entry.borrow_mut();
        let is_current = entry
            .as_ref()
            .is_some_and(|entry| Rc::ptr_eq(&entry.registration, &registration));
        if is_current {
            let Some(entry) = entry.as_mut() else {
                return;
            };
            entry.callback = Some(callback);
        }
    }

    fn remove(&self, registration: &Rc<()>) -> bool {
        let mut entry = self.entry.borrow_mut();
        let is_current = entry
            .as_ref()
            .is_some_and(|entry| Rc::ptr_eq(&entry.registration, registration));
        if is_current {
            entry.take();
        }
        is_current
    }
}

impl App {
    /// Registers the application callback for grapheme clusters that exhausted font fallback.
    ///
    /// Registering a callback replaces the previous registration. Reports are delivered on the
    /// foreground executor after platform shaping has released its internal locks. Dropping the
    /// active subscription disables platform reporting until another callback is registered.
    ///
    /// The callback may install fonts through TextSystem::add_fonts and then refresh affected
    /// windows. Installing fonts advances the shared font generation, so stale frame, retained
    /// layout and in-flight shaping results cannot repopulate the caches.
    pub fn on_missing_glyphs(
        &self,
        callback: impl FnMut(&[MissingGlyph], &mut App) + 'static,
    ) -> Subscription {
        let registration = self.missing_glyph_callback.replace(Box::new(callback));

        if let Some(mut receiver) = self.text_system.take_missing_glyph_receiver() {
            let callback = self.missing_glyph_callback.clone();
            self.spawn(async move |cx| {
                while let Ok(missing_glyphs) = receiver.recv().await {
                    if cx.update(|cx| callback.invoke(&missing_glyphs, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        self.text_system.enable_missing_glyph_reporting();

        let callback = self.missing_glyph_callback.clone();
        let text_system = self.text_system.clone();
        Subscription::new(move || {
            if callback.remove(&registration) {
                text_system.disable_missing_glyph_reporting();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FallbackFontClass, TestAppContext};
    use std::{cell::RefCell, rc::Rc};

    fn missing(grapheme: &'static str) -> MissingGlyph {
        MissingGlyph::new(grapheme.into(), FallbackFontClass::Proportional)
    }

    #[gpui::test]
    fn missing_glyph_callback_follows_active_subscription(cx: &mut TestAppContext) {
        let first = Rc::new(RefCell::new(Vec::new()));
        let first_subscription = cx.update(|cx| {
            let first = first.clone();
            cx.on_missing_glyphs(move |missing_glyphs, _| {
                first.borrow_mut().extend_from_slice(missing_glyphs);
            })
        });

        cx.update(|cx| {
            cx.text_system()
                .report_missing_glyphs_in_test(vec![missing("first")]);
        });
        cx.run_until_parked();
        assert_eq!(first.borrow().as_slice(), &[missing("first")]);

        let second = Rc::new(RefCell::new(Vec::new()));
        let second_subscription = cx.update(|cx| {
            let second = second.clone();
            cx.on_missing_glyphs(move |missing_glyphs, _| {
                second.borrow_mut().extend_from_slice(missing_glyphs);
            })
        });

        drop(first_subscription);
        cx.update(|cx| {
            cx.text_system()
                .report_missing_glyphs_in_test(vec![missing("second")]);
        });
        cx.run_until_parked();

        assert_eq!(first.borrow().as_slice(), &[missing("first")]);
        assert_eq!(second.borrow().as_slice(), &[missing("second")]);

        drop(second_subscription);
        cx.update(|cx| {
            cx.text_system()
                .report_missing_glyphs_in_test(vec![missing("disabled")]);
        });
        cx.run_until_parked();
        assert_eq!(second.borrow().as_slice(), &[missing("second")]);
    }
}
