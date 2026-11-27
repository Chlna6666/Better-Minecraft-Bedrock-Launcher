use super::{Ordered, Quad};

#[cfg(test)]
mod tests;

pub(super) fn pending<T: Ordered>(items: &mut [T]) {
    // Already ordered runs need no moves or scratch allocation. Keep equal-order
    // insertion stable on every path, including fresh payloads that are later shared.
    if items.is_sorted_by_key(T::order) {
        return;
    }
    // Small runs and repeated orders use the standard stable sort.
    if std::mem::size_of::<T>() < std::mem::size_of::<Quad>()
        || items.len() < 1024
        || std::mem::size_of_val(items) < 128 * 1024
    {
        items.sort_by_key(T::order);
        return;
    }
    let first = items[0].order();
    let middle = items[items.len() / 2].order();
    let last = items[items.len() - 1].order();
    let span = first.max(middle).max(last) - first.min(middle).min(last);
    // Cheap hints preserve the original fast path for dense or frequently repeated layers.
    if first == middle
        || middle == last
        || first == last
        || u64::from(span) < items.len() as u64 / 4
    {
        items.sort_by_key(T::order);
        return;
    }
    if !favors_indirect(items) {
        items.sort_by_key(T::order);
        return;
    }
    let Some(mut keys) = indirect_keys(items) else {
        items.sort_by_key(T::order);
        return;
    };
    keys.sort_unstable();
    permute(items, &mut keys);
}

fn indirect_keys<T: Ordered>(items: &[T]) -> Option<Vec<(u32, usize)>> {
    // Adapt Zed #66898 only for fresh payloads. Shared segments are never moved here.
    let bit_count = items.len().next_power_of_two();
    let mut seen = vec![0u64; bit_count / 64];
    let keys: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let order = item.order();
            let bucket = (order ^ (order >> 16)) as usize & (bit_count - 1);
            seen[bucket / 64] |= 1u64 << (bucket % 64);
            (order, index)
        })
        .collect();
    // Verify diversity over the whole run: rare sample outliers must not disguise repeated keys.
    // Hash collisions can only cause a conservative fallback, never inflate this lower bound.
    if seen
        .iter()
        .map(|word| word.count_ones() as usize)
        .sum::<usize>()
        < items.len() / 2
    {
        return None;
    }
    Some(keys)
}

fn favors_indirect<T: Ordered>(items: &[T]) -> bool {
    // Sample across the run so a repeated prefix does not decide the whole batch.
    // Bias toward the original sort when orders repeat or form long sorted runs.
    let mut sample = [0u32; 64];
    for (i, order) in sample.iter_mut().enumerate() {
        let start = items.len() * i / 64;
        let end = items.len() * (i + 1) / 64;
        let mut noise = i as u32 ^ 0xa511e9b3;
        noise = (noise ^ (noise >> 16)).wrapping_mul(0x7feb352d);
        noise = (noise ^ (noise >> 15)).wrapping_mul(0x846ca68b);
        noise ^= noise >> 16;
        *order = items[start + noise as usize % (end - start)].order();
    }
    let descents = sample.windows(2).filter(|pair| pair[0] > pair[1]).count();
    if !(16..=48).contains(&descents) {
        return false;
    }
    sample.sort_unstable();
    let distinct = 1 + sample.windows(2).filter(|pair| pair[0] != pair[1]).count();
    distinct >= 60 && u64::from(sample[63] - sample[0]) >= items.len() as u64 / 2
}

fn permute<T: Copy>(items: &mut [T], keys: &mut [(u32, usize)]) {
    for start in 0..items.len() {
        if keys[start].1 == start {
            continue;
        }
        let displaced = items[start];
        let mut target = start;
        loop {
            let source = keys[target].1;
            keys[target].1 = target;
            if source == start {
                items[target] = displaced;
                break;
            }
            items[target] = items[source];
            target = source;
        }
    }
}


