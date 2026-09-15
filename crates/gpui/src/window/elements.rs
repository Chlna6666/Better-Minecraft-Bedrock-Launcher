use super::*;

#[derive(Default)]
struct AssetViewSubscriptions {
    next_generation: u64,
    pending: collections::FxHashMap<(TypeId, u64), PendingAssetViewSubscription>,
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
    asset_id: (TypeId, u64),
    view: EntityId,
    new_load: bool,
) -> Option<u64> {
    let state = asset_view_subscriptions(cx);

    if !new_load
        && let Some(pending) = state.pending.get_mut(&asset_id)
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
        asset_id,
        PendingAssetViewSubscription {
            generation,
            views,
        },
    );
    Some(generation)
}

fn finish_asset_view_subscription(
    cx: &mut App,
    asset_id: (TypeId, u64),
    generation: u64,
) -> collections::FxHashSet<EntityId> {
    let state = asset_view_subscriptions(cx);
    if state
        .pending
        .get(&asset_id)
        .is_none_or(|pending| pending.generation != generation)
    {
        return collections::FxHashSet::default();
    }

    state
        .pending
        .remove(&asset_id)
        .map_or_else(collections::FxHashSet::default, |pending| pending.views)
}

impl Window {
    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    /// Your view will be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time. While a shared load is pending, wakeups are also coalesced per asset and observing
    /// view so animation or scroll frames cannot accumulate duplicate completion tasks.
    pub fn use_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, is_first) = cx.fetch_asset::<A>(source);
        if let Some(output) = task.clone().now_or_never() {
            return Some(output);
        }

        let entity_id = self.current_view();
        let asset_id = (TypeId::of::<A>(), crate::hash(source));
        let Some(generation) = subscribe_asset_view(cx, asset_id, entity_id, is_first) else {
            return None;
        };

        self.spawn(cx, {
            let task = task.clone();
            async move |cx| {
                task.await;

                // Asset completion must itself wake the owning views. Deferring this through
                // `on_next_frame` can deadlock an otherwise idle window: there is no next frame
                // until unrelated input (often a mouse move) happens to request one.
                cx.update(move |_, cx| {
                    for entity_id in finish_asset_view_subscription(cx, asset_id, generation) {
                        cx.notify(entity_id);
                    }
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
                    cx.observe(&new_state, move |_, cx| {
                        cx.notify(current_view);
                    })
                    .detach();
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
    pub fn with_element_state<S, R>(
        &mut self,
        global_id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        let key = (GlobalElementId(global_id.0.clone()), TypeId::of::<S>());
        self.next_frame
            .accessed_element_states
            .push((key.0.clone(), TypeId::of::<S>()));

        if let Some(any) = self
            .next_frame
            .element_states
            .remove(&key)
            .or_else(|| self.rendered_frame.element_states.remove(&key))
        {
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
            self.next_frame.element_states.insert(
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
            self.next_frame.element_states.insert(
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
}
