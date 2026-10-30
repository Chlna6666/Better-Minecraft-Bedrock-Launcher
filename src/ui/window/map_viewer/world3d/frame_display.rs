//! Item-frame display faces decoded from Bedrock block entities.

use crate::ui::window::map_viewer::preview_3d_source::{Preview3dStreamFace, Preview3dStreamPass};
use bedrock_world::NbtTag;
use bedrock_world::map_item::{MapItemId, filled_map_id};
use bedrock_world::surface::ChunkData;
use std::sync::Arc;

pub(super) const MAP_MATERIAL_PREFIX: &str = "world3d/map/";
pub(super) const ITEM_MATERIAL_PREFIX: &str = "world3d/item/";

fn item_rotation(value: Option<&NbtTag>) -> f32 {
    match value {
        Some(NbtTag::Byte(value)) => f32::from(*value),
        Some(NbtTag::Short(value)) => f32::from(*value),
        Some(NbtTag::Int(value)) => *value as f32,
        Some(NbtTag::Float(value)) => *value,
        Some(NbtTag::Double(value)) => *value as f32,
        _ => 0.0,
    }
}

#[derive(Clone)]
pub(super) struct FrameDisplay {
    position: [i32; 3],
    normal: [i32; 3],
    rotation: f32,
    content: FrameContent,
}

#[derive(Clone)]
enum FrameContent {
    Map(MapItemId),
    Item(String),
}

impl FrameDisplay {
    pub(super) fn estimated_heap_bytes(&self) -> usize {
        match &self.content {
            FrameContent::Map(id) => id.as_str().len(),
            FrameContent::Item(name) => name.capacity(),
        }
    }

    /// Whether this frame displays a map item.
    pub(super) fn is_map(&self) -> bool {
        matches!(self.content, FrameContent::Map(_))
    }

    /// Block position of this frame.
    pub(super) const fn position(&self) -> [i32; 3] {
        self.position
    }

    pub(super) fn collect(chunk: &ChunkData) -> Vec<Self> {
        chunk
            .block_entities
            .iter()
            .filter_map(|entity| {
                let position = entity.position?;
                let block = chunk
                    .subchunks
                    .get(&i8::try_from(position[1].div_euclid(16)).ok()?)?
                    .block_state_at(
                        position[0].rem_euclid(16) as u8,
                        position[1].rem_euclid(16) as u8,
                        position[2].rem_euclid(16) as u8,
                    )?;
                if !matches!(
                    block.name.as_str(),
                    "minecraft:frame" | "minecraft:glow_frame"
                ) {
                    return None;
                }
                let facing = block
                    .states
                    .get("facing_direction")
                    .or_else(|| block.states.get("minecraft:facing_direction"))?;
                let normal = match facing {
                    NbtTag::Byte(0) | NbtTag::Int(0) => [0, -1, 0],
                    NbtTag::Byte(1) | NbtTag::Int(1) => [0, 1, 0],
                    NbtTag::Byte(2) | NbtTag::Int(2) => [0, 0, -1],
                    NbtTag::Byte(3) | NbtTag::Int(3) => [0, 0, 1],
                    NbtTag::Byte(4) | NbtTag::Int(4) => [-1, 0, 0],
                    NbtTag::Byte(5) | NbtTag::Int(5) => [1, 0, 0],
                    _ => return None,
                };
                let NbtTag::Compound(fields) = &entity.nbt else {
                    return None;
                };
                let item = fields.get("Item")?;
                let content = if let Some(id) = filled_map_id(item) {
                    FrameContent::Map(id)
                } else {
                    let NbtTag::Compound(item) = item else {
                        return None;
                    };
                    let Some(NbtTag::String(name)) = item.get("Name") else {
                        return None;
                    };
                    FrameContent::Item(name.clone())
                };
                let rotation = item_rotation(fields.get("ItemRotation"));
                Some(Self {
                    position,
                    normal,
                    rotation,
                    content,
                })
            })
            .collect()
    }

