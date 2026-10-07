use sc_nbt::NbtValue;
use std::fmt;
use std::sync::Arc;

pub const SUBCHUNK_SIZE: i32 = 16;
pub const SUBCHUNK_VOLUME: usize = 16 * 16 * 16;
pub const OVERWORLD_MIN_Y: i32 = -64;
pub const OVERWORLD_MAX_Y: i32 = 319;
pub const NETHER_MIN_Y: i32 = 0;
pub const NETHER_MAX_Y: i32 = 127;
pub const END_MIN_Y: i32 = 0;
pub const END_MAX_Y: i32 = 255;

pub const DIMENSION_OVERWORLD: i32 = 0;
pub const DIMENSION_NETHER: i32 = 1;
pub const DIMENSION_END: i32 = 2;

/// Vertical block bounds (inclusive) for a vanilla dimension id.
pub const fn dimension_bounds(dimension: i32) -> (i32, i32) {
    match dimension {
        DIMENSION_NETHER => (NETHER_MIN_Y, NETHER_MAX_Y),
        DIMENSION_END => (END_MIN_Y, END_MAX_Y),
        _ => (OVERWORLD_MIN_Y, OVERWORLD_MAX_Y),
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ChunkPosition {
    pub x: i32,
    pub z: i32,
}

impl ChunkPosition {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub fn from_world(x: i32, z: i32) -> Self {
        Self::new(x.div_euclid(SUBCHUNK_SIZE), z.div_euclid(SUBCHUNK_SIZE))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SubChunkIndex {
    pub y: i8,
}

impl SubChunkIndex {
    pub const fn new(y: i8) -> Self {
        Self { y }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LocalBlockPosition {
    pub x: u8,
    pub y: u8,
    pub z: u8,
}

impl LocalBlockPosition {
    pub fn new(x: u8, y: u8, z: u8) -> Option<Self> {
        (x < 16 && y < 16 && z < 16).then_some(Self { x, y, z })
    }

    /// Bedrock paletted-storage index order: XZY
    /// (`(x << 8) | (z << 4) | y`) — x-major, the order used by both the
    /// LevelDB persistent format and the network sub-chunk format
    /// (shared by the LevelDB persistent format and the network sub-chunk format). Do NOT confuse with the
    /// Java Edition YZX order.
    pub const fn linear_index(self) -> usize {
        ((self.x as usize) << 8) | ((self.z as usize) << 4) | self.y as usize
    }
}

/// Internal block state id. LevelDB parsing stores the FNV1a state hash here.
/// For protocol 2168 hashed mode, the network layer serializes this value
/// directly; legacy/non-hashed packet paths may still map it through the
/// version-pack runtime-id dictionary.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct BlockRuntimeId(pub u32);

// ---------------------------------------------------------------------------
// Bit-packed storage (u32 word layout shared by disk and network)
// ---------------------------------------------------------------------------

/// Palette entry count to Bedrock bit width (1/2/3/4/5/6/8/16 set).
///
/// Canonical bit width of in-memory `Paletted*Storage::words`; the network layer uses the same
/// widths (V2 bit array, minimum 2 bits), so network sends can copy packed words
/// directly. Persistent (LevelDB) width is 1 bit only at 2 or fewer entries;
/// disk read/write repacks across that single divergence.
#[inline]
pub fn palette_bits(palette_len: usize) -> u8 {
    match palette_len {
        0..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        17..=32 => 5,
        33..=64 => 6,
        65..=256 => 8,
        _ => 16,
    }
}

/// Entries per u32 word (3 bits to 10, 5 bits to 6, 6 bits to 5; high bits are padding).
#[inline]
pub fn values_per_word(bits: u8) -> usize {
    32 / bits as usize
}

/// u32 words needed to pack 4096 entries.
#[inline]
pub fn packed_word_count(bits: u8) -> usize {
    let vpw = values_per_word(bits);
    (SUBCHUNK_VOLUME + vpw - 1) / vpw
}

/// Read one bit-packed entry (LSB first, matching the disk/network word layout).
#[inline]
pub fn read_packed_word(words: &[u32], bits: u8, index: usize) -> Option<u32> {
    let vpw = values_per_word(bits);
    let word = *words.get(index / vpw)?;
    let shift = (index % vpw) * bits as usize;
    Some((word >> shift) & ((1u32 << bits) - 1))
}

/// Write one bit-packed entry (LSB first).
#[inline]
pub fn write_packed_word(words: &mut [u32], bits: u8, index: usize, value: u32) {
    let vpw = values_per_word(bits);
    let shift = (index % vpw) * bits as usize;
    let mask = (1u32 << bits) - 1;
    let word = &mut words[index / vpw];
    *word = (*word & !(mask << shift)) | ((value & mask) << shift);
}

/// Bit-width migration: `from_bits` packed words to `to_bits` packed words (all 4096 entries).
/// Only called when palette growth crosses a width step or disk/canonical widths diverge (rare path).
pub fn repack_packed_words(words: &[u32], from_bits: u8, to_bits: u8) -> Box<[u32]> {
    if from_bits == to_bits {
        return words.to_vec().into_boxed_slice();
    }
    let mut out = vec![0u32; packed_word_count(to_bits)];
    for index in 0..SUBCHUNK_VOLUME {
        if let Some(value) = read_packed_word(words, from_bits, index) {
            write_packed_word(&mut out, to_bits, index, value);
        } else {
            break;
        }
    }
    out.into_boxed_slice()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PalettedBlockStorage {
    Single(BlockRuntimeId),
    Paletted {
        palette: Vec<BlockRuntimeId>,
        /// Bit-packed palette indices: [`palette_bits(palette.len())`] bits per entry,
        /// LSB first within a word, `32 / bits` entries per word. Matches the network V2 bit-array layout
        /// word for word, so it can be sent as whole blocks.
        words: Box<[u32]>,
    },
}

impl PalettedBlockStorage {
    pub fn single(value: BlockRuntimeId) -> Self {
        Self::Single(value)
    }

    /// Build from an explicit index table (generator/test path), packed at the canonical width.
    pub fn from_indices(palette: Vec<BlockRuntimeId>, indices: &[u16]) -> Self {
        if palette.len() == 1 {
            return Self::Single(palette[0]);
        }
        let bits = palette_bits(palette.len());
        let mut words = vec![0u32; packed_word_count(bits)];
        for (index, palette_index) in indices.iter().enumerate().take(SUBCHUNK_VOLUME) {
            write_packed_word(&mut words, bits, index, u32::from(*palette_index));
        }
        Self::Paletted {
            palette,
            words: words.into_boxed_slice(),
        }
    }

    /// Build from on-disk-width packed words (LevelDB read path): repacks when the width
    /// disagrees with canonical (single divergence: disk 1 bit vs canonical 2 bits).
    pub fn from_packed(palette: Vec<BlockRuntimeId>, words: Vec<u32>, bits: u8) -> Self {
        if palette.len() == 1 {
            return Self::Single(palette[0]);
        }
        let canonical = palette_bits(palette.len());
        let words = if bits == canonical {
            words.into_boxed_slice()
        } else {
            repack_packed_words(&words, bits, canonical)
        };
        Self::Paletted { palette, words }
    }

    /// Canonical-width packed words (the network layer can send them word for word).
    pub fn packed_words(&self) -> Option<&[u32]> {
        match self {
            Self::Single(_) => None,
            Self::Paletted { words, .. } => Some(words),
        }
    }

    pub fn get(&self, index: usize) -> Option<BlockRuntimeId> {
        if index >= SUBCHUNK_VOLUME {
            return None;
        }
        match self {
            Self::Single(value) => Some(*value),
            Self::Paletted { palette, words } => {
                let bits = palette_bits(palette.len());
                let palette_index = read_packed_word(words, bits, index)? as usize;
                palette.get(palette_index).copied()
            }
        }
    }

    /// Write the block at one index, returning the old value.
    /// Lazy upgrade: `Single` with an equal value is a zero-cost no-op; a different value upgrades to
    /// `Paletted` (one 1KB packed-word allocation, then no more allocation for writes in the store).
    /// Palette growth across a width step repacks everything (rare, 4096 shift-mask ops).
    pub fn set(&mut self, index: usize, value: BlockRuntimeId) -> Option<BlockRuntimeId> {
        if index >= SUBCHUNK_VOLUME {
            return None;
        }
        if let Self::Paletted { palette, .. } = self {
            if palette.len() >= SUBCHUNK_VOLUME && !palette.contains(&value) {
                return Self::compact_and_replace(self, index, value);
            }
        }
        match self {
            Self::Single(current) => {
                let previous = *current;
                if previous == value {
                    return Some(previous);
                }
                let bits = palette_bits(2);
                let mut words = vec![0u32; packed_word_count(bits)];
                write_packed_word(&mut words, bits, index, 1);
                *self = Self::Paletted {
                    palette: vec![previous, value],
                    words: words.into_boxed_slice(),
                };
                Some(previous)
            }
            Self::Paletted { palette, words } => {
                let old_bits = palette_bits(palette.len());
                let previous = match read_packed_word(words, old_bits, index) {
                    Some(palette_index) => palette.get(palette_index as usize).copied()?,
                    None => return None,
                };
                // A subchunk holds at most 4096 distinct states; linear lookup over real-world palette
                // sizes (ones to dozens) beats hashing.
                let palette_index = match palette.iter().position(|entry| *entry == value) {
                    Some(position) => position,
                    None => {
                        palette.push(value);
                        palette.len() - 1
                    }
                };
                let new_bits = palette_bits(palette.len());
                if new_bits != old_bits {
                    *words = repack_packed_words(words, old_bits, new_bits);
                }
                write_packed_word(words, new_bits, index, palette_index as u32);
                Some(previous)
            }
        }
    }

    /// Rebuild a saturated palette while applying one replacement. Palettes
    /// may contain stale states after repeated edits; compact at the Bedrock
    /// subchunk state-count limit so persistence never sees an unbounded
    /// palette or an index wider than the storage format supports.
    fn compact_and_replace(
        storage: &mut Self,
        index: usize,
        value: BlockRuntimeId,
    ) -> Option<BlockRuntimeId> {
        let (palette, words) = match storage {
            Self::Single(_) => return None,
            Self::Paletted { palette, words } => (palette, words),
        };
        let old_bits = palette_bits(palette.len());
        let previous_index = read_packed_word(words, old_bits, index)? as usize;
        let previous = *palette.get(previous_index)?;

        // Packed indices are u16-bounded. Any larger palette entries are
        // unreachable and can be discarded during this normalization.
        let mut remap = vec![u32::MAX; palette.len().min(u16::MAX as usize + 1)];
        let mut compacted_palette = Vec::with_capacity(palette.len().min(SUBCHUNK_VOLUME));
        let mut compacted_indices = vec![0u16; SUBCHUNK_VOLUME];
        for (position, mapped_index) in compacted_indices.iter_mut().enumerate() {
            if position == index {
                let new_index = u16::try_from(compacted_palette.len()).ok()?;
                compacted_palette.push(value);
                *mapped_index = new_index;
                continue;
            }

            let old_index = read_packed_word(words, old_bits, position)? as usize;
            let entry = remap.get_mut(old_index)?;
            if *entry == u32::MAX {
                *entry = u32::try_from(compacted_palette.len()).ok()?;
                compacted_palette.push(*palette.get(old_index)?);
            }
            *mapped_index = u16::try_from(*entry).ok()?;
        }

        let new_bits = palette_bits(compacted_palette.len());
        let mut compacted_words = vec![0u32; packed_word_count(new_bits)];
        for (position, palette_index) in compacted_indices.into_iter().enumerate() {
            write_packed_word(
                &mut compacted_words,
                new_bits,
                position,
                u32::from(palette_index),
            );
        }
        *storage = Self::Paletted {
            palette: compacted_palette,
            words: compacted_words.into_boxed_slice(),
        };
        Some(previous)
    }

    pub fn is_single(&self) -> bool {
        matches!(self, Self::Single(_))
    }
}

/// A 3D biome storage for one subchunk section, mirroring the block storage
/// layout but with plain numeric biome ids as palette entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PalettedBiomeStorage {
    Single(u32),
    Paletted {
        palette: Vec<u32>,
        /// Bit-packed palette indices (same layout as [`PalettedBlockStorage::Paletted`]).
        words: Box<[u32]>,
    },
}

impl PalettedBiomeStorage {
    pub fn single(value: u32) -> Self {
        Self::Single(value)
    }

    /// Build from an explicit index table (generator/test path).
    pub fn from_indices(palette: Vec<u32>, indices: &[u16]) -> Self {
        if palette.len() == 1 {
            return Self::Single(palette[0]);
        }
        let bits = palette_bits(palette.len());
        let mut words = vec![0u32; packed_word_count(bits)];
        for (index, palette_index) in indices.iter().enumerate().take(SUBCHUNK_VOLUME) {
            write_packed_word(&mut words, bits, index, u32::from(*palette_index));
        }
        Self::Paletted {
            palette,
            words: words.into_boxed_slice(),
        }
    }

    /// Build from on-disk-width packed words (LevelDB read path): repacks on width mismatch.
    pub fn from_packed(palette: Vec<u32>, words: Vec<u32>, bits: u8) -> Self {
        if palette.len() == 1 {
            return Self::Single(palette[0]);
        }
        let canonical = palette_bits(palette.len());
        let words = if bits == canonical {
            words.into_boxed_slice()
        } else {
            repack_packed_words(&words, bits, canonical)
        };
        Self::Paletted { palette, words }
    }

    /// Packed words at spec width (the network layer sends them verbatim).
    pub fn packed_words(&self) -> Option<&[u32]> {
        match self {
            Self::Single(_) => None,
            Self::Paletted { words, .. } => Some(words),
        }
    }

    pub fn get(&self, index: usize) -> Option<u32> {
        if index >= SUBCHUNK_VOLUME {
            return None;
        }
        match self {
            Self::Single(value) => Some(*value),
            Self::Paletted { palette, words } => {
                let bits = palette_bits(palette.len());
                let palette_index = read_packed_word(words, bits, index)? as usize;
                palette.get(palette_index).copied()
            }
        }
    }

    /// Write the biome id at one index, returning the old value (same semantics as PalettedBlockStorage::set).
    pub fn set(&mut self, index: usize, value: u32) -> Option<u32> {
        if index >= SUBCHUNK_VOLUME {
            return None;
        }
        match self {
            Self::Single(current) => {
                let previous = *current;
                if previous == value {
                    return Some(previous);
                }
                let bits = palette_bits(2);
                let mut words = vec![0u32; packed_word_count(bits)];
                write_packed_word(&mut words, bits, index, 1);
                *self = Self::Paletted {
                    palette: vec![previous, value],
                    words: words.into_boxed_slice(),
                };
                Some(previous)
            }
            Self::Paletted { palette, words } => {
                let old_bits = palette_bits(palette.len());
                let previous = match read_packed_word(words, old_bits, index) {
                    Some(palette_index) => palette.get(palette_index as usize).copied()?,
                    None => return None,
                };
                let palette_index = match palette.iter().position(|entry| *entry == value) {
                    Some(position) => position,
                    None => {
                        palette.push(value);
                        palette.len() - 1
                    }
                };
                let new_bits = palette_bits(palette.len());
                if new_bits != old_bits {
                    *words = repack_packed_words(words, old_bits, new_bits);
                }
                write_packed_word(words, new_bits, index, palette_index as u32);
                Some(previous)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubChunk {
    pub index: SubChunkIndex,
    pub layers: Vec<PalettedBlockStorage>,
}

impl SubChunk {
    pub fn empty(index: SubChunkIndex, air: BlockRuntimeId) -> Self {
        Self {
            index,
            layers: vec![PalettedBlockStorage::single(air)],
        }
    }

    pub fn empty_network(index: SubChunkIndex) -> Self {
        Self {
            index,
            layers: Vec::new(),
        }
    }

    /// Write the block at the given layer, returning the old value. Missing layers lazily materialize
    /// as all-air storage (writing air to a missing layer is a no-op returning air).
    pub fn set_block(
        &mut self,
        layer: usize,
        local: LocalBlockPosition,
        value: BlockRuntimeId,
    ) -> Option<BlockRuntimeId> {
        const MAX_LAYERS: usize = 2; // Layer 0 is blocks, layer 1 is the second layer such as water
        if layer >= MAX_LAYERS {
            return None;
        }
        let air = BlockRuntimeId(crate::block_dictionary::air_runtime_id());
        if self.layers.len() <= layer && value == air {
            return Some(air);
        }
        while self.layers.len() <= layer {
            self.layers.push(PalettedBlockStorage::single(air));
        }
        self.layers[layer].set(local.linear_index(), value)
    }

    /// Read the block at the given layer. Missing layers read as air.
    pub fn get_block(&self, layer: usize, local: LocalBlockPosition) -> Option<BlockRuntimeId> {
        match self.layers.get(layer) {
            Some(storage) => storage.get(local.linear_index()),
            None => Some(BlockRuntimeId(crate::block_dictionary::air_runtime_id())),
        }
    }

    /// Whether the subchunk is all air: no layers (empty section) or air-only entries in every layer.
    /// Counts empty sections for the network `subChunkCount`;
    /// Counts empty sections for the network `subChunkCount`.
    pub fn is_all_air(&self, air: BlockRuntimeId) -> bool {
        self.layers.iter().all(|storage| match storage {
            PalettedBlockStorage::Single(id) => *id == air,
            PalettedBlockStorage::Paletted { palette, .. } => palette.iter().all(|id| *id == air),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Chunk {
    pub position: ChunkPosition,
    pub dimension: i32,
    pub min_y: i32,
    pub max_y: i32,
    pub subchunks: Vec<SubChunk>,
    /// One entry per subchunk section, bottom-up. Empty when the source world
    /// had no 3D biome data; the network layer then sends a zeroed fallback.
    pub biomes: Vec<PalettedBiomeStorage>,
    /// Block entity NBT compounds exactly as stored on disk. Unknown fields
    /// are preserved so re-serialization does not lose data.
    pub block_entities: Vec<NbtValue>,
    /// Raw 512-byte Bedrock Data3D heightmap prefix when known. Keeping the
    /// bytes opaque preserves storage semantics across unrelated metadata edits.
    pub data_3d_heightmap: Option<Arc<[u8; 512]>>,
}

impl Chunk {
    pub fn empty_overworld(position: ChunkPosition) -> Self {
        Self::empty(
            position,
            DIMENSION_OVERWORLD,
            OVERWORLD_MIN_Y,
            OVERWORLD_MAX_Y,
        )
    }

    pub fn empty(position: ChunkPosition, dimension: i32, min_y: i32, max_y: i32) -> Self {
        let height = max_y
            .checked_sub(min_y)
            .and_then(|height| height.checked_add(1))
            .unwrap_or(0);
        let requested_count = if height > 0 {
            (height as u32).saturating_add(SUBCHUNK_SIZE as u32 - 1) / SUBCHUNK_SIZE as u32
        } else {
            0
        } as usize;
        let first = min_y.div_euclid(SUBCHUNK_SIZE);
        let representable_count = if (i8::MIN as i32..=i8::MAX as i32).contains(&first) {
            (i8::MAX as i32 - first + 1) as usize
        } else {
            0
        };
        let count = requested_count.min(representable_count);
        // Empty layers (0 layers): encode only the bottom placeholder plus real block sections,
        // never an explicit air fill (air is the client default).
        let subchunks = (0..count)
            .map(|offset| SubChunk::empty_network(SubChunkIndex::new(first as i8 + offset as i8)))
            .collect();
        Self {
            position,
            dimension,
            min_y,
            max_y,
            subchunks,
            biomes: Vec::new(),
            block_entities: Vec::new(),
            data_3d_heightmap: None,
        }
    }

    pub fn subchunk_count(&self) -> usize {
        self.subchunks.len()
    }

    pub fn estimated_memory_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>()
            + self.subchunks.capacity() * std::mem::size_of::<SubChunk>()
            + self.biomes.capacity() * std::mem::size_of::<PalettedBiomeStorage>()
            + self.block_entities.capacity() * std::mem::size_of::<NbtValue>();
        if self.data_3d_heightmap.is_some() {
            bytes += 512;
        }
        for subchunk in &self.subchunks {
            bytes += subchunk.layers.capacity() * std::mem::size_of::<PalettedBlockStorage>();
            for layer in &subchunk.layers {
                if let PalettedBlockStorage::Paletted { palette, words } = layer {
                    bytes += palette.capacity() * std::mem::size_of::<BlockRuntimeId>()
                        + words.len() * std::mem::size_of::<u32>();
                }
            }
        }
        for biome in &self.biomes {
            if let PalettedBiomeStorage::Paletted { palette, words } = biome {
                bytes += palette.capacity() * std::mem::size_of::<u32>()
                    + words.len() * std::mem::size_of::<u32>();
            }
        }
        bytes
    }

    /// Conservative estimate of the heap requested by `Chunk::clone()` for a
    /// writeback snapshot. Compound NBT maps are shared through Arc, while
    /// owned NBT strings, arrays and list buffers are duplicated.
    pub fn estimated_writeback_snapshot_bytes(&self) -> usize {
        self.block_entities
            .iter()
            .fold(self.estimated_memory_bytes(), |total, value| {
                total.saturating_add(value.estimated_clone_heap_bytes())
            })
    }

    /// Array index of the highest non-empty subchunk in the column (0-based, bottom-up; index 0 is the lowest world section).
    /// Returns `None` for an all-air column.
    ///
    /// Non-empty matches the network-send semantics: any layer palette holding a non-air entry counts as non-empty.
    /// (`SubChunk::is_all_air`).
    pub fn highest_non_empty_section(&self) -> Option<usize> {
        let air = BlockRuntimeId(crate::block_dictionary::air_runtime_id());
        self.subchunks
            .iter()
            .enumerate()
            .rev()
            .find(|(_, subchunk)| !subchunk.is_all_air(air))
            .map(|(index, _)| index)
    }

    /// Network `LevelChunk.subChunkCount` value: subchunk count from the bottom to the highest non-empty section
    /// (including bottom empty sections and the highest non-empty one; 0 when all empty).
    ///
    /// Payload holds exactly as many section headers as the field value, headers ordered bottom-up,
    /// each carrying the true world-section y; negative-y empty sections are themselves
    /// the placeholder, so no extra placeholder data is prepended.
    /// 3D biomes still send the full dimension height (24 sections), independent of the section count.
    pub fn network_subchunk_count(&self) -> usize {
        self.highest_non_empty_section()
            .map_or(0, |index| index + 1)
    }

    /// Array index into `subchunks` for the subchunk containing world_y.
    pub fn section_of(&self, world_y: i32) -> Option<usize> {
        if world_y < self.min_y || world_y > self.max_y {
            return None;
        }
        Some(((world_y - self.min_y) / SUBCHUNK_SIZE) as usize)
    }

    /// Read a block (layer 0). Out-of-range y returns None; empty subchunks read as air.
    pub fn block_at(&self, local_x: u8, world_y: i32, local_z: u8) -> Option<BlockRuntimeId> {
        self.block_at_layer(0, local_x, world_y, local_z)
    }

    pub fn block_at_layer(
        &self,
        layer: usize,
        local_x: u8,
        world_y: i32,
        local_z: u8,
    ) -> Option<BlockRuntimeId> {
        let section = self.section_of(world_y)?;
        let subchunk = self.subchunks.get(section)?;
        let local = LocalBlockPosition::new(
            local_x,
            ((world_y - self.min_y) % SUBCHUNK_SIZE) as u8,
            local_z,
        )?;
        subchunk.get_block(layer, local)
    }

    /// Highest non-air block y in the column (scans top-down, skipping fully empty subchunks).
    pub fn highest_block_at(&self, local_x: u8, local_z: u8, air: BlockRuntimeId) -> Option<i32> {
        for (section, subchunk) in self.subchunks.iter().enumerate().rev() {
            if subchunk.layers.is_empty() {
                continue;
            }
            let base = self.min_y + (section as i32) * SUBCHUNK_SIZE;
            for offset in (0..SUBCHUNK_SIZE).rev() {
                let local = LocalBlockPosition::new(local_x, offset as u8, local_z)?;
                match subchunk.get_block(0, local) {
                    Some(id) if id != air => return Some(base + offset),
                    _ => {}
                }
            }
        }
        None
    }

    /// Write a block, returning the old value. Out-of-range y/coords return None.
    pub fn set_block_at(
        &mut self,
        layer: usize,
        local_x: u8,
        world_y: i32,
        local_z: u8,
        value: BlockRuntimeId,
    ) -> Option<BlockRuntimeId> {
        let section = self.section_of(world_y)?;
        let subchunk = self.subchunks.get_mut(section)?;
        let local = LocalBlockPosition::new(
            local_x,
            ((world_y - self.min_y) % SUBCHUNK_SIZE) as u8,
            local_z,
        )?;
        subchunk.set_block(layer, local, value)
    }
}

impl fmt::Display for ChunkPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_world_coordinates_use_floor_chunk_division() {
        assert_eq!(
            ChunkPosition::from_world(-1, -17),
            ChunkPosition::new(-1, -2)
        );
    }

    #[test]
    fn overworld_has_24_subchunks() {
        assert_eq!(
            Chunk::empty_overworld(ChunkPosition::new(0, 0)).subchunk_count(),
            24
        );
    }

    #[test]
    fn network_subchunk_count_matches_highest_non_empty_section() {
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let air = crate::block_dictionary::air_runtime_id();
        // All empty: field 0.
        assert_eq!(chunk.network_subchunk_count(), 0);
        assert_eq!(chunk.highest_non_empty_section(), None);

        // Flat ground at the lowest section (array index 0): field 1.
        chunk.set_block_at(0, 0, -64, 0, BlockRuntimeId(1000));
        assert_eq!(chunk.network_subchunk_count(), 1);
        assert_eq!(chunk.highest_non_empty_section(), Some(0));

        // Ground raised to section 0 (array index 4): field 5.
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(1, 1));
        chunk.set_block_at(0, 0, 0, 0, BlockRuntimeId(1000));
        assert_eq!(chunk.network_subchunk_count(), 5);
        assert_eq!(chunk.highest_non_empty_section(), Some(4));

        // Highest section 19 (y=319): 24 sections in the column.
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(2, 2));
        chunk.set_block_at(0, 0, 319, 0, BlockRuntimeId(1000));
        assert_eq!(chunk.network_subchunk_count(), 24);
        assert_eq!(chunk.highest_non_empty_section(), Some(23));

        // Subchunks with only an air layer (explicit Single(air)) still count as empty.
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(3, 3));
        let single_air = PalettedBlockStorage::single(BlockRuntimeId(air));
        chunk.subchunks[10].layers.push(single_air);
        assert_eq!(chunk.network_subchunk_count(), 0);

        // A non-air second layer (e.g. water) counts as non-empty.
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(4, 4));
        chunk.subchunks[0]
            .layers
            .push(PalettedBlockStorage::single(BlockRuntimeId(1000)));
        assert_eq!(chunk.network_subchunk_count(), 1);
    }

    #[test]
    fn linear_index_is_bedrock_storage_order() {
        // XZY: (x << 8) | (z << 4) | y
        let position = LocalBlockPosition::new(1, 2, 3).unwrap();
        assert_eq!(position.linear_index(), (1 << 8) | (3 << 4) | 2);
        assert_eq!(
            LocalBlockPosition::new(15, 15, 15).unwrap().linear_index(),
            4095
        );
    }

    #[test]
    fn dimension_bounds_match_vanilla() {
        assert_eq!(dimension_bounds(DIMENSION_OVERWORLD), (-64, 319));
        assert_eq!(dimension_bounds(DIMENSION_NETHER), (0, 127));
        assert_eq!(dimension_bounds(DIMENSION_END), (0, 255));
    }

    #[test]
    fn empty_chunk_uses_ceil_for_partial_section_height() {
        let chunk = Chunk::empty(ChunkPosition::new(0, 0), 0, 0, 16);
        assert_eq!(chunk.subchunks.len(), 2);
    }

    #[test]
    fn storage_set_upgrades_single_lazily() {
        let mut storage = PalettedBlockStorage::single(BlockRuntimeId(1));
        // Writing the same value does not upgrade.
        assert_eq!(storage.set(0, BlockRuntimeId(1)), Some(BlockRuntimeId(1)));
        assert!(storage.is_single());
        // A different value upgrades to a two-entry palette.
        assert_eq!(storage.set(5, BlockRuntimeId(2)), Some(BlockRuntimeId(1)));
        assert!(!storage.is_single());
        assert_eq!(storage.get(5), Some(BlockRuntimeId(2)));
        assert_eq!(storage.get(0), Some(BlockRuntimeId(1)));
        // Existing entries are reused without growing the palette.
        assert_eq!(storage.set(6, BlockRuntimeId(2)), Some(BlockRuntimeId(1)));
        match &storage {
            PalettedBlockStorage::Paletted { palette, .. } => assert_eq!(palette.len(), 2),
            _ => unreachable!(),
        }
    }

    #[test]
    fn palette_growth_crossing_bit_boundaries_repacks_correctly() {
        // Grow the palette across 2/3/4/5-bit steps and verify old values still read back after repacking.
        let mut storage = PalettedBlockStorage::single(BlockRuntimeId(1000));
        let distinct: Vec<u32> = (1000..1040).collect(); // 40 entries, 6 bits
        for (index, value) in distinct.iter().enumerate() {
            let _ = storage.set(index, BlockRuntimeId(*value));
        }
        for (index, value) in distinct.iter().enumerate() {
            assert_eq!(storage.get(index), Some(BlockRuntimeId(*value)));
        }
        // Untouched positions keep the Single-era value (1000 is palette[0]).
        assert_eq!(storage.get(4095), Some(BlockRuntimeId(1000)));
        match &storage {
            PalettedBlockStorage::Paletted { palette, words } => {
                assert_eq!(palette.len(), 40);
                assert_eq!(words.len(), packed_word_count(palette_bits(40)));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn saturated_palette_compacts_stale_entries_before_persistence_limit() {
        let palette = (0..SUBCHUNK_VOLUME as u32)
            .map(BlockRuntimeId)
            .collect::<Vec<_>>();
        let indices = vec![0u16; SUBCHUNK_VOLUME];
        let mut storage = PalettedBlockStorage::from_indices(palette, &indices);

        assert_eq!(
            storage.set(0, BlockRuntimeId(50_000)),
            Some(BlockRuntimeId(0))
        );
        assert_eq!(storage.get(0), Some(BlockRuntimeId(50_000)));
        assert_eq!(storage.get(1), Some(BlockRuntimeId(0)));
        match &storage {
            PalettedBlockStorage::Paletted { palette, words } => {
                assert_eq!(palette.len(), 2, "unused historical entries are removed");
                assert_eq!(words.len(), packed_word_count(palette_bits(2)));
            }
            PalettedBlockStorage::Single(_) => unreachable!(),
        }
    }

    #[test]
    fn saturated_palette_replaces_an_entry_when_every_state_is_live() {
        let palette = (0..SUBCHUNK_VOLUME as u32)
            .map(BlockRuntimeId)
            .collect::<Vec<_>>();
        let indices = (0..SUBCHUNK_VOLUME as u16).collect::<Vec<_>>();
        let mut storage = PalettedBlockStorage::from_indices(palette, &indices);

        assert_eq!(
            storage.set(123, BlockRuntimeId(50_000)),
            Some(BlockRuntimeId(123))
        );
        assert_eq!(storage.get(122), Some(BlockRuntimeId(122)));
        assert_eq!(storage.get(123), Some(BlockRuntimeId(50_000)));
        assert_eq!(storage.get(124), Some(BlockRuntimeId(124)));
        match &storage {
            PalettedBlockStorage::Paletted { palette, words } => {
                assert_eq!(palette.len(), SUBCHUNK_VOLUME);
                assert_eq!(words.len(), packed_word_count(palette_bits(palette.len())));
            }
            PalettedBlockStorage::Single(_) => unreachable!(),
        }
    }

    #[test]
    fn from_indices_packs_at_canonical_bits() {
        let palette = vec![BlockRuntimeId(7), BlockRuntimeId(8), BlockRuntimeId(9)];
        let mut indices = vec![0u16; SUBCHUNK_VOLUME];
        indices[0] = 2;
        indices[4095] = 1;
        let storage = PalettedBlockStorage::from_indices(palette, &indices);
        assert_eq!(storage.get(0), Some(BlockRuntimeId(9)));
        assert_eq!(storage.get(4095), Some(BlockRuntimeId(8)));
        assert_eq!(storage.get(1), Some(BlockRuntimeId(7)));
        match &storage {
            PalettedBlockStorage::Paletted { palette, words } => {
                // 3 entries use canonical 2 bits, 256 words.
                assert_eq!(palette_bits(palette.len()), 2);
                assert_eq!(words.len(), 256);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn from_packed_repacks_one_bit_disk_words() {
        // Disk 1 bit (2 entries) to canonical 2 bits: semantics unchanged.
        let palette = vec![BlockRuntimeId(1), BlockRuntimeId(2)];
        // 128 words with index-5 bit set (word 0 bit 5).
        let mut words = vec![0u32; packed_word_count(1)];
        words[0] = 1 << 5;
        let storage = PalettedBlockStorage::from_packed(palette, words, 1);
        assert_eq!(storage.get(5), Some(BlockRuntimeId(2)));
        assert_eq!(storage.get(0), Some(BlockRuntimeId(1)));
        match &storage {
            PalettedBlockStorage::Paletted { words, .. } => assert_eq!(words.len(), 256),
            _ => unreachable!(),
        }
    }

    #[test]
    fn biome_storage_bitpacked_roundtrip() {
        let mut storage = PalettedBiomeStorage::single(1);
        assert_eq!(storage.set(100, 9), Some(1));
        assert_eq!(storage.get(100), Some(9));
        assert_eq!(storage.get(0), Some(1));
        // Overwrites the old value.
        assert_eq!(storage.set(100, 3), Some(9));
        assert_eq!(storage.get(100), Some(3));
    }

    #[test]
    fn chunk_set_and_get_roundtrip_including_empty_section() {
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let air = crate::block_dictionary::air_runtime_id();
        // Empty subchunks read as air.
        assert_eq!(chunk.block_at(3, 100, 4), Some(BlockRuntimeId(air)));
        // Reads back after writing.
        let previous = chunk.set_block_at(0, 3, 100, 4, BlockRuntimeId(777));
        assert_eq!(previous, Some(BlockRuntimeId(air)));
        assert_eq!(chunk.block_at(3, 100, 4), Some(BlockRuntimeId(777)));
        // Adjacent coords are unaffected.
        assert_eq!(chunk.block_at(3, 101, 4), Some(BlockRuntimeId(air)));
        // Out of range.
        assert_eq!(chunk.set_block_at(0, 0, 320, 0, BlockRuntimeId(1)), None);
        assert_eq!(chunk.block_at(0, -65, 0), None);
    }
}
