use super::*;

impl<'a, T: 'static> Context<'a, T> {
    /// Convenience method for accessing view state in an event callback.
    ///
    /// Many GPUI callbacks take the form of `Fn(&E, &mut Window, &mut App)`,
    /// but it's often useful to be able to access view state in these
    /// callbacks. This method provides a convenient way to do so.
    pub fn listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = self.entity().downgrade();
        move |e: &E, window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| f(view, e, window, cx)).ok();
        }
    }

    /// Convenience method for producing view state in a closure.
    /// See `listener` for more details.
    pub fn processor<E, R>(
        &self,
        f: impl Fn(&mut T, E, &mut Window, &mut Context<T>) -> R + 'static,
    ) -> impl Fn(E, &mut Window, &mut App) -> R + 'static {
        let view = self.entity();
        move |e: E, window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| f(view, e, window, cx))
        }
    }

    /// Focus the given view in the given window. View type is required to implement Focusable.
    pub fn focus_view<W: Focusable>(&mut self, view: &Entity<W>, window: &mut Window) {
        window.focus(&view.focus_handle(self));
    }

    /// Sets a given callback to be run on the next frame.
    pub fn on_next_frame(
        &self,
        window: &mut Window,
        f: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) where
        T: 'static,
    {
        let view = self.entity();
        window.on_next_frame(move |window, cx| view.update(cx, |view, cx| f(view, window, cx)));
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer_in(
        &mut self,
        window: &Window,
        f: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) {
        let view = self.entity();
        window.defer(self, move |window, cx| {
            view.update(cx, |view, cx| f(view, window, cx))
        });
    }

    /// Observe another entity for changes to its state, as tracked by [`Context::notify`].
    pub fn observe_in<V2>(
        &mut self,
        observed: &Entity<V2>,
        window: &mut Window,
        mut on_notify: impl FnMut(&mut T, Entity<V2>, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        V2: 'static,
        T: 'static,
    {
        let observed_id = observed.entity_id();
        let observed = observed.downgrade();
        let window_handle = window.handle;
        let observer = self.weak_entity();
        self.new_observer(
            observed_id,
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| {
                        if let Some((observer, observed)) =
                            observer.upgrade().zip(observed.upgrade())
                        {
                            observer.update(cx, |observer, cx| {
                                on_notify(observer, observed, window, cx);
                            });
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false)
            }),
        )
    }

    /// Subscribe to events emitted by another entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked with a reference to the current view, a handle to the emitting `Entity`, the event, a mutable reference to the `Window`, and the context for the entity.
    pub fn subscribe_in<Emitter, Evt>(
        &mut self,
        emitter: &Entity<Emitter>,
        window: &Window,
        mut on_event: impl FnMut(&mut T, &Entity<Emitter>, &Evt, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        Emitter: EventEmitter<Evt>,
        Evt: 'static,
    {
        let emitter = emitter.downgrade();
        let window_handle = window.handle;
        let subscriber = self.weak_entity();
        self.new_subscription(
            emitter.entity_id(),
            (
                TypeId::of::<Evt>(),
                Box::new(move |event, cx| {
                    window_handle
                        .update(cx, |_, window, cx| {
                            if let Some((subscriber, emitter)) =
                                subscriber.upgrade().zip(emitter.upgrade())
                            {
                                let event = event.downcast_ref().expect("invalid event type");
                                subscriber.update(cx, |subscriber, cx| {
                                    on_event(subscriber, &emitter, event, window, cx);
                                });
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                }),
            ),
        )
    }

    /// Register a callback to be invoked when the view is released.
    ///
    /// The callback receives a handle to the view's window. This handle may be
    /// invalid, if the window was closed before the view was released.
    pub fn on_release_in(
        &mut self,
        window: &Window,
        on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let entity = self.entity();
        self.app.observe_release_in(&entity, window, on_release)
    }

    /// Register a callback to be invoked when the given Entity is released.
    pub fn observe_release_in<T2>(
        &self,
        observed: &Entity<T2>,
        window: &Window,
        mut on_release: impl FnMut(&mut T, &mut T2, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static,
        T2: 'static,
    {
        let observer = self.weak_entity();
        self.app
            .observe_release_in(observed, window, move |observed, window, cx| {
                observer
                    .update(cx, |observer, cx| {
                        on_release(observer, observed, window, cx)
                    })
                    .ok();
            })
    }

    /// Register a callback to be invoked when the window is resized.
    pub fn observe_window_bounds(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.bounds_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to be invoked when the window is activated or deactivated.
    pub fn observe_window_activation(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.activation_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Registers a callback to be invoked when the window appearance changes.
    pub fn observe_window_appearance(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.appearance_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to be invoked when a keystroke is received by the application
    /// in any window. Note that this fires after all other action and event mechanisms have resolved
    /// and that this API will not be invoked if the event's propagation is stopped.
    pub fn observe_keystrokes(
        &mut self,
        mut f: impl FnMut(&mut T, &KeystrokeEvent, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        fn inner(
            keystroke_observers: &SubscriberSet<(), KeystrokeObserver>,
            handler: KeystrokeObserver,
        ) -> Subscription {
            let (subscription, activate) = keystroke_observers.insert((), handler);
            activate();
            subscription
        }

        let view = self.weak_entity();
        inner(
            &self.keystroke_observers,
            Box::new(move |event, window, cx| {
                if let Some(view) = view.upgrade() {
                    view.update(cx, |view, cx| f(view, event, window, cx));
                    true
                } else {
                    false
                }
            }),
        )
    }

    /// Register a callback to be invoked when the window's pending input changes.
    pub fn observe_pending_input(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.pending_input_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Register a listener to be called when the given focus handle receives focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus(
        &mut self,
        handle: &FocusHandle,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let focus_id = handle.id;
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                view.update(cx, |view, cx| {
                    if event.previous_focus_path.last() != Some(&focus_id)
                        && event.current_focus_path.last() == Some(&focus_id)
                    {
                        listener(view, window, cx)
                    }
                })
                .is_ok()
            }));
        self.defer(|_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle or one of its descendants receives focus.
    /// This does not fire if the given focus handle - or one of its descendants - was previously focused.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_in(
        &mut self,
        handle: &FocusHandle,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let focus_id = handle.id;
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                view.update(cx, |view, cx| {
                    if event.is_focus_in(focus_id) {
                        listener(view, window, cx)
                    }
                })
                .is_ok()
            }));
        self.defer(|_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_blur(
        &mut self,
        handle: &FocusHandle,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let focus_id = handle.id;
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                view.update(cx, |view, cx| {
                    if event.previous_focus_path.last() == Some(&focus_id)
                        && event.current_focus_path.last() != Some(&focus_id)
                    {
                        listener(view, window, cx)
                    }
                })
                .is_ok()
            }));
        self.defer(|_| activate());
        subscription
    }

    /// Register a listener to be called when nothing in the window has focus.
    /// This typically happens when the node that was focused is removed from the tree,
    /// and this callback lets you chose a default place to restore the users focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_lost(
        &mut self,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.focus_lost_listeners.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| listener(view, window, cx))
                    .is_ok()
            }),
        );
        self.defer(|_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle or one of its descendants loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_out(
        &mut self,
        handle: &FocusHandle,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, FocusOutEvent, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let focus_id = handle.id;
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                view.update(cx, |view, cx| {
                    if let Some(blurred_id) = event.previous_focus_path.last().copied()
                        && event.is_focus_out(focus_id)
                    {
                        let event = FocusOutEvent {
                            blurred: WeakFocusHandle {
                                id: blurred_id,
                                handles: Arc::downgrade(&cx.focus_handles),
                            },
                        };
                        listener(view, event, window, cx)
                    }
                })
                .is_ok()
            }));
        self.defer(|_| activate());
        subscription
    }

    /// Schedule a future to be run asynchronously.
    /// The given callback is invoked with a [`WeakEntity<V>`] to avoid leaking the entity for a long-running process.
    /// It's also given an [`AsyncWindowContext`], which can be used to access the state of the entity across await points.
    /// The returned future will be polled on the main thread.
    #[track_caller]
    pub fn spawn_in<AsyncFn, R>(&self, window: &Window, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncWindowContext) -> R + 'static,
    {
        let view = self.weak_entity();
        window.spawn(self, async move |cx| f(view, cx).await)
    }

    /// Register a callback to be invoked when the given global state changes.
    pub fn observe_global_in<G: Global>(
        &mut self,
        window: &Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let window_handle = window.handle;
        let view = self.weak_entity();
        let (subscription, activate) = self.global_observers.insert(
            TypeId::of::<G>(),
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| {
                        view.update(cx, |view, cx| f(view, window, cx)).is_ok()
                    })
                    .unwrap_or(false)
            }),
        );
        self.defer(move |_| activate());
        subscription
    }

    /// Register a callback to be invoked when the given Action type is dispatched to the window.
    pub fn on_action(
        &mut self,
        action_type: TypeId,
        window: &mut Window,
        listener: impl Fn(&mut T, &dyn Any, DispatchPhase, &mut Window, &mut Context<T>) + 'static,
    ) {
        let handle = self.weak_entity();
        window.on_action(action_type, move |action, phase, window, cx| {
            handle
                .update(cx, |view, cx| {
                    listener(view, action, phase, window, cx);
                })
                .ok();
        });
    }

    /// Move focus to the current view, assuming it implements [`Focusable`].
    pub fn focus_self(&mut self, window: &mut Window)
    where
        T: Focusable,
    {
        let view = self.entity();
        window.defer(self, move |window, cx| {
            view.read(cx).focus_handle(cx).focus(window)
        })
    }
}
