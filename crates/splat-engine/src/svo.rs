//! Reader for SuperSplat / SplatTransform collision voxels (`scene.voxel.json` + `scene.voxel.bin`, version 1.x).
//!
//! The format is a sparse voxel octree at `voxelResolution` metres per cell over `gridBounds`. Each node is one
//! `u32` word:
//! - `0xff000000`: the whole node is solid.
//! - Interior node: the top byte is the child mask (bit = octant, x is bit 0). The low 24 bits index the first child,
//!   and children are stored contiguously in popcount order.
//! - Leaf (at `treeDepth`): the low 24 bits index a *mixed* leaf. Each mixed leaf is two words: a 64-bit mask of a
//!   4×4×4 block (bit = x + 4y + 16z). Mixed-leaf words live after all node words.
//!
//! This follows the reader in the earlier DungeonFlux web code, which read the same files.

use std::fs;
use std::path::Path;

const SOLID: u32 = 0xff00_0000;
const INDEX_MASK: u32 = 0x00ff_ffff;
/// Cells per leaf edge.
pub const LEAF_SIZE: u32 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub grid_min: [f32; 3],
    pub grid_max: [f32; 3],
    pub resolution: f32,
    pub tree_depth: u32,
    pub node_count: usize,
    pub leaf_data_count: usize,
}

#[derive(Debug)]
pub struct Octree {
    pub meta: Meta,
    words: Vec<u32>,
}

/// Grid dimensions in cells: `ceil((max - min) / resolution)` per axis.
pub fn grid_cells(meta: &Meta) -> [u32; 3] {
    [0, 1, 2].map(|i| ((meta.grid_max[i] - meta.grid_min[i]) / meta.resolution).ceil() as u32)
}

impl Octree {
    /// Loads `scene.voxel.json` and `scene.voxel.bin` from `dir`.
    pub fn load(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_json_and_bin(&fs::read(dir.join("scene.voxel.json"))?, &fs::read(dir.join("scene.voxel.bin"))?)
    }

    /// Builds the octree from the two files' contents, for callers without a file system (the browser build).
    pub fn from_json_and_bin(json: &[u8], bin: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let meta = parse_meta(&serde_json::from_slice(json)?)?;
        Self::from_bytes(meta, bin)
    }

    /// Builds the octree from metadata and the raw little-endian `.bin` contents, checking the counts first.
    pub fn from_bytes(meta: Meta, bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let expected = meta.node_count + meta.leaf_data_count;
        if bytes.len() != expected * 4 {
            return Err(format!("voxel binary is {} bytes; expected {} words", bytes.len(), expected).into());
        }
        let words: Vec<u32> = bytes.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        Ok(Self { meta, words })
    }

    /// Calls `visit` with the index of every occupied cell, relative to `grid_min`, in x-y-z cell units.
    /// Each mixed leaf contributes its occupied cells; solid nodes contribute every cell they cover.
    pub fn for_each_occupied(&self, mut visit: impl FnMut([u32; 3])) {
        let dims = grid_cells(&self.meta);
        // (node index, depth, origin in cells)
        let mut stack: Vec<(usize, u32, [u32; 3])> = vec![(0, 0, [0, 0, 0])];
        while let Some((node, depth, origin)) = stack.pop() {
            let word = self.words[node];
            let span = LEAF_SIZE << (self.meta.tree_depth - depth);
            if word == SOLID {
                for z in origin[2]..(origin[2] + span).min(dims[2]) {
                    for y in origin[1]..(origin[1] + span).min(dims[1]) {
                        for x in origin[0]..(origin[0] + span).min(dims[0]) {
                            visit([x, y, z]);
                        }
                    }
                }
                continue;
            }
            if depth >= self.meta.tree_depth {
                let base = self.meta.node_count + (word & INDEX_MASK) as usize * 2;
                for bit in 0..64u32 {
                    if self.words[base + (bit / 32) as usize] & (1 << (bit % 32)) != 0 {
                        let cell = [origin[0] + (bit & 3), origin[1] + ((bit >> 2) & 3), origin[2] + ((bit >> 4) & 3)];
                        if cell[0] < dims[0] && cell[1] < dims[1] && cell[2] < dims[2] {
                            visit(cell);
                        }
                    }
                }
                continue;
            }
            let mask = word >> 24;
            let first = (word & INDEX_MASK) as usize;
            let child_span = LEAF_SIZE << (self.meta.tree_depth - depth - 1);
            let mut offset = 0usize;
            for octant in 0..8u32 {
                if mask & (1 << octant) != 0 {
                    let child_origin = [
                        origin[0] + if octant & 1 != 0 { child_span } else { 0 },
                        origin[1] + if octant & 2 != 0 { child_span } else { 0 },
                        origin[2] + if octant & 4 != 0 { child_span } else { 0 },
                    ];
                    stack.push((first + offset, depth + 1, child_origin));
                    offset += 1;
                }
            }
        }
    }

