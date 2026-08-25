from pathlib import Path

path = Path(__file__).resolve().parents[1] / "crates/bedrock-block-model/src/neighbor.rs"
text = path.read_text(encoding="utf-8")
old = '''    const fn from_descriptor(descriptor: NeighborBlockDescriptor) -> Self {
        let mut bits = u32::from(descriptor.connection_bits) & Self::CONNECTION_MASK;
        if descriptor.wall_up.unwrap_or(false) {
            bits |= Self::WALL_UP_BIT;
        }
        if descriptor.top_half {
            bits |= Self::TOP_HALF_BIT;
        }
        if let Some(shape) = descriptor.stair_shape {
            bits |= (shape as u32) << Self::STAIR_SHIFT;
        }
        let facing = descriptor.facing.map_or(4, HorizontalDirection::index);
        bits |= u32::from(facing) << Self::FACING_SHIFT;
        bits |= u32::from(descriptor.power & 0xf) << Self::POWER_SHIFT;
        bits |= (descriptor.kind as u32) << Self::KIND_SHIFT;
        Self(bits)
    }
'''
new = '''    const fn from_descriptor(descriptor: NeighborBlockDescriptor) -> Self {
        let mut bits = (descriptor.connection_bits as u32) & Self::CONNECTION_MASK;
        if matches!(descriptor.wall_up, Some(true)) {
            bits |= Self::WALL_UP_BIT;
        }
        if descriptor.top_half {
            bits |= Self::TOP_HALF_BIT;
        }
        if let Some(shape) = descriptor.stair_shape {
            bits |= (shape as u32) << Self::STAIR_SHIFT;
        }
        let facing = match descriptor.facing {
            Some(direction) => direction.index(),
            None => 4,
        };
        bits |= (facing as u32) << Self::FACING_SHIFT;
        bits |= ((descriptor.power & 0xf) as u32) << Self::POWER_SHIFT;
        bits |= (descriptor.kind as u32) << Self::KIND_SHIFT;
        Self(bits)
    }
'''
count = text.count(old)
if count != 1:
    raise RuntimeError(f"neighbor const compatibility: expected 1 match, got {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
