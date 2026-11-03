//! Circular column coverage shared by streaming, eviction and the range display.

pub(super) fn contains(dx: i64, dz: i64, radius: i32) -> bool {
    let radius = i64::from(radius);
    radius >= 0 && dx.abs() <= radius && dz.abs() <= radius && dx * dx + dz * dz <= radius * radius
}

pub(super) fn column_count(radius: i32) -> usize {
    if radius < 0 {
        return 0;
    }
    let radius = i64::from(radius);
    (-radius..=radius)
        .map(|x| {
            let half_width = ((radius * radius - x * x) as f64).sqrt() as usize;
            2 * half_width + 1
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_coverage_includes_boundary_and_excludes_corners() {
        assert!(contains(256, 0, 256));
        assert!(contains(0, -256, 256));
        assert!(contains(3, 4, 5));
        assert!(!contains(4, 4, 5));
        assert!(!contains(256, 1, 256));
        assert!(!contains(i64::from(i32::MAX) - i64::from(i32::MIN), 0, 256));
    }

    #[test]
    fn column_count_matches_discrete_circle() {
        for radius in [0, 1, 4, 8, 32, 256] {
            let actual = (-radius..=radius)
                .flat_map(|x| (-radius..=radius).map(move |z| (x, z)))
                .filter(|&(x, z)| contains(i64::from(x), i64::from(z), radius))
                .count();
            assert_eq!(column_count(radius), actual);
        }
        assert_eq!(column_count(1), 5);
        assert_eq!(column_count(8), 197);
        assert_eq!(column_count(256), 205861);
    }
}
