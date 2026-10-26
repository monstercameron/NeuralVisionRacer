//! A streamed level's collision, from its voxels (`scene.voxel.json` + `scene.voxel.bin`): the walkable floor, and
//! the walls (anything solid at car height above the road: facades, posts, planters).
//!
//! The road becomes streamed heightfields; obstacles use fitted voxel boxes in per-tile compounds. The car spawns on the road: the floor cell nearest the level
//! centre among the lowest surfaces.

use crate::svo::{grid_cells, Octree};
use std::path::Path;

/// The spawn needs no wall within this footprint around it: half the car's width and length plus a margin, with the
/// car's length along z (the car spawns facing along z).
const SPAWN_CLEAR_HALF_WIDTH_M: f32 = 1.2;
const SPAWN_CLEAR_HALF_LENGTH_M: f32 = 2.5;

/// Walls are solid blocks from this high above the road (kerbs and steps below it stay drivable floor)...
const WALL_FROM_M: f32 = 0.4;
/// ...up to this high, about a car's height and some.
const WALL_TO_M: f32 = 2.4;
/// How far from a column to look for the road it stands beside.
const WALL_REACH_M: f32 = 0.6;

/// The road spawn needs this much floor around it, in metres, and no more than `SPAWN_FLAT_M` of height change.
const SPAWN_RADIUS_M: f32 = 1.0;
const SPAWN_FLAT_M: f32 = 0.15;

#[derive(Debug, Clone, PartialEq)]
pub struct LevelFloor {
    /// Floor cells as grid indices, the solid cell whose top face is walkable.
    pub cells: Vec<[u32; 3]>,
    /// World position of grid cell (0, 0, 0), the grid minimum.
    pub origin: [f32; 3],
    /// Metres per cell.
    pub resolution: f32,
    /// Grid size in cells.
    pub dims: [u32; 3],
    /// Wall columns over the block grid (`block_m` square), indexed `bx * nbz + bz`: the road block they stand on.
    pub walls: Vec<Option<u32>>,
    /// Source-fitted obstacle geometry. None retains coarse columns for synthetic/legacy callers.
    pub wall_boxes: Option<Vec<crate::svo::WallBox>>,
    /// Block grid size `[nbx, nbz]`.
    pub wall_dims: [usize; 2],
    /// Metres per block (4 cells).
    pub block_m: f32,
    /// Where to look for the spawn first, world space: the scan's own starting camera (`viewer-settings.json`),
    /// which is reliably somewhere open. `None` searches from the grid's centre.
    pub spawn_hint: Option<[f32; 3]>,
    /// The direction (x, z) the scan's starting camera looks: the car spawns facing it, towards open ground.
    pub spawn_forward: Option<[f32; 2]>,
}

impl LevelFloor {
    /// Loads the collision voxels in `dir` and extracts the floor. `clearance_m` is the air needed above a floor.
    pub fn load(dir: &Path, clearance_m: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let mut level = Self::from_octree(Octree::load(dir)?, clearance_m)?;
        if let Ok(settings) = std::fs::read(dir.join("viewer-settings.json")) {
            level.set_spawn_hint_from_viewer_settings(&settings);
        }
        Ok(level)
    }