    /// Per-block state of the whole grid, `0` empty, `1` solid, `2` mixed, indexed `(bz * nbx + bx) * nby + by`.
    /// A block is one leaf (4×4×4 cells). Solid nodes fill every block they cover, clipped to the grid.
    fn block_states(&self) -> (Vec<u8>, [usize; 3]) {
        let dims = grid_cells(&self.meta);
        let bdims = [0, 1, 2].map(|i| (dims[i] as usize).div_ceil(LEAF_SIZE as usize));
        let mut state = vec![0u8; bdims[0] * bdims[1] * bdims[2]];
        let index = |b: [usize; 3]| (b[2] * bdims[0] + b[0]) * bdims[1] + b[1];
        let mut stack: Vec<(usize, u32, [u32; 3])> = vec![(0, 0, [0, 0, 0])];
        while let Some((node, depth, origin)) = stack.pop() {
            let span = LEAF_SIZE << (self.meta.tree_depth - depth);
            if (0..3).any(|i| origin[i] >= dims[i]) {
                continue;
            }
            let word = self.words[node];
            if word == SOLID || depth >= self.meta.tree_depth {
                let value = if word == SOLID {
                    1
                } else {
                    let base = self.meta.node_count + (word & INDEX_MASK) as usize * 2;
                    let mask = self.words[base] as u64 | ((self.words[base + 1] as u64) << 32);
                    if mask == 0 { continue; }
                    2
                };
                let (lo, hi) = (origin.map(|v| v as usize / LEAF_SIZE as usize), [0, 1, 2].map(|i| ((origin[i] + span).min(dims[i]) as usize).div_ceil(LEAF_SIZE as usize)));
                for bz in lo[2]..hi[2] {
                    for bx in lo[0]..hi[0] {
                        for by in lo[1]..hi[1] {
                            state[index([bx, by, bz])] = value;
                        }
                    }
                }
                continue;
            }
            let mask = word >> 24;
            let first = (word & INDEX_MASK) as usize;
            let child_span = LEAF_SIZE << (self.meta.tree_depth - depth - 1);
            let mut offset = 0usize;
            for octant in 0..8u32 {
                if mask & (1 << octant) != 0 {
                    let child_origin = [
                        origin[0] + if octant & 1 != 0 { child_span } else { 0 },
                        origin[1] + if octant & 2 != 0 { child_span } else { 0 },
                        origin[2] + if octant & 4 != 0 { child_span } else { 0 },
                    ];
                    stack.push((first + offset, depth + 1, child_origin));
                    offset += 1;
                }
            }
        }
        (state, bdims)
    }

    /// Occupancy mask (bit = x + 4y + 16z) of the leaf block that contains `cell`, or `0` when empty.
    fn leaf_mask(&self, cell: [u32; 3]) -> u64 {
        let mut node = 0usize;
        for depth in 0..self.meta.tree_depth {
            let word = self.words[node];
            if word == SOLID {
                return u64::MAX;
            }
            let mask = word >> 24;
            let shift = self.meta.tree_depth - depth - 1 + 2;
            let octant = ((cell[0] >> shift) & 1) | (((cell[1] >> shift) & 1) << 1) | (((cell[2] >> shift) & 1) << 2);
            if mask & (1 << octant) == 0 {
                return 0;
            }
            node = (word & INDEX_MASK) as usize + (mask & ((1 << octant) - 1)).count_ones() as usize;
        }
        let word = self.words[node];
        if word == SOLID {
            return u64::MAX;
        }
        let base = self.meta.node_count + (word & INDEX_MASK) as usize * 2;
        self.words[base] as u64 | ((self.words[base + 1] as u64) << 32)
    }

