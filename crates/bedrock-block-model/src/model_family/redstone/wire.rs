use crate::model_family::direction::CardinalDirection;
use crate::model_family::shape::{ModelPlane, ModelShape, ground_plane, uv16};
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
    // The dust textures draw the wire stripe horizontally. A north-south wire must rotate that
    // stripe by a quarter turn, otherwise the line renders across the block instead of along it.
    let uv = if along_z {
        [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]
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
            [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        ),
        CardinalDirection::South => (
            [
                [0.0, 0.012, 0.5],
                [1.0, 0.012, 0.5],
                [1.0, 0.012, 1.0],
                [0.0, 0.012, 1.0],
            ],
            [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
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
    // The dust texture draws a horizontal stripe, so wall-climb planes rotate it a quarter turn
    // to run the stripe up the face instead of across it.
    let uv = match direction {
        CardinalDirection::North | CardinalDirection::East => {
            [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]
        }
        CardinalDirection::South | CardinalDirection::West => {
            [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]
        }
    };
    ModelPlane::new(corners, direction.normal())
        .with_material_slot("down")
        .with_uv(uv)
}
