use crate::{
    AnyView, AnyWindowHandle, AppContext, AsyncApp, DispatchPhase, Effect, EntityId, EventEmitter,
    FocusHandle, FocusOutEvent, Focusable, Global, KeystrokeObserver, Reservation, SubscriberSet,
    Subscription, Task, WeakEntity, WeakFocusHandle, Window, WindowHandle,
};
use anyhow::Result;
use futures::FutureExt;
use std::{
    any::{Any, TypeId},
    borrow::{Borrow, BorrowMut},
    future::Future,
    ops,
    sync::Arc,
};
use util::Deferred;

use super::{App, AsyncWindowContext, Entity, KeystrokeEvent};

mod window;

/// The app context, with specialized behavior for the given entity.
pub struct Context<'a, T> {
    app: &'a mut App,
    entity_state: WeakEntity<T>,
}

impl<'a, T> ops::Deref for Context<'a, T> {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

impl<'a, T> ops::DerefMut for Context<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.app
    }
}

impl<'a, T: 'static> Context<'a, T> {
    pub(crate) fn new_context(app: &'a mut App, entity_state: WeakEntity<T>) -> Self {
        Self { app, entity_state }
    }

    /// The entity id of the entity backing this context.
    pub fn entity_id(&self) -> EntityId {
        self.entity_state.entity_id
    }

    /// Returns a handle to the entity belonging to this context.
    pub fn entity(&self) -> Entity<T> {
        self.weak_entity()
            .upgrade()
            .expect("The entity must be alive if we have a entity context")
    }

    /// Returns a weak handle to the entity belonging to this context.
    pub fn weak_entity(&self) -> WeakEntity<T> {
        self.entity_state.clone()
    }

    /// Arranges for the given function to be called whenever [`Context::notify`] is
    /// called with the given entity.
    pub fn observe<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(&mut T, Entity<W>, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static,
        W: 'static,
    {
        let this = self.weak_entity();
        self.app.observe_internal(entity, move |e, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| on_notify(this, e, cx));
                true
            } else {
                false
            }
        })
    }

    /// Observe changes to ourselves
    pub fn observe_self(
        &mut self,
        mut on_event: impl FnMut(&mut T, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let this = self.entity();
        self.app.observe(&this, move |this, cx| {
            this.update(cx, |this, cx| on_event(this, cx))
        })
    }

    /// Subscribe to an event type from another entity
    pub fn subscribe<T2, Evt>(
        &mut self,
        entity: &Entity<T2>,
        mut on_event: impl FnMut(&mut T, Entity<T2>, &Evt, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static,
        T2: 'static + EventEmitter<Evt>,
        Evt: 'static,
    {
        let this = self.weak_entity();
        self.app.subscribe_internal(entity, move |e, event, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| on_event(this, e, event, cx));
                true
            } else {
                false
            }
        })
    }

    /// Subscribe to an event type from ourself
    pub fn subscribe_self<Evt>(
        &mut self,
        mut on_event: impl FnMut(&mut T, &Evt, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static + EventEmitter<Evt>,
        Evt: 'static,
    {
        let this = self.entity();
        self.app.subscribe(&this, move |this, evt, cx| {
            this.update(cx, |this, cx| on_event(this, evt, cx))
        })
    }

    /// Register a callback to be invoked when GPUI releases this entity.
    pub fn on_release(&self, on_release: impl FnOnce(&mut T, &mut App) + 'static) -> Subscription
    where
        T: 'static,
    {
        let (subscription, activate) = self.app.release_listeners.insert(
            self.entity_state.entity_id,
            Box::new(move |this, cx| {
                let this = this.downcast_mut().expect("invalid entity type");
                on_release(this, cx);
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to be run on the release of another entity
    pub fn observe_release<T2>(
        &self,
        entity: &Entity<T2>,
        on_release: impl FnOnce(&mut T, &mut T2, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: Any,
        T2: 'static,
    {
        let entity_id = entity.entity_id();
        let this = self.weak_entity();
        let (subscription, activate) = self.app.release_listeners.insert(
            entity_id,
            Box::new(move |entity, cx| {
                let entity = entity.downcast_mut().expect("invalid entity type");
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, cx| on_release(this, entity, cx));
                }
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to for updates to the given global
    pub fn observe_global<G: 'static>(
        &mut self,
        mut f: impl FnMut(&mut T, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let handle = self.weak_entity();
        let (subscription, activate) = self.global_observers.insert(
            TypeId::of::<G>(),
            Box::new(move |cx| handle.update(cx, |view, cx| f(view, cx)).is_ok()),
        );
        self.defer(move |_| activate());
        subscription
    }

    /// Register a callback to be invoked when the application is about to restart.
    pub fn on_app_restart(
        &self,
        mut on_restart: impl FnMut(&mut T, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let handle = self.weak_entity();
        self.app.on_app_restart(move |cx| {
            handle.update(cx, |entity, cx| on_restart(entity, cx)).ok();
        })
    }

    /// Arrange for the given function to be invoked whenever the application is quit.
    /// The future returned from this callback will be polled for up to [crate::SHUTDOWN_TIMEOUT] until the app fully quits.
    pub fn on_app_quit<Fut>(
        &self,
        mut on_quit: impl FnMut(&mut T, &mut Context<T>) -> Fut + 'static,
    ) -> Subscription
    where
        Fut: 'static + Future<Output = ()>,
        T: 'static,
    {
        let handle = self.weak_entity();
        self.app.on_app_quit(move |cx| {
            let future = handle.update(cx, |entity, cx| on_quit(entity, cx)).ok();
            async move {
                if let Some(future) = future {
                    future.await;
                }
            }
            .boxed_local()
        })
    }

    /// Tell GPUI that this entity has changed and observers of it should be notified.
    pub fn notify(&mut self) {
        self.app.notify(self.entity_state.entity_id);
    }

    /// Spawn the future returned by the given function.
    /// The function is provided a weak handle to the entity owned by this context and a context that can be held across await points.
    /// The returned task must be held or detached.
    #[track_caller]
    pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        T: 'static,
        AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        let this = self.weak_entity();
        self.app.spawn(async move |cx| f(this, cx).await)
    }

    /// Run something using this entity and cx, when the returned struct is dropped
    pub fn on_drop(
        &self,
        f: impl FnOnce(&mut T, &mut Context<T>) + 'static,
    ) -> Deferred<impl FnOnce()> {
        let this = self.weak_entity();
        let mut cx = self.to_async();
        util::defer(move || {
            this.update(&mut cx, f).ok();
        })
    }
}

impl<T> Context<'_, T> {
    /// Emit an event of the specified type, which can be handled by other entities that have subscribed via `subscribe` methods on their respective contexts.
    pub fn emit<Evt>(&mut self, event: Evt)
    where
        T: EventEmitter<Evt>,
        Evt: 'static,
    {
        self.app.pending_effects.push_back(Effect::Emit {
            emitter: self.entity_state.entity_id,
            event_type: TypeId::of::<Evt>(),
            event: Box::new(event),
        });
    }
}

impl<T> AppContext for Context<'_, T> {
    type Result<U> = U;

    fn new<U: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<U>) -> U) -> Entity<U> {
        self.app.new(build_entity)
    }

    fn reserve_entity<U: 'static>(&mut self) -> Reservation<U> {
        self.app.reserve_entity()
    }

    fn insert_entity<U: 'static>(
        &mut self,
        reservation: Reservation<U>,
        build_entity: impl FnOnce(&mut Context<U>) -> U,
    ) -> Self::Result<Entity<U>> {
        self.app.insert_entity(reservation, build_entity)
    }

    fn update_entity<U: 'static, R>(
        &mut self,
        handle: &Entity<U>,
        update: impl FnOnce(&mut U, &mut Context<U>) -> R,
    ) -> R {
        self.app.update_entity(handle, update)
    }

    fn as_mut<'a, E>(&'a mut self, handle: &Entity<E>) -> Self::Result<super::GpuiBorrow<'a, E>>
    where
        E: 'static,
    {
        self.app.as_mut(handle)
    }

    fn read_entity<U, R>(
        &self,
        handle: &Entity<U>,
        read: impl FnOnce(&U, &App) -> R,
    ) -> Self::Result<R>
    where
        U: 'static,
    {
        self.app.read_entity(handle, read)
    }

    fn update_window<R, F>(&mut self, window: AnyWindowHandle, update: F) -> Result<R>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> R,
    {
        self.app.update_window(window, update)
    }

    fn read_window<U, R>(
        &self,
        window: &WindowHandle<U>,
        read: impl FnOnce(Entity<U>, &App) -> R,
    ) -> Result<R>
    where
        U: 'static,
    {
        self.app.read_window(window, read)
    }

    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.app.background_executor.spawn(future)
    }

    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> Self::Result<R>
    where
        G: Global,
    {
        self.app.read_global(callback)
    }
}

impl<T> Borrow<App> for Context<'_, T> {
    fn borrow(&self) -> &App {
        self.app
    }
}

impl<T> BorrowMut<App> for Context<'_, T> {
    fn borrow_mut(&mut self) -> &mut App {
        self.app
    }
}
