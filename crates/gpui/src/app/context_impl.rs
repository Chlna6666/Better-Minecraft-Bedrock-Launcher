use std::{any::{Any, TypeId, type_name}, future::Future};

use anyhow::{Context as _, Result, anyhow};

use crate::{
    AnyEntity, AnyView, AnyWindowHandle, App, AppContext, Context, Effect, Entity, Global,
    GpuiBorrow, Reservation, Task, Window, WindowHandle,
};

impl App {
    #[inline(never)]
    fn update_entity_erased(
        &mut self,
        handle: &AnyEntity,
        entity_type: &str,
        update: &mut dyn FnMut(&mut dyn Any, &mut App),
    ) {
        self.update(|cx| {
            let mut lease = cx.entities.lease_erased(handle, entity_type);
            update(
                lease
                    .entity
                    .as_deref_mut()
                    .expect("active entity lease must contain its entity"),
                cx,
            );
            cx.entities.end_lease_erased(handle.entity_id(), lease);
        });
    }
}

impl AppContext for App {
    type Result<T> = T;

    /// Builds an entity that is owned by the application.
    ///
    /// The given function will be invoked with a [`Context`] and must return an object representing the entity. An
    /// [`Entity`] handle will be returned, which can be used to access the entity in a context.
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        self.update(|cx| {
            let slot = cx.entities.reserve();
            let handle = slot.clone();
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));

            cx.push_effect(Effect::EntityCreated {
                entity: handle.clone().into_any(),
                tid: TypeId::of::<T>(),
                window: cx.window_update_stack.last().cloned(),
            });

            cx.entities.insert(slot, entity);
            handle
        })
    }

    fn reserve_entity<T: 'static>(&mut self) -> Self::Result<Reservation<T>> {
        Reservation(self.entities.reserve())
    }

    fn insert_entity<T: 'static>(
        &mut self,
        reservation: Reservation<T>,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Self::Result<Entity<T>> {
        self.update(|cx| {
            let slot = reservation.0;
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));
            cx.entities.insert(slot, entity)
        })
    }

    /// Updates the entity referenced by the given handle. The function is passed a mutable reference to the
    /// entity along with a `Context` for the entity.
    #[inline(always)]
    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let mut update = Some(update);
        let mut result = None;
        self.update_entity_erased(handle, type_name::<T>(), &mut |entity, cx| {
            result = Some(
                update.take().expect("entity update callback runs once")(
                    entity
                        .downcast_mut::<T>()
                        .expect("entity type must match its typed handle"),
                    &mut Context::new_context(cx, handle.downgrade()),
                ),
            );
        });
        result.expect("entity update callback produces a result")
    }

    fn as_mut<'a, T>(&'a mut self, handle: &Entity<T>) -> GpuiBorrow<'a, T>
    where
        T: 'static,
    {
        GpuiBorrow::new(handle.clone(), self)
    }

    fn read_entity<T, R>(
        &self,
        handle: &Entity<T>,
        read: impl FnOnce(&T, &App) -> R,
    ) -> Self::Result<R>
    where
        T: 'static,
    {
        let entity = self.entities.read(handle);
        read(entity, self)
    }

    fn update_window<T, F>(&mut self, handle: AnyWindowHandle, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.update_window_id(handle.id, update)
    }

    fn read_window<T, R>(
        &self,
        window: &WindowHandle<T>,
        read: impl FnOnce(Entity<T>, &App) -> R,
    ) -> Result<R>
    where
        T: 'static,
    {
        let window = self
            .windows
            .get(window.id)
            .context("window not found")?
            .as_deref()
            .expect("attempted to read a window that is already on the stack");

        let root_view = window.root.clone().unwrap();
        let view = root_view
            .downcast::<T>()
            .map_err(|_| anyhow!("root view's type has changed"))?;

        Ok(read(view, self))
    }

    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_executor.spawn(future)
    }

    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> Self::Result<R>
    where
        G: Global,
    {
        let mut g = self.global::<G>();
        callback(g, self)
    }
}
