#[derive(Clone, Copy)]
pub(super) struct TextureRegion {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Copy)]
pub(super) struct CuboidUv {
    pub(super) top: TextureRegion,
    pub(super) bottom: TextureRegion,
    pub(super) right: TextureRegion,
    pub(super) front: TextureRegion,
    pub(super) left: TextureRegion,
    pub(super) back: TextureRegion,
}

pub(super) fn head_uv(overlay: bool) -> CuboidUv {
    uv_box(if overlay { 32 } else { 0 }, 0, 8, 8, 8)
}

pub(super) fn body_uv(overlay: bool) -> CuboidUv {
    uv_box(16, if overlay { 32 } else { 16 }, 8, 12, 4)
}

pub(super) fn arm_uv(left: bool, overlay: bool, slim: bool) -> CuboidUv {
    let width = if slim { 3 } else { 4 };
    let (x, y) = match (left, overlay) {
        (false, false) => (40, 16),
        (false, true) => (40, 32),
        (true, false) => (32, 48),
        (true, true) => (48, 48),
    };
    uv_box(x, y, width, 12, 4)
}

pub(super) fn leg_uv(left: bool, overlay: bool) -> CuboidUv {
    let (x, y) = match (left, overlay) {
        (false, false) => (0, 16),
        (false, true) => (0, 32),
        (true, false) => (16, 48),
        (true, true) => (0, 48),
    };
    uv_box(x, y, 4, 12, 4)
}

pub(super) fn uv_box(x: u32, y: u32, width: u32, height: u32, depth: u32) -> CuboidUv {
    CuboidUv {
        top: TextureRegion {
            x: x + depth,
            y,
            width,
            height: depth,
        },
        bottom: TextureRegion {
            x: x + depth + width,
            y,
            width,
            height: depth,
        },
        right: TextureRegion {
            x,
            y: y + depth,
            width: depth,
            height,
        },
        front: TextureRegion {
            x: x + depth,
            y: y + depth,
            width,
            height,
        },
        left: TextureRegion {
            x: x + depth + width,
            y: y + depth,
            width: depth,
            height,
        },
        back: TextureRegion {
            x: x + depth * 2 + width,
            y: y + depth,
            width,
            height,
        },
    }
}
