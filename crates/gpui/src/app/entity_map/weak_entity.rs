use crate::{App, AppContext, Context, Flatten, VisualContext, Window};
use anyhow::{Context as _, Result};
use derive_more::{Deref, DerefMut};
use parking_lot::RwLock;
use std::{
    any::{TypeId, type_name},
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::{
        Weak,
        atomic::{AtomicU64, Ordering::SeqCst},
    },
};

use super::{AnyEntity, Entity, EntityId, EntityRefCounts};
use crate::foundation::util::atomic_incr_if_not_zero;

/// A type erased, weak reference to a entity.
#[derive(Clone)]
pub struct AnyWeakEntity {
    pub(crate) entity_id: EntityId,
    entity_type: TypeId,
    entity_ref_counts: Weak<RwLock<EntityRefCounts>>,
}

impl AnyWeakEntity {
    pub(in crate::app::entity_map) fn new(
        entity_id: EntityId,
        entity_type: TypeId,
        entity_ref_counts: Weak<RwLock<EntityRefCounts>>,
    ) -> Self {
        Self {
            entity_id,
            entity_type,
            entity_ref_counts,
        }
    }

    /// Get the entity ID associated with this weak reference.
    pub fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    /// Check if this weak handle can be upgraded, or if the entity has already been dropped
    pub fn is_upgradable(&self) -> bool {
        let ref_count = self
            .entity_ref_counts
            .upgrade()
            .and_then(|ref_counts| Some(ref_counts.read().counts.get(self.entity_id)?.load(SeqCst)))
            .unwrap_or(0);
        ref_count > 0
    }

    /// Upgrade this weak entity reference to a strong reference.
    pub fn upgrade(&self) -> Option<AnyEntity> {
        let ref_counts = &self.entity_ref_counts.upgrade()?;
        let ref_counts = ref_counts.read();
        let ref_count = ref_counts.counts.get(self.entity_id)?;

        if atomic_incr_if_not_zero(ref_count) == 0 {
            // entity_id is in dropped_entity_ids
            return None;
        }
        drop(ref_counts);

        Some(AnyEntity {
            entity_id: self.entity_id,
            entity_type: self.entity_type,
            entity_map: self.entity_ref_counts.clone(),
            #[cfg(any(test, feature = "leak-detection"))]
            handle_id: self
                .entity_ref_counts
                .upgrade()
                .unwrap()
                .write()
                .leak_detector
                .handle_created(self.entity_id),
        })
    }

    /// Assert that entity referenced by this weak handle has been released.
    #[cfg(any(test, feature = "leak-detection"))]
    pub fn assert_released(&self) {
        self.entity_ref_counts
            .upgrade()
            .unwrap()
            .write()
            .leak_detector
            .assert_released(self.entity_id);

        if self
            .entity_ref_counts
            .upgrade()
            .and_then(|ref_counts| Some(ref_counts.read().counts.get(self.entity_id)?.load(SeqCst)))
            .is_some()
        {
            panic!(
                "entity was recently dropped but resources are retained until the end of the effect cycle."
            )
        }
    }

    /// Creates a weak entity that can never be upgraded.
    pub fn new_invalid() -> Self {
        /// To hold the invariant that all ids are unique, and considering that slotmap
        /// increases their IDs from `0`, we can decrease ours from `u64::MAX` so these
        /// two will never conflict (u64 is way too large).
        static UNIQUE_NON_CONFLICTING_ID_GENERATOR: AtomicU64 = AtomicU64::new(u64::MAX);
        let entity_id = UNIQUE_NON_CONFLICTING_ID_GENERATOR.fetch_sub(1, SeqCst);

        Self {
            // Safety:
            //   Docs say this is safe but can be unspecified if slotmap changes the representation
            //   after `1.0.7`, that said, providing a valid entity_id here is not necessary as long
            //   as we guarantee that `entity_id` is never used if `entity_ref_counts` equals
            //   to `Weak::new()` (that is, it's unable to upgrade), that is the invariant that
            //   actually needs to be hold true.
            //
            //   And there is no sane reason to read an entity slot if `entity_ref_counts` can't be
            //   read in the first place, so we're good!
            entity_id: entity_id.into(),
            entity_type: TypeId::of::<()>(),
            entity_ref_counts: Weak::new(),
        }
    }
}

