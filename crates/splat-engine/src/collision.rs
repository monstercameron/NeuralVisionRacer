//! Level collision streamed around the car. The level's voxel octree gives a floor height per cell and the wall
//! columns (see `level`); those grids are small and stay in memory, while the physics colliders made from them only
//! exist in square tiles near the car. A whole street is millions of floor cells; the car only ever touches the few
//! around it.
//!
//! The floor grid is cleaned once (small holes filled, a median taken; see `floor`) and stored as 16-bit cell
//! heights. Each tile smooths its own cells with an edge-preserving filter (`floor::bilateral`: cobbles and voxel
//! steps average out, kerbs keep their edge) and becomes one heightfield, a continuous surface with a point every
//! `level.floor_mesh_m`, so the wheels roll over the road instead of catching box edges. Neighbouring tiles share
//! their edge points. Each tile hangs its heightfield and wall boxes on one fixed body, so dropping a tile is one
//! removal. Tiles are built when they come within `level.collision_radius_m` of the car and dropped beyond 1.5 times
//! that, so a car moving along a tile edge does not make them flicker.

use crate::config::EngineConfig;
use crate::level::LevelFloor;
use rapier3d::parry::shape::{HeightField, HeightFieldCellStatus, HeightFieldFlags};
use rapier3d::parry::utils::Array2;
use rapier3d::prelude::*;
use std::collections::BTreeMap;

/// Default side of a collision tile, metres (`level.collision_tile_m`).
pub const TILE_M: f32 = 16.0;
/// Floor cells per collision cell along each axis. The voxel floor is finer than wheels need (5 cm in Delft);
/// a collision cell takes the highest of its voxel cells, so kerbs keep their height, and far fewer boxes result.
const FLOOR_DOWNSAMPLE: usize = 2;
/// Tiles whose area comes within `level.collision_radius_m` of the car are built, and dropped once they are farther
/// than this many times that.
const KEEP_FACTOR: f32 = 1.5;

pub struct LevelCollision {
    /// Floor top per cell, in cells above the grid origin plus one; 0 is a hole. Indexed `x * nz + z`.
    heights: Vec<u16>,
    nx: usize,
    nz: usize,
    /// Collision cell size, and the voxel size the stored tops count in.
    cell_m: f32,
    voxel_m: f32,
    origin: [f32; 3],
    walls: Vec<Option<u32>>,
    wall_dims: [usize; 2],
    block_m: f32,
    wall_height_m: f32,
    friction: f32,
    restitution: f32,
    tile_m: f32,
    build_m: f32,
    keep_m: f32,
    /// Road smoothing radius in collision cells, the height change it keeps as a step, and the heightfield's point
    /// spacing in collision cells.
    smooth_cells: usize,
    keep_step_m: f32,
    mesh_stride: usize,
    tiles: BTreeMap<(i32, i32), RigidBodyHandle>,
    /// Bumped whenever tiles are built or dropped, so a debug view knows when to redraw.
    generation: u64,
}

impl LevelCollision {
    pub fn new(level: &LevelFloor, config: &EngineConfig) -> Self {
        let k = FLOOR_DOWNSAMPLE;
        let (nx, nz) = ((level.dims[0] as usize).div_ceil(k), (level.dims[2] as usize).div_ceil(k));
        let cell_m = level.resolution * k as f32;
        // Tops in voxel cells, the highest per collision cell.
        let mut tops: Vec<Option<f32>> = vec![None; nx * nz];
        for c in &level.cells {
            let slot = &mut tops[(c[0] as usize / k) * nz + c[2] as usize / k];
            let top = c[1] as f32 + 1.0;
            *slot = Some(slot.map_or(top, |h: f32| h.max(top)));
        }
        let tops = crate::floor::smooth(&crate::floor::fill_holes(&tops, nx, nz), nx, nz);
        let heights = tops.iter().map(|t| t.map_or(0, |t| (t as u16).saturating_add(1))).collect();
        Self {
            heights,
            nx,
            nz,
            cell_m,
            voxel_m: level.resolution,
            origin: level.origin,
            walls: if config.level_walls { level.walls.clone() } else { Vec::new() },
            wall_dims: level.wall_dims,
            block_m: level.block_m,
            wall_height_m: config.level_wall_height_m,
            friction: config.physics_friction,
            restitution: config.physics_restitution,
            tile_m: config.level_collision_tile_m.max(1.0),
            build_m: config.level_collision_radius_m.max(1.0),
            keep_m: config.level_collision_radius_m.max(1.0) * KEEP_FACTOR,
            smooth_cells: (config.level_floor_smooth_m.max(0.0) / cell_m).round() as usize,
            keep_step_m: config.level_floor_step_keep_m,
            mesh_stride: (config.level_floor_mesh_m / cell_m).round().max(1.0) as usize,
            tiles: BTreeMap::new(),
            generation: 0,
        }
    }

