use super::*;

impl App {
    /// Returns handles to all open windows in the application.
    /// Each handle could be downcast to a handle typed for the root view of that window.
    /// To find all windows of a given type, you could filter on
    pub fn windows(&self) -> Vec<AnyWindowHandle> {
        self.windows
            .keys()
            .flat_map(|window_id| self.window_handles.get(&window_id).copied())
            .collect()
    }

    /// Returns the window handles ordered by their appearance on screen, front to back.
    ///
    /// The first window in the returned list is the active/topmost window of the application.
    ///
    /// This method returns None if the platform doesn't implement the method yet.
    pub fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        self.platform.window_stack()
    }

    /// Returns a handle to the window that is currently focused at the platform level, if one exists.
    pub fn active_window(&self) -> Option<AnyWindowHandle> {
        self.platform.active_window()
    }

    /// Opens a new window with the given option and the root view returned by the given function.
    /// The function is invoked with a `Window`, which can be used to interact with window-specific
    /// functionality.
    pub fn open_window<V: 'static + Render>(
        &mut self,
        options: crate::WindowOptions,
        build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> anyhow::Result<WindowHandle<V>> {
        self.update(|cx| {
            let id = cx.windows.insert(None);
            let handle = WindowHandle::new(id);
            match Window::new(handle.into(), options, cx) {
                Ok(mut window) => {
                    cx.window_update_stack.push(id);
                    let root_view = build_root_view(&mut window, cx);
                    cx.window_update_stack.pop();
                    window.root.replace(root_view.into());
                    window.defer(cx, |window: &mut Window, cx| window.appearance_changed(cx));

                    cx.window_handles.insert(id, window.handle);
                    let window_slot = cx
                        .windows
                        .get_mut(id)
                        .ok_or_else(|| anyhow::anyhow!("newly inserted window slot missing"))?;
                    window_slot.replace(Box::new(window));
                    window_slot
                        .as_deref_mut()
                        .ok_or_else(|| anyhow::anyhow!("newly inserted window missing"))?
                        .request_initial_frame();
                    Ok(handle)
                }
                Err(e) => {
                    cx.windows.remove(id);
                    Err(e)
                }
            }
        })
    }

    #[inline(always)]
    pub(in crate::app) fn update_window_id<T, F>(&mut self, id: WindowId, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        let mut update = Some(update);
        let mut result = None;
        self.update_window_erased(id, &mut |arguments| {
            if let Some((root_view, window, cx)) = arguments {
                result = Some(update.take().expect("window update callback runs once")(
                    root_view, window, cx,
                ));
            } else {
                drop(update.take());
                drop(result.take());
            }
        });
        result.ok_or_else(|| anyhow!("window not found"))
    }

    #[inline(never)]
    fn update_window_erased(
        &mut self,
        id: WindowId,
        update: &mut dyn FnMut(Option<(AnyView, &mut Window, &mut App)>),
    ) {
        self.update(|cx| {
            let Some(mut window) = cx.windows.get_mut(id).and_then(Option::take) else {
                update(None);
                return;
            };

            let root_view = window.root.clone().unwrap();

            cx.window_update_stack.push(window.handle.id);
            update(Some((root_view, &mut window, cx)));
            cx.window_update_stack.pop();

            if window.removed {
                cx.window_handles.remove(&id);
                cx.windows.remove(id);

                cx.window_closed_observers.clone().retain(&(), |callback| {
                    callback(cx);
                    true
                });
            } else if let Some(window_slot) = cx.windows.get_mut(id) {
                window_slot.replace(window);
            } else {
                update(None);
            }
        });
    }
}