impl fmt::Debug for AnyWeakEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("entity_id", &self.entity_id)
            .field("entity_type", &self.entity_type)
            .finish()
    }
}

impl<T> From<WeakEntity<T>> for AnyWeakEntity {
    fn from(entity: WeakEntity<T>) -> Self {
        entity.any_entity
    }
}

impl Hash for AnyWeakEntity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity_id.hash(state);
    }
}

impl PartialEq for AnyWeakEntity {
    fn eq(&self, other: &Self) -> bool {
        self.entity_id == other.entity_id
    }
}

impl Eq for AnyWeakEntity {}

impl Ord for AnyWeakEntity {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity_id.cmp(&other.entity_id)
    }
}

impl PartialOrd for AnyWeakEntity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A weak reference to a entity of the given type.
#[derive(Deref, DerefMut)]
pub struct WeakEntity<T> {
    #[deref]
    #[deref_mut]
    any_entity: AnyWeakEntity,
    entity_type: PhantomData<fn(T) -> T>,
}

impl<T> fmt::Debug for WeakEntity<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("entity_id", &self.any_entity.entity_id)
            .field("entity_type", &type_name::<T>())
            .finish()
    }
}

impl<T> Clone for WeakEntity<T> {
    fn clone(&self) -> Self {
        Self {
            any_entity: self.any_entity.clone(),
            entity_type: self.entity_type,
        }
    }
}

impl<T: 'static> WeakEntity<T> {
    pub(in crate::app::entity_map) fn new(
        any_entity: AnyWeakEntity,
        entity_type: PhantomData<fn(T) -> T>,
    ) -> Self {
        Self {
            any_entity,
            entity_type,
        }
    }

    /// Upgrade this weak entity reference into a strong entity reference
    pub fn upgrade(&self) -> Option<Entity<T>> {
        Some(Entity {
            any_entity: self.any_entity.upgrade()?,
            entity_type: self.entity_type,
        })
    }

    /// Updates the entity referenced by this handle with the given function if
    /// the referenced entity still exists. Returns an error if the entity has
    /// been released.
    pub fn update<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> Result<R>
    where
        C: AppContext,
        Result<C::Result<R>>: Flatten<R>,
    {
        Flatten::flatten(
            self.upgrade()
                .context("entity released")
                .map(|this| cx.update_entity(&this, update)),
        )
    }

    /// Updates the entity referenced by this handle with the given function if
    /// the referenced entity still exists, within a visual context that has a window.
    /// Returns an error if the entity has been released.
    pub fn update_in<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R,
    ) -> Result<R>
    where
        C: VisualContext,
        Result<C::Result<R>>: Flatten<R>,
    {
        let window = cx.window_handle();
        let this = self.upgrade().context("entity released")?;

        Flatten::flatten(window.update(cx, |_, window, cx| {
            this.update(cx, |entity, cx| update(entity, window, cx))
        }))
    }

    /// Reads the entity referenced by this handle with the given function if
    /// the referenced entity still exists. Returns an error if the entity has
    /// been released.
    pub fn read_with<C, R>(&self, cx: &C, read: impl FnOnce(&T, &App) -> R) -> Result<R>
    where
        C: AppContext,
        Result<C::Result<R>>: Flatten<R>,
    {
        Flatten::flatten(
            self.upgrade()
                .context("entity released")
                .map(|this| cx.read_entity(&this, read)),
        )
    }

    /// Create a new weak entity that can never be upgraded.
    pub fn new_invalid() -> Self {
        Self {
            any_entity: AnyWeakEntity::new_invalid(),
            entity_type: PhantomData,
        }
    }
}

impl<T> Hash for WeakEntity<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.any_entity.hash(state);
    }
}

impl<T> PartialEq for WeakEntity<T> {
    fn eq(&self, other: &Self) -> bool {
        self.any_entity == other.any_entity
    }
}

impl<T> Eq for WeakEntity<T> {}

impl<T> PartialEq<Entity<T>> for WeakEntity<T> {
    fn eq(&self, other: &Entity<T>) -> bool {
        self.entity_id() == other.any_entity.entity_id()
    }
}

impl<T: 'static> Ord for WeakEntity<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity_id().cmp(&other.entity_id())
    }
}

impl<T: 'static> PartialOrd for WeakEntity<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
