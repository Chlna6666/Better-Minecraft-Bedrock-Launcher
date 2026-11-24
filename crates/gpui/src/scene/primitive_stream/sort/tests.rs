use super::*;
use crate::{Quad, ScaledPixels, Shadow};
use std::{hint::black_box, time::Instant};

fn orders(count: usize, order_limit: u32) -> impl Iterator<Item = u32> {
    (0..count).map(move |index| (index as u32).wrapping_mul(2_654_435_761) % order_limit)
}

fn quads(count: usize, order_limit: u32) -> Vec<Quad> {
    orders(count, order_limit)
        .enumerate()
        .map(|(index, order)| {
            let mut item = Quad {
                order,
                ..Default::default()
            };
            item.bounds.origin.x = ScaledPixels(index as f32);
            item
        })
        .collect()
}

#[test]
fn indirect_sort_preserves_every_payload_and_equal_order_insertion() {
    let mut actual = quads(4096, u32::MAX);
    actual[4094].order = actual[1].order;
    assert!(favors_indirect(&actual));
    let mut expected = actual.clone();
    expected.sort_by_key(Quad::order);
    pending(&mut actual);
    assert_eq!(actual, expected);
}

#[test]
fn repeated_and_nearly_sorted_orders_keep_stable_insertion() {
    for count in [0, 1, 32, 1024, 10_000] {
        for order_limit in [1, 8, 32, 64, 128, 256] {
            let mut actual = quads(count, order_limit);
            if count >= 1024 {
                assert!(
                    !favors_indirect(&actual),
                    "count={count} order_limit={order_limit}"
                );
            }
            let mut expected = actual.clone();
            expected.sort_by_key(Quad::order);
            pending(&mut actual);
            assert_eq!(actual, expected);
        }
    }
    let mut actual = quads(4096, 4096);
    actual.sort_by_key(Quad::order);
    actual.rotate_right(1);
    assert!(!favors_indirect(&actual));
    let mut expected = actual.clone();
    expected.sort_by_key(Quad::order);
    pending(&mut actual);
    assert_eq!(actual, expected);
}

#[test]
fn rare_distinct_sample_outliers_cannot_disguise_repeated_orders() {
    let mut actual = quads(10_000, 1);
    // Put rare, widely separated keys at every sampling point.
    for i in 0..64 {
        let start = actual.len() * i / 64;
        let end = actual.len() * (i + 1) / 64;
        let mut noise = i as u32 ^ 0xa511e9b3;
        noise = (noise ^ (noise >> 16)).wrapping_mul(0x7feb352d);
        noise = (noise ^ (noise >> 15)).wrapping_mul(0x846ca68b);
        noise ^= noise >> 16;
        actual[start + noise as usize % (end - start)].order =
            (i as u32).wrapping_mul(2_654_435_761);
    }
    actual[0].order = 0x1234;
    actual[5000].order = 0x9abcdeff;
    actual[9999].order = 0x57631452;
    assert!(favors_indirect(&actual));
    assert!(indirect_keys(&actual).is_none());
    let mut expected = actual.clone();
    expected.sort_by_key(Quad::order);
    pending(&mut actual);
    assert_eq!(actual, expected);
}

#[test]
fn sorted_reversed_and_wide_orders_are_lossless() {
    for count in [1024, 10_000] {
        for reverse in [false, true] {
            let mut actual = quads(count, u32::MAX);
            actual.sort_by_key(Quad::order);
            let expected = actual.clone();
            if reverse {
                actual.reverse();
            }
            pending(&mut actual);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
#[ignore = "manual optimized-profile comparison of real Quad/Shadow payloads"]
fn pending_sort_profile() {
    for count in [1024, 10_000] {
        for order_limit in [1, 8, 32, 64, 128, 256, count as u32, u32::MAX] {
            let input = quads(count, order_limit);
            profile(&input, order_limit);
            let shadows: Vec<_> = input
                .iter()
                .map(|quad| Shadow {
                    order: quad.order,
                    bounds: quad.bounds,
                    ..Default::default()
                })
                .collect();
            profile(&shadows, order_limit);
        }
    }
}

fn profile<T: Ordered>(input: &[T], order_limit: u32) {
    let mut baseline = Vec::new();
    let mut adapted = Vec::new();
    for trial in 0..201 {
        // Alternate the measured order; allocation/cloning is outside the timed region.
        let mut first = input.to_vec();
        let mut second = input.to_vec();
        for (items, candidate) in [(&mut first, trial % 2 == 0), (&mut second, trial % 2 != 0)] {
            let start = Instant::now();
            if candidate {
                pending(black_box(items));
            } else {
                items.sort_by_key(T::order);
            }
            let nanos = start.elapsed().as_nanos();
            if trial > 0 {
                if candidate {
                    adapted.push(nanos);
                } else {
                    baseline.push(nanos);
                }
            }
            black_box(items);
        }
    }
    baseline.sort_unstable();
    adapted.sort_unstable();
    println!(
        "{} n={} order_limit={order_limit} median_ns before={} after={}",
        std::any::type_name::<T>(),
        input.len(),
        baseline[100],
        adapted[100]
    );
}
