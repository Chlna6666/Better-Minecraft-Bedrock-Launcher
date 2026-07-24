use super::*;

#[test]
fn test_bounds_intersects() {
    let bounds1 = Bounds {
        origin: Point { x: 0.0, y: 0.0 },
        size: Size {
            width: 5.0,
            height: 5.0,
        },
    };
    let bounds2 = Bounds {
        origin: Point { x: 4.0, y: 4.0 },
        size: Size {
            width: 5.0,
            height: 5.0,
        },
    };
    let bounds3 = Bounds {
        origin: Point { x: 10.0, y: 10.0 },
        size: Size {
            width: 5.0,
            height: 5.0,
        },
    };

    // Test Case 1: Intersecting bounds
    assert!(bounds1.intersects(&bounds2));

    // Test Case 2: Non-Intersecting bounds
    assert!(!bounds1.intersects(&bounds3));

    // Test Case 3: Bounds intersecting with themselves
    assert!(bounds1.intersects(&bounds1));
}
