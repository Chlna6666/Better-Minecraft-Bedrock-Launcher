use crate::model_family::direction::CardinalDirection;
use crate::model_family::shape::{ModelPlane, ModelShape, full_texture_uv, ground_plane, uv16};
use crate::state::{BlockStateQuery, BlockStateValue};

pub(crate) fn shape_for(state: &BlockStateQuery) -> ModelShape {
    let mut connected = Vec::new();
    let mut up_connections = Vec::new();
    for direction in CardinalDirection::ALL {
        if let Some(connection) = redstone_wire_connection(state, direction) {
            match connection {
                RedstoneWireConnection::Side => connected.push(direction),
                RedstoneWireConnection::Up => {
                    connected.push(direction);
                    up_connections.push(direction);
                }
            }
        }
    }

    let straight = connected.len() == 2
        && ((connected.contains(&CardinalDirection::North)
            && connected.contains(&CardinalDirection::South))
            || (connected.contains(&CardinalDirection::East)
                && connected.contains(&CardinalDirection::West)));
    let mut shape = ModelShape::default();
    if straight {
        let along_z = connected.contains(&CardinalDirection::North);
        shape.planes.push(redstone_wire_ground_plane_full(along_z));
    } else {
        shape.planes.push(redstone_wire_dot_plane());
        for direction in connected {
            shape.planes.push(redstone_wire_arm_plane(direction));
        }
    }
    for direction in up_connections {
        shape.planes.push(redstone_wire_wall_plane(direction));
    }
    shape
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RedstoneWireConnection {
    Side,
    Up,
}

fn redstone_wire_connection(
    state: &BlockStateQuery,
    direction: CardinalDirection,
) -> Option<RedstoneWireConnection> {
    let key = direction.state_key();
    let value = state
        .state(key)
        .or_else(|| state.state(&format!("redstone_signal_{key}")))
        .or_else(|| state.state(&format!("{key}_connection")));
    match value {
        Some(value) if value.matches_literal("up") => Some(RedstoneWireConnection::Up),
        Some(value)
            if value.matches_literal("side")
                || value.matches_literal("true")
                || value.matches_literal("1") =>
        {
            Some(RedstoneWireConnection::Side)
        }
        Some(value)
            if value.matches_literal("none")
                || value.matches_literal("false")
                || value.matches_literal("0") =>
        {
            None
        }
        Some(BlockStateValue::String(_)) => Some(RedstoneWireConnection::Side),
        Some(value) => value.is_truthy().then_some(RedstoneWireConnection::Side),
        None => None,
    }
}

fn redstone_wire_dot_plane() -> ModelPlane {
    ground_plane(
        [
            [0.25, 0.01, 0.25],
            [0.75, 0.01, 0.25],
            [0.75, 0.01, 0.75],
            [0.25, 0.01, 0.75],
        ],
        Some("up"),
        uv16(4.0, 4.0, 12.0, 12.0),
    )
}

fn redstone_wire_ground_plane_full(along_z: bool) -> ModelPlane {
    let uv = if along_z {
        full_texture_uv()
    } else {
        uv16(16.0, 0.0, 0.0, 16.0)
    };
    ground_plane(
        [
            [0.0, 0.01, 0.0],
            [1.0, 0.01, 0.0],
            [1.0, 0.01, 1.0],
            [0.0, 0.01, 1.0],
        ],
        Some("down"),
        uv,
    )
}

fn redstone_wire_arm_plane(direction: CardinalDirection) -> ModelPlane {
    let (corners, uv) = match direction {
        CardinalDirection::North => (
            [
                [0.0, 0.011, 0.0],
                [1.0, 0.011, 0.0],
                [1.0, 0.011, 0.5],
                [0.0, 0.011, 0.5],
            ],
            uv16(0.0, 8.0, 16.0, 16.0),
        ),
        CardinalDirection::South => (
            [
                [0.0, 0.012, 0.5],
                [1.0, 0.012, 0.5],
                [1.0, 0.012, 1.0],
                [0.0, 0.012, 1.0],
            ],
            uv16(0.0, 0.0, 16.0, 8.0),
        ),
        CardinalDirection::East => (
            [
                [0.5, 0.013, 0.0],
                [1.0, 0.013, 0.0],
                [1.0, 0.013, 1.0],
                [0.5, 0.013, 1.0],
            ],
            uv16(0.0, 0.0, 8.0, 16.0),
        ),
        CardinalDirection::West => (
            [
                [0.0, 0.014, 0.0],
                [0.5, 0.014, 0.0],
                [0.5, 0.014, 1.0],
                [0.0, 0.014, 1.0],
            ],
            uv16(8.0, 0.0, 16.0, 16.0),
        ),
    };
    ground_plane(corners, Some("down"), uv)
}

fn redstone_wire_wall_plane(direction: CardinalDirection) -> ModelPlane {
    let corners = match direction {
        CardinalDirection::North => [
            [0.0, 0.0, 0.01],
            [1.0, 0.0, 0.01],
            [1.0, 1.0, 0.01],
            [0.0, 1.0, 0.01],
        ],
        CardinalDirection::South => [
            [1.0, 0.0, 0.99],
            [0.0, 0.0, 0.99],
            [0.0, 1.0, 0.99],
            [1.0, 1.0, 0.99],
        ],
        CardinalDirection::East => [
            [0.99, 0.0, 0.0],
            [0.99, 0.0, 1.0],
            [0.99, 1.0, 1.0],
            [0.99, 1.0, 0.0],
        ],
        CardinalDirection::West => [
            [0.01, 0.0, 1.0],
            [0.01, 0.0, 0.0],
            [0.01, 1.0, 0.0],
            [0.01, 1.0, 1.0],
        ],
    };
    ModelPlane::new(corners, direction.normal())
        .with_material_slot("down")
        .with_uv(full_texture_uv())
}