    pub(super) fn face(&self) -> Preview3dStreamFace {
        // right x up must equal the facing normal so the content is never mirrored when seen
        // from the front. Vertical frames keep world up; horizontal frames keep map-north up.
        let (right, up): ([f64; 3], [f64; 3]) = match self.normal {
            [0, -1, 0] => ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            [0, 1, 0] => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            [0, 0, -1] => ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            [0, 0, 1] => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            [-1, 0, 0] => ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            _ => ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        };
        // A map fills the whole frame block and sits just in front of the border ring. An item
        // sprite is half a block wide and sits in the frame recess, 1/16 in front of the wooden
        // backing plate, so it never z-fights the border front face.
        let (half_extent, material, offset, lift) = match &self.content {
            FrameContent::Map(id) => (0.5, format!("{MAP_MATERIAL_PREFIX}{id}"), 0.4375, 0.001),
            FrameContent::Item(name) => (
                0.25,
                format!("{ITEM_MATERIAL_PREFIX}{name}"),
                0.46875,
                0.002,
            ),
        };
        let center = self.position.map(|value| f64::from(value) + 0.5);
        let center: [f64; 3] = std::array::from_fn(|index| {
            center[index] - f64::from(self.normal[index]) * offset
                + f64::from(self.normal[index]) * lift
        });
        // Bedrock stores ItemRotation in degrees and positive values rotate clockwise when seen
        // from the front, so the scene-space angle is negated.
        let degrees = f64::from(self.rotation).rem_euclid(360.0);
        let angle = -degrees.to_radians();
        let (sin, cos) = angle.sin_cos();
        let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| {
            let x = x * half_extent;
            let y = y * half_extent;
            let (x, y) = (x * cos + y * sin, -x * sin + y * cos);
            std::array::from_fn(|axis| center[axis] + right[axis] * x + up[axis] * y)
        });
        Preview3dStreamFace {
            corners,
            color: [1.0; 4],
            shade: 1.0,
            normal: self.normal,
            material: Arc::from(material),
            uv: Some([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
            pass: Preview3dStreamPass::DetailOpaque,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameContent, FrameDisplay, MAP_MATERIAL_PREFIX, item_rotation};
    use bedrock_world::NbtTag;
    use bedrock_world::map_item::MapItemId;

    #[test]
    fn map_frame_face_is_flush_with_the_support_wall() {
        let display = FrameDisplay {
            position: [10, 64, 20],
            normal: [0, 0, -1],
            rotation: 0.0,
            content: FrameContent::Map(MapItemId::new("42").expect("map id")),
        };
        let face = display.face();
        assert_eq!(face.material.as_ref(), format!("{MAP_MATERIAL_PREFIX}42"));
        assert_eq!(face.normal, [0, 0, -1]);
        // Facing north means the supporting wall is on the south side of the frame block.
        assert!(
            face.corners
                .iter()
                .all(|corner| (corner[2] - 20.9365).abs() < 0.0001)
        );
        assert_eq!(face.corners[0][0], 11.0);
        assert_eq!(face.corners[1][0], 10.0);
        // A map covers the whole block, so adjacent frames tile without a gap.
        assert!(((face.corners[1][0] - face.corners[0][0]).abs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn frame_basis_is_right_handed_for_every_facing() {
        for normal in [
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
        ] {
            let display = FrameDisplay {
                position: [0, 0, 0],
                normal,
                rotation: 0.0,
                content: FrameContent::Item("minecraft:paper".to_string()),
            };
            let face = display.face();
            let right = [
                face.corners[1][0] - face.corners[0][0],
                face.corners[1][1] - face.corners[0][1],
                face.corners[1][2] - face.corners[0][2],
            ];
            let up = [
                face.corners[3][0] - face.corners[0][0],
                face.corners[3][1] - face.corners[0][1],
                face.corners[3][2] - face.corners[0][2],
            ];
            let cross = [
                right[1] * up[2] - right[2] * up[1],
                right[2] * up[0] - right[0] * up[2],
                right[0] * up[1] - right[1] * up[0],
            ];
            let length = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
            assert!(length > 0.1);
            for axis in 0..3 {
                let expected = f64::from(normal[axis]);
                assert!(
                    (cross[axis] / length - expected).abs() < 1e-6,
                    "facing {normal:?} axis {axis}"
                );
            }
        }
    }

    #[test]
    fn byte_item_rotation_is_degrees() {
        assert_eq!(item_rotation(Some(&NbtTag::Byte(3))), 3.0);
    }
}
