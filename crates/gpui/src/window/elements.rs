use super::lifecycle::RetainedInvalidationScope;
use super::*;

/// Pending asset wakeups are coalesced per asset *and* per window.
///
/// Window identity is part of the key because an asset can become ready before a window has
/// finished publishing its entity -> invalidator registrations. Keeping one completion task per
/// observing window lets the callback invalidate that exact window directly instead of relying on
/// a global entity notification having already discovered the window.
type AssetViewSubscriptionId = (TypeId, u64, u64);

#[derive(Default)]
struct AssetViewSubscriptions {
    next_generation: u64,
    pending: collections::FxHashMap<AssetViewSubscriptionId, PendingAssetViewSubscription>,
}

struct PendingAssetViewSubscription {
    generation: u64,
    views: collections::FxHashSet<EntityId>,
}

fn asset_view_subscriptions(cx: &mut App) -> &mut AssetViewSubscriptions {
    cx.globals_by_type
        .entry(TypeId::of::<AssetViewSubscriptions>())
        .or_insert_with(|| Box::new(AssetViewSubscriptions::default()))
        .downcast_mut::<AssetViewSubscriptions>()
        .expect("asset view subscription state type mismatch")
}

fn subscribe_asset_view(
    cx: &mut App,
    subscription_id: AssetViewSubscriptionId,
    view: EntityId,
    new_load: bool,
) -> Option<u64> {
    let state = asset_view_subscriptions(cx);

    if !new_load
        && let Some(pending) = state.pending.get_mut(&subscription_id)
    {
        pending.views.insert(view);
        return None;
    }

    let generation = state.next_generation;
    state.next_generation = state
        .next_generation
        .checked_add(1)
        .expect("asset view subscription generation overflow");
    let mut views = collections::FxHashSet::default();
    views.insert(view);
    state.pending.insert(
        subscription_id,
        PendingAssetViewSubscription {
            generation,
            views,
        },
    );
    Some(generation)
}

fn finish_asset_view_subscription(
    cx: &mut App,
    subscription_id: AssetViewSubscriptionId,
    generation: u64,
) -> collections::FxHashSet<EntityId> {
    let state = asset_view_subscriptions(cx);
    if state
        .pending
        .get(&subscription_id)
        .is_none_or(|pending| pending.generation != generation)
    {
        return collections::FxHashSet::default();
    }

    state
        .pending
        .remove(&subscription_id)
        .map_or_else(collections::FxHashSet::default, |pending| pending.views)
}

impl Window {
    /// Invalidates the exact views waiting for newly available asset pixels and immediately asks
    /// the platform for a presentation-capable frame.
    ///
    /// This bypasses the App-level entity -> window lookup deliberately: an asset may finish during
    /// a window's first frame before that lookup has been published. The invalidation remains
    /// view-scoped, so unrelated cached views can still retain and replay their previous ranges.
    pub(crate) fn schedule_asset_ready_views(
        &mut self,
        views: impl IntoIterator<Item = EntityId>,
    ) {
        let mut any = false;
        for entity_id in views {
            any = true;
            let _ = self.invalidator.invalidate_retained_path_with_scope(
                entity_id,
                None,
                RetainedInvalidationScope::InvalidateSubtree,
            );
        }
        if any {
            self.schedule_image_ready_frame();
        }
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    /// Your view will be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time. While a shared load is pending, wakeups are coalesced per asset, observing window and
    /// view so animation or scroll frames cannot accumulate duplicate completion tasks.
    pub fn use_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, is_first) = cx.fetch_asset::<A>(source);
        if let Some(output) = task.clone().now_or_never() {
            return Some(output);
        }

        let entity_id = self.current_view();
        let subscription_id = (
            TypeId::of::<A>(),
            crate::hash(source),
            self.handle.window_id().as_u64(),
        );
        let Some(generation) =
            subscribe_asset_view(cx, subscription_id, entity_id, is_first)
        else {
            return None;
        };

        self.spawn(cx, {
            let task = task.clone();
            async move |cx| {
                task.await;

                // Invalidate the exact observing window directly. During a window's first frame the
                // global App entity -> WindowInvalidator map may not have been published yet, so a
                // plain `cx.notify(view)` can queue an effect without making the image-ready frame's
                // view dirty. That frame then legally replays the old retained image range until an
                // unrelated input event dirties the view. Per-window subscriptions close that race
                // while retaining one completion task per asset/window rather than per element.
                cx.update(move |window, cx| {
                    let views =
                        finish_asset_view_subscription(cx, subscription_id, generation);
                    window.schedule_asset_ready_views(views);
                })
                .ok();
            }
        })
        .detach();