    /// Walls, per block column (4×4 cells): `Some(r)` where something solid stands within `band` blocks above the
    /// local road, whose top block is `r`. The road under a column is the lowest floor (see `floor_cells`) within
    /// `reach` blocks, so a facade or a post next to the street finds the street's height. Columns with open air above
    /// their own floor are never walls. Returns the grid, indexed `bx * nbz + bz`, and its size `[nbx, nbz]`.
    pub fn wall_columns(&self, clearance: u32, band: (usize, usize), reach: usize) -> (Vec<Option<u32>>, [usize; 2]) {
        let (state, bdims) = self.block_states();
        let clear_blocks = clearance.div_ceil(LEAF_SIZE) as usize;
        let (nbx, nby, nbz) = (bdims[0], bdims[1], bdims[2]);
        let column = |bx: usize, bz: usize| {
            let start = (bz * nbx + bx) * nby;
            &state[start..start + nby]
        };
        // The floor block of every column, as `floor_cells` finds it.
        let mut ground = vec![None::<usize>; nbx * nbz];
        for bz in 0..nbz {
            for bx in 0..nbx {
                let col = column(bx, bz);
                ground[bx * nbz + bz] = (0..nby).find(|&by| {
                    col[by] != 0 && (by + 1..nby).take_while(|&j| col[j] == 0).count() >= clear_blocks.max(1)
                });
            }
        }
        let reach = reach as isize;
        let mut walls = vec![None; nbx * nbz];
        for bz in 0..nbz {
            for bx in 0..nbx {
                let mut road = None::<usize>;
                for dx in -reach..=reach {
                    for dz in -reach..=reach {
                        let (x, z) = (bx as isize + dx, bz as isize + dz);
                        if x < 0 || z < 0 || x >= nbx as isize || z >= nbz as isize {
                            continue;
                        }
                        if let Some(g) = ground[x as usize * nbz + z as usize] {
                            road = Some(road.map_or(g, |r| r.min(g)));
                        }
                    }
                }
                let Some(road) = road else { continue };
                let col = column(bx, bz);
                let (lo, hi) = ((road + band.0).min(nby), (road + band.1).min(nby));
                if col[lo..hi].iter().any(|&b| b != 0) {
                    walls[bx * nbz + bz] = Some(road as u32);
                }
            }
        }
        (walls, [nbx, nbz])
    }

    /// Floor cells: for each 4×4 column of cells, the lowest solid-to-empty surface that has at least `clearance`
    /// cells of air above it (or the top of the grid). Returns cell indices `[x, y, z]` of the solid cells whose top
    /// face is the walkable surface, one per (x, z) column in that footprint.
    ///
    /// Works per block so memory stays at one byte per 64 cells, not per occupied cell.
    pub fn floor_cells(&self, clearance: u32) -> Vec<[u32; 3]> {
        let dims = grid_cells(&self.meta);
        let (state, bdims) = self.block_states();
        let clear_blocks = clearance.div_ceil(LEAF_SIZE) as usize;
        let index = |bx: usize, by: usize, bz: usize| (bz * bdims[0] + bx) * bdims[1] + by;
        let mut floor = Vec::new();
        for bz in 0..bdims[2] {
            for bx in 0..bdims[0] {
                let column = index(bx, 0, bz);
                let col = &state[column..column + bdims[1]];
                // The first block that is occupied with an empty block above it, and at least `clear_blocks` of
                // air above it before the next occupied block (or the top of the grid).
                let mut found = None;
                for by in 0..bdims[1] {
                    if col[by] == 0 {
                        continue;
                    }
                    // Real air must lie above the surface. A column solid up to the grid's top has no walkable surface,
                    // so the grid edge never counts as air.
                    let above = (by + 1..bdims[1]).take_while(|&j| col[j] == 0).count();
                    if above >= clear_blocks.max(1) {
                        found = Some(by);
                        break;
                    }
                }
                let Some(by) = found else { continue };
                let origin = [bx * LEAF_SIZE as usize, by * LEAF_SIZE as usize, bz * LEAF_SIZE as usize];
                if col[by] == 1 {
                    for dz in 0..LEAF_SIZE as usize {
                        for dx in 0..LEAF_SIZE as usize {
                            let (x, z) = (origin[0] + dx, origin[2] + dz);
                            if x < dims[0] as usize && z < dims[2] as usize {
                                floor.push([x as u32, (origin[1] + 3) as u32, z as u32]);
                            }
                        }
                    }
                } else {
                    let mask = self.leaf_mask([origin[0] as u32, origin[1] as u32, origin[2] as u32]);
                    for dz in 0..LEAF_SIZE {
                        for dx in 0..LEAF_SIZE {
                            let top = (0..LEAF_SIZE).rev().find(|&dy| mask & (1u64 << (dx + 4 * dy + 16 * dz)) != 0);
                            if let Some(dy) = top {
                                let (x, y, z) = (origin[0] + dx as usize, origin[1] + dy as usize, origin[2] + dz as usize);
                                if x < dims[0] as usize && z < dims[2] as usize && y < dims[1] as usize {
                                    floor.push([x as u32, y as u32, z as u32]);
                                }
                            }
                        }
                    }
                }
            }
        }
        floor
    }
}

