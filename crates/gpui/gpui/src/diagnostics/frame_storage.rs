//! On-demand shallow backing-storage accounting for the two UI frames.
//! The snapshot is deliberately not a process or GPU memory total.

use collections::FxHashSet;
use std::mem::size_of;

/// Known Vec backing capacities of the root Scene and hitbox stores of a window.
/// Shared physical allocations are counted once across the committed and build frames.
/// Excludes allocation headers, spare allocator pages, nested Paint, path vertex data,
/// callbacks, layout/dependency records, font data and GPU-owner-only publications.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct FrameStorageSnapshot {
    /// Operation indices and non-primitive operation metadata.
    pub operation_bytes: usize,
    /// Direct primitive backing arrays, including operation-owned typed payloads.
    pub primitive_bytes: usize,
    /// Draw and operation descriptors, prefix states and prepared batches.
    pub descriptor_bytes: usize,
    /// Hitbox geometry and hitbox span descriptors.
    pub hitbox_bytes: usize,
    /// Number of distinct nonempty physical backing allocations.
    pub allocations: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Operation,
    Primitive,
    Descriptor,
    Hitbox,
}

#[derive(Default)]
pub(crate) struct Accounting {
    seen: FxHashSet<*const ()>,
    snapshot: FrameStorageSnapshot,
}

impl Accounting {
    pub(crate) fn add<T>(&mut self, values: &Vec<T>, kind: Kind) {
        let bytes = values.capacity().saturating_mul(size_of::<T>());
        if bytes == 0 || !self.seen.insert(values.as_ptr().cast()) {
            return;
        }
        let field = match kind {
            Kind::Operation => &mut self.snapshot.operation_bytes,
            Kind::Primitive => &mut self.snapshot.primitive_bytes,
            Kind::Descriptor => &mut self.snapshot.descriptor_bytes,
            Kind::Hitbox => &mut self.snapshot.hitbox_bytes,
        };
        *field = field.saturating_add(bytes);
        self.snapshot.allocations += 1;
    }

    pub(crate) fn finish(self) -> FrameStorageSnapshot {
        self.snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn shared_backing_is_counted_once_and_capacity_is_not_length() {
        let mut values = Vec::<u32>::with_capacity(128);
        values.push(7);
        let owner = Arc::new(values);
        let alias = Arc::clone(&owner);
        let independent = vec![7u32];
        let mut accounting = Accounting::default();
        accounting.add(&owner, Kind::Primitive);
        accounting.add(&alias, Kind::Primitive);
        accounting.add(&independent, Kind::Primitive);
        accounting.add(&Vec::<u32>::new(), Kind::Descriptor);
        let snapshot = accounting.finish();
        assert_eq!(snapshot.primitive_bytes, (owner.capacity() + independent.capacity()) * 4);
        assert_eq!(snapshot.allocations, 2);
        assert_eq!(snapshot.descriptor_bytes, 0);
    }
}