        None
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading or doesn't exist this will return None.
    /// Your view will not be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, _) = cx.fetch_asset::<A>(source);
        task.now_or_never()
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames.
    pub fn use_keyed_state<S: 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        let current_view = self.current_view();
        self.with_global_id(key.into(), |global_id, window| {
            window.with_element_state(global_id, |state: Option<Entity<S>>, window| {
                if let Some(state) = state {
                    (state.clone(), state)
                } else {
                    let new_state = cx.new(|cx| init(window, cx));
                    Self::observe_keyed_state(&new_state, current_view, cx);
                    (new_state.clone(), new_state)
                }
            })
        })
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames, without needing to specify a key
    ///
    /// NOTE: This method uses the location of the caller to generate an ID for this state.
    ///       If this is not sufficient to identify your state (e.g. you're rendering a list item),
    ///       you can provide a custom ElementID using the `use_keyed_state` method.
    #[track_caller]
    pub fn use_state<S: 'static>(
        &mut self,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        self.use_keyed_state(
            ElementId::CodeLocation(*core::panic::Location::caller()),
            cx,
            init,
        )
    }

    /// Updates or initializes state for an element with the given id that lives across multiple
    /// frames. If an element with this ID existed in the rendered frame, its state will be passed
    /// to the given closure. The state returned by the closure will be stored so it can be referenced
    /// when drawing the next frame. This method should only be called as part of element drawing.
    #[inline(always)]
    pub fn with_element_state<S, R>(
        &mut self,
        global_id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        let (key, state) = self.take_element_state(global_id, TypeId::of::<S>());

        if let Some(any) = state {
            let ElementStateBox {
                inner,
                #[cfg(debug_assertions)]
                type_name,
            } = any;
            // Using the extra inner option to avoid needing to reallocate a new box.
            let mut state_box = inner
                .downcast::<Option<S>>()
                .map_err(|_| {
                    #[cfg(debug_assertions)]
                    {
                        anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}, actual: {:?}",
                            std::any::type_name::<S>(),
                            type_name
                        )
                    }

                    #[cfg(not(debug_assertions))]
                    {
                        anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}",
                            std::any::type_name::<S>(),
                        )
                    }
                })
                .unwrap();

            let state = state_box.take().expect(
                "reentrant call to with_element_state for the same state type and element id",
            );
            let (result, state) = f(Some(state), self);
            state_box.replace(state);
            self.insert_element_state(
                key,
                ElementStateBox {
                    inner: state_box,
                    #[cfg(debug_assertions)]
                    type_name,
                },
            );
            result
        } else {
            let (result, state) = f(None, self);
            self.insert_element_state(
                key,
                ElementStateBox {
                    inner: Box::new(Some(state)),
                    #[cfg(debug_assertions)]
                    type_name: std::any::type_name::<S>(),
                },
            );
            result
        }
    }

    /// A variant of `with_element_state` that allows the element's id to be optional. This is a convenience
    /// method for elements where the element id may or may not be assigned. Prefer using `with_element_state`
    /// when the element is guaranteed to have an id.
    ///
    /// The first option means 'no ID provided'
    /// The second option means 'not yet initialized'
    pub fn with_optional_element_state<S, R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        f: impl FnOnce(Option<Option<S>>, &mut Self) -> (R, Option<S>),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(global_id) = global_id {
            self.with_element_state(global_id, |state, cx| {
                let (result, state) = f(Some(state), cx);
                let state =
                    state.expect("you must return some state when you pass some element id");
                (result, state)
            })
        } else {
            let (result, state) = f(None, self);
            debug_assert!(
                state.is_none(),
                "you must not return an element state when passing None for the global id"
            );
            result
        }
    }
    #[inline(never)]
    fn take_element_state(
        &mut self,
        global_id: &GlobalElementId,
        state_type: TypeId,
    ) -> ((GlobalElementId, TypeId), Option<ElementStateBox>) {
        let key = (GlobalElementId(global_id.0.clone()), state_type);
        self.next_frame
            .accessed_element_states
            .push((key.0.clone(), state_type));
        let state = self
            .next_frame
            .element_states
            .remove(&key)
            .or_else(|| self.rendered_frame.element_states.remove(&key));
        (key, state)
    }

    #[inline(never)]
    fn insert_element_state(
        &mut self,
        key: (GlobalElementId, TypeId),
        state: ElementStateBox,
    ) -> Option<ElementStateBox> {
        self.next_frame.element_states.insert(key, state)
    }

    #[inline(never)]
    fn observe_keyed_state<S: 'static>(state: &Entity<S>, current_view: EntityId, cx: &mut App) {
        cx.observe(state, move |_, cx| {
            cx.notify(current_view);
        })
        .detach();
    }

}