fn parse_meta(json: &serde_json::Value) -> Result<Meta, Box<dyn std::error::Error>> {
    let version = json["version"].as_str().map(str::to_string).unwrap_or_else(|| json["version"].to_string());
    if !(version == "1" || version.starts_with("1.")) {
        return Err(format!("unsupported voxel version {version}; expected 1.x").into());
    }
    let vec3 = |v: &serde_json::Value| -> Result<[f32; 3], Box<dyn std::error::Error>> {
        let a = v.as_array().filter(|a| a.len() == 3).ok_or("expected a 3-element array")?;
        Ok([0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0) as f32))
    };
    let bounds = &json["gridBounds"];
    let grid_min = vec3(&bounds["min"])?;
    let grid_max = vec3(&bounds["max"])?;
    let resolution = json["voxelResolution"].as_f64().ok_or("voxelResolution missing")? as f32;
    if json["leafSize"].as_u64() != Some(LEAF_SIZE as u64) {
        return Err("unsupported leaf size; expected 4".into());
    }
    let tree_depth = json["treeDepth"].as_u64().ok_or("treeDepth missing")? as u32;
    let node_count = json["nodeCount"].as_u64().ok_or("nodeCount missing")? as usize;
    let leaf_data_count = json["leafDataCount"].as_u64().ok_or("leafDataCount missing")? as usize;
    if leaf_data_count % 2 != 0 {
        return Err("leafDataCount must be even (two words per mixed leaf)".into());
    }
    Ok(Meta { grid_min, grid_max, resolution, tree_depth, node_count, leaf_data_count })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A depth-1 tree of side 8 cells. Node 0 is interior with only octant 0; node 1 is that leaf, pointing at
    /// mixed leaf 0. The leaf's 64-bit mask has one occupied cell at (1, 2, 3).
    fn tiny_octree() -> Octree {
        let meta = Meta { grid_min: [0.0; 3], grid_max: [8.0, 8.0, 8.0], resolution: 1.0, tree_depth: 1, node_count: 2, leaf_data_count: 2 };
        let bit = 1 + 2 * 4 + 3 * 16; // x = 1, y = 2, z = 3 inside the 4×4×4 leaf
        let words = vec![
            (0b1u32 << 24) | 1, // interior: child mask 0b1, first child at node 1
            0,                  // leaf: mixed leaf index 0
            if bit < 32 { 1 << bit } else { 0 },
            if bit >= 32 { 1 << (bit - 32) } else { 0 },
        ];
        Octree { meta, words }
    }

    #[test]
    fn occupied_cells_decode_the_leaf_mask() {
        let tree = tiny_octree();
        let mut cells = Vec::new();
        tree.for_each_occupied(|c| cells.push(c));
        assert_eq!(cells, vec![[1, 2, 3]]);
    }

    #[test]
    fn solid_nodes_cover_their_whole_span() {
        let meta = Meta { grid_min: [0.0; 3], grid_max: [4.0; 3], resolution: 1.0, tree_depth: 0, node_count: 1, leaf_data_count: 0 };
        let tree = Octree { meta, words: vec![SOLID] };
        let mut count = 0;
        tree.for_each_occupied(|_| count += 1);
        assert_eq!(count, 64);
    }

    #[test]
    fn a_solid_block_to_the_grid_top_has_no_walkable_floor() {
        // The block fills the grid to its top, so nothing is air above it: no floor.
        let meta = Meta { grid_min: [0.0; 3], grid_max: [4.0; 3], resolution: 1.0, tree_depth: 0, node_count: 1, leaf_data_count: 0 };
        let tree = Octree { meta, words: vec![SOLID] };
        assert!(tree.floor_cells(2).is_empty());
    }

    #[test]
    fn a_lone_cell_is_a_floor_when_nothing_is_above_it() {
        let tree = tiny_octree();
        assert_eq!(tree.floor_cells(2), vec![[1, 2, 3]]);
        assert!(tree.floor_cells(2).len() == 1);
    }

    #[test]
    fn rejects_a_binary_of_the_wrong_length() {
        let meta = Meta { grid_min: [0.0; 3], grid_max: [4.0; 3], resolution: 1.0, tree_depth: 0, node_count: 1, leaf_data_count: 0 };
        assert!(Octree::from_bytes(meta, &[0u8; 8]).is_err());
    }
}
