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
