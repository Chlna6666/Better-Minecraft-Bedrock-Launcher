use super::*;

/// Allocation size only; UI layout keeps using the native client extent.
pub(super) fn planned_size(
    current: DrawableSize,
    requested: DrawableSize,
    retain_capacity: bool,
) -> DrawableSize {
    if !retain_capacity {
        return requested;
    }
    fn axis(current: u32, requested: u32) -> u32 {
        if requested <= current {
            return current;
        }
        // Native buffer replacement is expensive. Geometric headroom avoids
        // repeating it at every 128-pixel step of a growing gesture; final
        // sizing still releases this temporary capacity when the gesture ends.
        let capacity = current.saturating_add(current / 2).max(requested);
        capacity
            .checked_add(127)
            .map(|value| value / 128 * 128)
            .filter(|value| *value <= i32::MAX as u32)
            .unwrap_or(requested)
    }
    DrawableSize {
        width: axis(current.width, requested.width),
        height: axis(current.height, requested.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_resize_reuses_capacity_without_changing_final_extent() {
        let current = DrawableSize {
            width: 1024,
            height: 640,
        };
        let requested = DrawableSize {
            width: 973,
            height: 610,
        };
        assert_eq!(planned_size(current, requested, true), current);
        assert_eq!(planned_size(current, requested, false), requested);
        assert_eq!(
            planned_size(
                current,
                DrawableSize {
                    width: 1025,
                    height: 641
                },
                true
            ),
            DrawableSize {
                width: 1536,
                height: 1024
            }
        );
    }
}