    /// Builds the tiles near `centre` (world space) and drops far ones. Returns (built, dropped).
    pub fn update(&mut self, world: &mut PhysicsWorld, centre: [f32; 3]) -> (usize, usize) {
        let (cx, cz) = (centre[0] - self.origin[0], centre[2] - self.origin[2]);
        let (tile, build, keep) = (self.tile_m, self.build_m, self.keep_m);
        let reach = (build / tile).ceil() as i32 + 1;
        let (tx0, tz0) = ((cx / tile).floor() as i32, (cz / tile).floor() as i32);
        let (tiles_x, tiles_z) = (self.tiles_along(self.nx), self.tiles_along(self.nz));
        let mut built = 0;
        for tx in (tx0 - reach).max(0)..=(tx0 + reach).min(tiles_x - 1) {
            for tz in (tz0 - reach).max(0)..=(tz0 + reach).min(tiles_z - 1) {
                if self.tiles.contains_key(&(tx, tz)) || tile_distance(cx, cz, tx, tz, tile) > build {
                    continue;
                }
                if let Some(body) = self.build_tile(world, tx, tz) {
                    self.tiles.insert((tx, tz), body);
                    built += 1;
                }
            }
        }
        let far: Vec<(i32, i32)> = self.tiles.keys().filter(|&&(tx, tz)| tile_distance(cx, cz, tx, tz, tile) > keep).copied().collect();
        for key in &far {
            if let Some(body) = self.tiles.remove(key) {
                world.remove_body_with_colliders(body, false);
            }
        }
        if built + far.len() > 0 {
            self.generation += 1;
        }
        (built, far.len())
    }

