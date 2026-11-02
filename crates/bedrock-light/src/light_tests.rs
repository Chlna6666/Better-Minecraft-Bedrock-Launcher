use super::*;

#[test]
fn opaque_sources_emit_but_removed_sources_do_not_leave_light() {
    let mut cells = vec![LightCell::AIR; 33];
    cells[16] = LightCell::new(15, 15).unwrap();
    let lit = LightVolume::solve([33, 1, 1], &cells, false).unwrap();
    assert_eq!(lit.packed_at([16, 0, 0]), Some(15));
    assert_eq!(lit.packed_at([30, 0, 0]), Some(1));
    assert_eq!(lit.packed_at([31, 0, 0]), Some(0));
    cells[16] = LightCell::AIR;
    assert!(
        LightVolume::solve([33, 1, 1], &cells, false)
            .unwrap()
            .levels()
            .iter()
            .all(|&n| n == 0)
    );
}

#[test]
fn roof_blocks_sky_and_openings_admit_lateral_light() {
    let size = [5, 5, 5];
    let mut cells = vec![LightCell::AIR; 125];
    for z in 0..5 {
        for x in 0..5 {
            cells[index(size, [x, 3, z])] = LightCell::OPAQUE;
        }
    }
    let closed = LightVolume::solve(size, &cells, true).unwrap();
    assert_eq!(closed.packed_at([2, 2, 2]), Some(0));
    cells[index(size, [2, 3, 2])] = LightCell::AIR;
    let open = LightVolume::solve(size, &cells, true).unwrap();
    assert_eq!(open.packed_at([2, 0, 2]), Some(15 << 4));
    assert_eq!(open.packed_at([3, 2, 2]), Some(14 << 4));
    cells[index(size, [2, 3, 2])] = LightCell::new(0, 2).unwrap();
    assert_eq!(
        LightVolume::solve(size, &cells, true)
            .unwrap()
            .packed_at([2, 3, 2]),
        Some(13 << 4)
    );
}

#[test]
fn invalid_volume_and_levels_are_rejected() {
    assert!(LightVolume::solve([0, 1, 1], &[], true).is_err());
    assert!(LightVolume::solve([u32::MAX; 3], &[], true).is_err());
    assert!(LightCell::new(16, 0).is_none());
    assert!(LightCell::new(0, 16).is_none());
}