/// One narrow permutation workspace for all operation-owned draw kinds.
/// Unlike a gathered typed scratch vector, this keeps no second copy of
/// the large GPU ABI payload across frames.
#[derive(Default)]
pub(crate) struct DrawPermutation {
    order: Vec<u32>,
    // Dense counts are no longer needed when the inverse map is built.
    // Reuse the same buffer instead of retaining another narrow array.
    mapping: Vec<u32>,
    keys: Vec<u64>,
    wide_keys: Vec<u128>,
}

impl DrawPermutation {
    /// Stable destination-to-source permutation. Dense DrawOrder domains
    /// are counted relative to their minimum, even when absolute orders
    /// are large. Sparse domains sort packed keys instead of random payload
    /// accesses. The already sorted case does not touch scratch at all.
    pub(crate) fn apply<T: Ordered>(
        &mut self, values: &mut [T], remap: impl FnOnce(&[u32]),
    ) {
        if values.len() < 2 || values.is_sorted_by_key(T::sort_key) {
            return;
        }
        debug_assert!(values.len() <= 1 << 28);
        let len = values.len();
        self.order.clear();
        if T::ORDER_ONLY {
            let (min, max) = values.iter().fold((u32::MAX, 0), |(min, max), value| {
                let order = value.order();
                (min.min(order), max.max(order))
            });
            let span = (max - min) as usize;
            if span < len.saturating_mul(2) {
                self.mapping.clear();
                self.mapping.resize(span + 1, 0);
                for value in values.iter() {
                    self.mapping[(value.order() - min) as usize] += 1;
                }
                let mut offset = 0;
                for count in &mut self.mapping {
                    let next = offset + *count;
                    *count = offset;
                    offset = next;
                }
                self.order.resize(len, 0);
                for (slot, value) in values.iter().enumerate() {
                    let next = &mut self.mapping[(value.order() - min) as usize];
                    self.order[*next as usize] = slot as u32;
                    *next += 1;
                }
            } else {
                self.keys.clear();
                self.keys.extend(values.iter().enumerate().map(|(slot, value)| {
                    (u64::from(value.order()) << 32) | slot as u64
                }));
                self.keys.sort_unstable();
                self.order.extend(self.keys.iter().map(|&key| key as u32));
            }
        } else {
            self.wide_keys.clear();
            self.wide_keys.extend(values.iter().enumerate().map(|(slot, value)| {
                value.packed_sort_key(slot as u32)
            }));
            self.wide_keys.sort_unstable();
            self.order.extend(self.wide_keys.iter().map(|&key| key as u32 & ((1 << 28) - 1)));
        }

        self.mapping.resize(len, 0);
        for (new, &old) in self.order.iter().enumerate() {
            self.mapping[old as usize] = new as u32;
        }
        remap(&self.mapping);

        // Rotate each permutation cycle with one typed assignment per
        // displaced slot instead of swapping two large primitives per step.
        for start in 0..len {
            if self.order[start] as usize == start { continue; }
            let displaced = values[start];
            let mut target = start;
            loop {
                let source = self.order[target] as usize;
                self.order[target] = target as u32;
                if source == start {
                    values[target] = displaced;
                    break;
                }
                values[target] = values[source];
                target = source;
            }
        }
        self.order.clear();
        self.mapping.clear();
        self.keys.clear();
        self.wide_keys.clear();
    }

    pub(crate) fn needs_trim(&self, floor: usize) -> bool {
        let threshold = floor.saturating_mul(2);
        self.order.capacity() > threshold
            || self.mapping.capacity() > threshold.saturating_mul(2)
            || self.keys.capacity() > threshold
            || self.wide_keys.capacity() > threshold
    }

    /// Trim only after the existing idle/high-water policy qualifies it.
    pub(crate) fn trim(&mut self, floor: usize) {
        macro_rules! trim {
            ($field:ident, $target:expr) => {
                if self.$field.capacity() > ($target).saturating_mul(2) {
                    self.$field.shrink_to($target);
                }
            };
        }
        trim!(order, floor);
        trim!(mapping, floor.saturating_mul(2));
        trim!(keys, floor);
        trim!(wide_keys, floor);
    }
}