    /// Takes the spawn hint from a scan's `viewer-settings.json`: its first camera's starting position.
    pub fn set_spawn_hint_from_viewer_settings(&mut self, json: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else { return };
        let vec3 = |v: &serde_json::Value| v.as_array().filter(|a| a.len() == 3).map(|a| [0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0) as f32));
        let initial = &value["cameras"][0]["initial"];
        if let Some(p) = vec3(&initial["position"]) {
            self.spawn_hint = Some(p);
            if let Some(t) = vec3(&initial["target"]) {
                let (dx, dz) = (t[0] - p[0], t[2] - p[2]);
                let length = (dx * dx + dz * dz).sqrt();
                self.spawn_forward = (length > 1e-3).then(|| [dx / length, dz / length]);
            }
        }
    }

    /// Extracts the floor from the level's two collision files, for the browser build.
    pub fn from_json_and_bin(json: &[u8], bin: &[u8], clearance_m: f32) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_octree(Octree::from_json_and_bin(json, bin)?, clearance_m)
    }

    fn from_octree(tree: Octree, clearance_m: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let resolution = tree.meta.resolution;
        let clearance = (clearance_m / resolution).ceil().max(1.0) as u32;
        let block_m = resolution * 4.0;
        let blocks = |m: f32| (m / block_m).round().max(1.0) as usize;
        let (walls, wall_dims) = tree.wall_columns(clearance, (blocks(WALL_FROM_M), blocks(WALL_TO_M)), blocks(WALL_REACH_M));
        let wall_boxes = tree.wall_boxes(&walls, wall_dims, (blocks(WALL_FROM_M), blocks(WALL_TO_M)));
        // Keep the conservative spawn exclusion grid, while driving uses the fitted boxes.
        Ok(Self {
            wall_boxes: Some(wall_boxes),
            spawn_hint: None,
            spawn_forward: None,
            cells: tree.floor_cells(clearance),
            origin: tree.meta.grid_min,
            resolution,
            dims: grid_cells(&tree.meta),
            walls,
            wall_dims,
            block_m,
        })
    }

    /// World position of the walkable top face of a floor cell.
    pub fn top_of(&self, cell: [u32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| self.origin[i] + (cell[i] as f32 + if i == 1 { 1.0 } else { 0.5 }) * self.resolution)
    }

    /// Picks the road spawn: the walkable spot nearest the grid centre that is flat, fully covered and clear of walls.
    /// A spot counts when every column within `SPAWN_RADIUS_M` has a floor, the floor's height varies by at most
    /// `SPAWN_FLAT_M`, and no wall column lies within a car-sized footprint around it.
    /// Returns the world position of its top face, or `None` when no spot qualifies.
    pub fn road_spawn(&self) -> Option<[f32; 3]> {
        if self.cells.is_empty() {
            return None;
        }
        let (nx, nz) = (self.dims[0] as usize, self.dims[2] as usize);
        // Dense top-cell heights per column, `None` for a hole. Built once per query; the query runs at start-up.
        let mut top = vec![None::<u32>; nx * nz];
        for c in &self.cells {
            let slot = &mut top[c[0] as usize * nz + c[2] as usize];
            *slot = Some(slot.map_or(c[1] + 1, |t: u32| t.max(c[1] + 1)));
        }
        let radius = (SPAWN_RADIUS_M / self.resolution).ceil() as i64;
        let flat = (SPAWN_FLAT_M / self.resolution).ceil() as u32;
        // Search outwards from the hint (in cells), or the grid's centre.
        let centre = match self.spawn_hint {
            Some(hint) => [0, 1, 2].map(|i| (hint[i] - self.origin[i]) / self.resolution),
            None => [0, 1, 2].map(|i| self.dims[i] as f32 / 2.0),
        };
        let mut candidates: Vec<(f32, [u32; 3])> = self
            .cells
            .iter()
            .map(|c| {
                let d = (c[0] as f32 - centre[0]).powi(2) + (c[2] as f32 - centre[2]).powi(2);
                (d, *c)
            })
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, c) in candidates.iter().take(400_000) {
            let (cx, cz) = (c[0] as i64, c[2] as i64);
            let mut lo = u32::MAX;
            let mut hi = 0u32;
            let mut covered = true;
            'window: for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let (x, z) = (cx + dx, cz + dz);
                    if x < 0 || z < 0 || x >= nx as i64 || z >= nz as i64 {
                        covered = false;
                        break 'window;
                    }
                    match top[x as usize * nz + z as usize] {
                        Some(t) => {
                            lo = lo.min(t);
                            hi = hi.max(t);
                        }
                        None => {
                            covered = false;
                            break 'window;
                        }
                    }
                }
            }
            if covered && hi - lo <= flat && self.clear_of_walls(c[0], c[2]) {
                return Some(self.top_of([c[0], c[1], c[2]]));
            }
        }
        None
    }

    /// No wall column within the spawn footprint around floor cell (x, z).
    fn clear_of_walls(&self, x: u32, z: u32) -> bool {
        if self.walls.is_empty() {
            return true;
        }
        let per_block = (self.block_m / self.resolution).round().max(1.0) as i64;
        let (bx, bz) = (i64::from(x) / per_block, i64::from(z) / per_block);
        let (rx, rz) = ((SPAWN_CLEAR_HALF_WIDTH_M / self.block_m).ceil() as i64, (SPAWN_CLEAR_HALF_LENGTH_M / self.block_m).ceil() as i64);
        let [nbx, nbz] = self.wall_dims.map(|n| n as i64);
        for i in (bx - rx).max(0)..=(bx + rx).min(nbx - 1) {
            for j in (bz - rz).max(0)..=(bz + rz).min(nbz - 1) {
                if self.walls[(i * nbz + j) as usize].is_some() {
                    return false;
                }
            }
        }
        true
    }

}