    /// Changes whenever the set of built tiles does.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Line segments outlining the built colliders, world space, for the debug view: each floor heightfield as a
    /// grid through every `stride`th point (squares left out over holes are skipped), and each wall box's edges.
    /// The flag is true for walls.
    pub fn debug_segments(&self, world: &PhysicsWorld, stride: usize) -> Vec<([f32; 3], [f32; 3], bool)> {
        let stride = stride.max(1);
        let mut out = Vec::new();
        for body in self.tiles.values() {
            let Some(body) = world.bodies.get(*body) else { continue };
            for handle in body.colliders() {
                let Some(collider) = world.colliders.get(*handle) else { continue };
                let pose = *collider.position();
                let to_world = |p: Vector| {
                    let w = pose * p;
                    [w.x, w.y, w.z]
                };
                if let Some(field) = collider.shape().as_heightfield() {
                    let heights = field.heights();
                    let (rows, cols) = (heights.nrows(), heights.ncols());
                    let scale = field.scale();
                    let point = |i: usize, j: usize| {
                        let x = -scale.x / 2.0 + scale.x * j as f32 / (cols - 1) as f32;
                        let z = -scale.z / 2.0 + scale.z * i as f32 / (rows - 1) as f32;
                        to_world(Vector::new(x, heights[(i, j)] * scale.y, z))
                    };
                    let removed = |i: usize, j: usize| {
                        let (ci, cj) = (i.min(rows - 2), j.min(cols - 2));
                        field.cells_statuses()[(ci, cj)].contains(rapier3d::parry::shape::HeightFieldCellStatus::CELL_REMOVED)
                    };
                    for i in (0..rows).step_by(stride) {
                        for j in 0..cols - 1 {
                            if !removed(i, j) {
                                out.push((point(i, j), point(i, j + 1), false));
                            }
                        }
                    }
                    for j in (0..cols).step_by(stride) {
                        for i in 0..rows - 1 {
                            if !removed(i, j) {
                                out.push((point(i, j), point(i + 1, j), false));
                            }
                        }
                    }
                } else if let Some(cuboid) = collider.shape().as_cuboid() {
                    let h = cuboid.half_extents;
                    let corner = |k: usize| to_world(Vector::new(if k & 1 == 1 { h.x } else { -h.x }, if k & 2 == 2 { h.y } else { -h.y }, if k & 4 == 4 { h.z } else { -h.z }));
                    for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
                        out.push((corner(a), corner(b), true));
                    }
                }
            }
        }
        out
    }

    /// Tiles built now.
    pub fn tiles(&self) -> usize {
        self.tiles.len()
    }

    /// Whether any collision exists yet (the car's tile is built once `update` has run near it).
    pub fn has_ground(&self) -> bool {
        !self.tiles.is_empty()
    }

    fn tiles_along(&self, cells: usize) -> i32 {
        ((cells as f32 * self.cell_m) / self.tile_m).ceil() as i32
    }

    /// The floor of the `per × per` cells from (x0, z0): a heightfield through the smoothed heights of every
    /// `mesh_stride`th cell centre, up to and including the next tile's first, so neighbours meet without a seam.
    /// Squares touching a hole are left out. `None` when the tile has no floor.
    fn floor_tile(&self, x0: usize, z0: usize, per: usize) -> Option<ColliderBuilder> {
        // The largest point spacing up to `mesh_stride` that divides the tile, so every tile's points line up.
        let stride = (1..=self.mesh_stride.min(per)).rev().find(|s| per % s == 0)?;
        let (x_end, z_end) = ((x0 + per).min(self.nx.checked_sub(1)?), (z0 + per).min(self.nz.checked_sub(1)?));
        if x_end < x0 + stride || z_end < z0 + stride {
            return None;
        }
        let (cols, rows) = ((x_end - x0) / stride + 1, (z_end - z0) / stride + 1);

        // The smoothing reads `smooth_cells` beyond the tile, so both sides of a tile edge agree.
        let r = self.smooth_cells;
        let (ax, az) = (x0.saturating_sub(r), z0.saturating_sub(r));
        let (bx, bz) = ((x_end + r).min(self.nx - 1), (z_end + r).min(self.nz - 1));
        let (w, d) = (bx - ax + 1, bz - az + 1);
        let mut raw = vec![None; w * d];
        for x in 0..w {
            for z in 0..d {
                let h = self.heights[(ax + x) * self.nz + az + z];
                raw[x * d + z] = (h > 0).then(|| f32::from(h - 1) * self.voxel_m);
            }
        }
        let smooth = crate::floor::bilateral(&raw, w, d, r, self.keep_step_m);

        // Points: rows along z, columns along x, column-major.
        let points: Vec<Option<f32>> = (0..cols)
            .flat_map(|j| (0..rows).map(move |i| (x0 + j * stride, z0 + i * stride)))
            .map(|(x, z)| smooth[(x - ax) * d + z - az])
            .collect();
        let present: Vec<f32> = points.iter().flatten().copied().collect();
        if present.is_empty() {
            return None;
        }
        let fill = present.iter().sum::<f32>() / present.len() as f32;
        let heights = Array2::new(rows, cols, points.iter().map(|p| p.unwrap_or(fill)).collect());
        let size = Vector::new(((cols - 1) * stride) as f32 * self.cell_m, 1.0, ((rows - 1) * stride) as f32 * self.cell_m);
        let mut field = HeightField::with_flags(heights, size, HeightFieldFlags::FIX_INTERNAL_EDGES);
        let mut any = false;
        for j in 0..cols - 1 {
            for i in 0..rows - 1 {
                let corners = [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)];
                if corners.iter().any(|&(ci, cj)| points[cj * rows + ci].is_none()) {
                    field.set_cell_status(i, j, HeightFieldCellStatus::CELL_REMOVED);
                } else {
                    any = true;
                }
            }
        }
        if !any {
            return None;
        }
        // The heightfield is centred on its position; its first point is the centre of cell (x0, z0).
        let centre = Vector::new((x0 as f32 + 0.5) * self.cell_m + size.x / 2.0, 0.0, (z0 as f32 + 0.5) * self.cell_m + size.z / 2.0);
        Some(ColliderBuilder::new(SharedShape::new(field)).translation(centre))
    }

    /// One tile's floor and wall boxes on a fixed body at the grid origin, or `None` when the tile is empty.
    fn build_tile(&self, world: &mut PhysicsWorld, tx: i32, tz: i32) -> Option<RigidBodyHandle> {
        let mut colliders = Vec::new();

        // Floor: this tile's smoothed heights as one heightfield.
        let per = (self.tile_m / self.cell_m).round() as usize;
        if let Some(floor) = self.floor_tile(tx as usize * per, tz as usize * per, per) {
            colliders.push(floor);
        }

        // Walls: this tile's wall columns, merged the same way, from a slab below the road to `wall_height_m` above.
        if !self.walls.is_empty() {
            let per = (self.tile_m / self.block_m).round() as usize;
            let [nbx, nbz] = self.wall_dims;
            let (x0, z0) = (tx as usize * per, tz as usize * per);
            let (w, d) = (per.min(nbx.saturating_sub(x0)), per.min(nbz.saturating_sub(z0)));
            let mut sub = vec![None; w * d];
            for x in 0..w {
                for z in 0..d {
                    sub[x * d + z] = self.walls[(x0 + x) * nbz + z0 + z].map(|r| r as f32);
                }
            }
            for b in crate::floor::merge(&sub, w, d) {
                let (bw, bd) = (b.w as f32 * self.block_m, b.d as f32 * self.block_m);
                let road_top = (b.top + 1.0) * self.block_m;
                let (bottom, top) = (road_top - crate::floor::SLAB_M, road_top + self.wall_height_m);
                let centre = Vector::new((x0 + b.x) as f32 * self.block_m + bw / 2.0, (bottom + top) / 2.0, (z0 + b.z) as f32 * self.block_m + bd / 2.0);
                colliders.push(ColliderBuilder::cuboid(bw / 2.0, (top - bottom) / 2.0, bd / 2.0).translation(centre));
            }
        }

        let mut colliders = colliders.into_iter().map(|c| c.friction(self.friction).restitution(self.restitution));
        let first = colliders.next()?;
        let body = RigidBodyBuilder::fixed().translation(Vector::new(self.origin[0], self.origin[1], self.origin[2]));
        let (handle, _) = world.insert(body, first);
        for collider in colliders {
            world.insert_collider(collider, Some(handle));
        }
        Some(handle)
    }
}

