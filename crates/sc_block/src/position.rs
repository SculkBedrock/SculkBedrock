//! Integer block coordinates.

use sc_world::chunk::ChunkPosition;
use std::fmt;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPosition {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Floor floating-point entity coordinates to their containing block.
    pub fn from_float(x: f32, y: f32, z: f32) -> Self {
        Self::new(x.floor() as i32, y.floor() as i32, z.floor() as i32)
    }

    pub fn chunk_position(&self) -> ChunkPosition {
        ChunkPosition::from_world(self.x, self.z)
    }

    pub fn local_x(&self) -> u8 {
        self.x.rem_euclid(16) as u8
    }

    pub fn local_z(&self) -> u8 {
        self.z.rem_euclid(16) as u8
    }
}

impl fmt::Display for BlockPosition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}, {}, {})", self.x, self.y, self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_map_to_floor_chunk_and_wrapped_local() {
        let position = BlockPosition::new(-1, 64, -17);
        assert_eq!(position.chunk_position(), ChunkPosition::new(-1, -2));
        assert_eq!(position.local_x(), 15);
        assert_eq!(position.local_z(), 15);
    }

    #[test]
    fn from_float_floors_toward_negative_infinity() {
        assert_eq!(
            BlockPosition::from_float(-0.5, 63.9, 16.0),
            BlockPosition::new(-1, 63, 16)
        );
    }
}