/// Distance in the ground plane from a point (relative to the grid origin) to tile (tx, tz)'s square.
fn tile_distance(x: f32, z: f32, tx: i32, tz: i32, tile: f32) -> f32 {
    let (x0, z0) = (tx as f32 * tile, tz as f32 * tile);
    let dx = (x0 - x).max(0.0).max(x - (x0 + tile));
    let dz = (z0 - z).max(0.0).max(z - (z0 + tile));
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat 64 × 64 m floor at y = 0 (cells of 0.25 m), no walls.
    fn level() -> LevelFloor {
        let n = 256u32;
        let cells = (0..n).flat_map(|x| (0..n).map(move |z| [x, 3, z])).collect();
        LevelFloor { cells, origin: [0.0, -1.0, 0.0], resolution: 0.25, dims: [n, 8, n], walls: Vec::new(), wall_dims: [0, 0], block_m: 1.0, spawn_hint: None, spawn_forward: None }
    }

    #[test]
    fn only_tiles_near_the_car_exist_and_they_follow_it() {
        let config = EngineConfig::default();
        let mut collision = LevelCollision::new(&level(), &config);
        let mut world = PhysicsWorld::new();
        collision.update(&mut world, [2.0, 0.0, 2.0]);
        let near_corner = collision.tiles();
        assert!(near_corner > 0 && near_corner < 16, "a corner of the 4 × 4-tile level builds part of it: {near_corner}");
        // The floor top is cell 3's top face: y = -1 + 4 × 0.25 = 0.
        world.step();
        let hit = world.query_pipeline().cast_ray(&Ray::new(Vector::new(2.0, 5.0, 2.0), Vector::new(0.0, -1.0, 0.0)), 10.0, true);
        assert!(hit.is_some_and(|(_, toi)| (toi - 5.0).abs() < 0.01), "floor at y = 0: {hit:?}");
        // Driving to the far corner builds tiles there and drops the start's.
        let (built, dropped) = collision.update(&mut world, [62.0, 0.0, 62.0]);
        assert!(built > 0 && dropped > 0, "built {built}, dropped {dropped}");
        assert!(collision.tiles.keys().all(|&(tx, tz)| tile_distance(62.0, 62.0, tx, tz, collision.tile_m) <= collision.keep_m));
    }

    #[test]
    fn the_floor_follows_a_ramp_along_x_and_is_smooth_across_tiles() {
        // 32 × 32 m of 5 cm voxels rising one voxel every 8 cells (40 cm) along x, flat along z: a 12.5 % ramp built
        // of 5 cm stairs, as the voxel floor stores a smooth slope.
        let n = 640u32;
        let cells = (0..n).flat_map(|x| (0..n).map(move |z| [x, x / 8, z])).collect();
        let floor = LevelFloor { cells, origin: [0.0, -1.0, 0.0], resolution: 0.05, dims: [n, 128, n], walls: Vec::new(), wall_dims: [0, 0], block_m: 1.0, spawn_hint: None, spawn_forward: None };
        let config = EngineConfig::default();
        let mut collision = LevelCollision::new(&floor, &config);
        let mut world = PhysicsWorld::new();
        collision.update(&mut world, [16.0, 0.0, 16.0]);
        world.step();
        let height = |x: f32, z: f32| {
            let hit = world.query_pipeline().cast_ray(&Ray::new(Vector::new(x, 20.0, z), Vector::new(0.0, -1.0, 0.0)), 40.0, true);
            20.0 - hit.expect("floor under the ray").1
        };
        // Voxel i's top is -1 + 0.05 (⌊i / 8⌋ + 1), so the stair tops average to the line y = -1 + x / 8 + 0.03;
        // smoothing straightens them to within a couple of centimetres, across the tile edge at 16 m too.
        let ramp = |x: f32| -1.0 + x / 8.0 + 0.03;
        let mut previous = height(4.0, 20.0);
        for i in 1..=100 {
            let x = 4.0 + i as f32 * 0.2;
            let h = height(x, 20.0);
            assert!((h - previous - 0.025).abs() < 0.01, "even rise of 2.5 cm per 20 cm at x = {x}: {previous} → {h}");
            previous = h;
        }
        for &x in &[6.0, 10.0, 15.9, 16.1, 22.0] {
            let expected = ramp(x);
            assert!((height(x, 30.0) - expected).abs() < 0.03, "at x = {x}: {} vs {expected}", height(x, 30.0));
        }
        assert!((height(10.0, 6.0) - height(10.0, 26.0)).abs() < 0.01, "flat along z");
    }
}
