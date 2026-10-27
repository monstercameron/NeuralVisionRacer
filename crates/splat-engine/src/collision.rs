//! Level collision streamed around the car. The level's voxel octree gives a floor height per cell and the wall
//! candidates and source-fitted obstacle boxes (see `level`); those stay in memory, while their physics colliders only
//! exist in square tiles near the car. A whole street is millions of floor cells; the car only ever touches the few
//! around it.
//!
//! The floor grid is cleaned once (small holes filled, a median taken; see `floor`) and stored as 16-bit cell
//! heights. Each tile smooths its own cells with an edge-preserving filter (`floor::bilateral`: cobbles and voxel
//! steps average out, kerbs keep their edge) and becomes one heightfield, a continuous surface with a point every
//! `level.floor_mesh_m`, so the wheels roll over the road instead of catching box edges. Neighbouring tiles share
//! their edge points. Steep road-to-roof bridges are discarded. Each tile hangs its heightfield and one obstacle
//! compound on one fixed body; rounded box edges remain inside occupied voxel bounds. Dropping a tile is one
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
    road: Option<crate::road::RoadProfile>,
    road_cells: usize,
    road_obstacles_removed: usize,
    road_floor_apron_m: f32,
    /// Floor top per cell, in cells above the grid origin plus one; 0 is a hole. Indexed `x * nz + z`.
    heights: Vec<u16>,
    nx: usize,
    nz: usize,
    /// Collision cell size, and the voxel size the stored tops count in.
    cell_m: f32,
    voxel_m: f32,
    origin: [f32; 3],
    walls: Vec<Option<u32>>,
    /// Refined geometry indexed once by tile, rather than rescanning the level each tick.
    wall_boxes: Option<BTreeMap<(i32, i32), Vec<crate::svo::WallBox>>>,
    soft_boxes: BTreeMap<(i32,i32),Vec<crate::svo::WallBox>>,
    soft_floor_samples_removed: usize,
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
    floor_max_slope: f32,
    tiles: BTreeMap<(i32, i32), RigidBodyHandle>,
    /// Empty tiles are immutable too: do not re-smooth and re-scan them every physics tick.
    empty_tiles: std::collections::BTreeSet<(i32, i32)>,
    /// Bumped whenever tiles are built or dropped, so a debug view knows when to redraw.
    generation: u64,
    /// Conservative movement radius within which no tile can cross a build/drop threshold.
    search_guard: Option<([f32; 2], f32)>,
    update_calls: u64,
    searches: u64,
    reused_searches: u64,
}

impl LevelCollision {
    pub fn new(level: &LevelFloor, config: &EngineConfig) -> Self {
        let materials=config.level_foliage.then(||crate::foliage::for_level(&config.level_path)).flatten();
        Self::new_with_materials(level,config,materials)
    }

    fn new_with_materials(level:&LevelFloor,config:&EngineConfig,materials:Option<&crate::foliage::MaterialMap>)->Self {
        let k = FLOOR_DOWNSAMPLE;
        let (nx, nz) = ((level.dims[0] as usize).div_ceil(k), (level.dims[2] as usize).div_ceil(k));
        let cell_m = level.resolution * k as f32;
        let road = crate::road::RoadProfile::for_level(&config.level_path);
        let road_floor_apron_m = config.level_floor_mesh_m.max(cell_m);
        // Tops in voxel cells, the highest per collision cell.
        let mut soft_floor_samples_removed=0;
        let mut tops: Vec<Option<f32>> = vec![None; nx * nz];
        for c in &level.cells {
            let p=[0,1,2].map(|i|level.origin[i]+(c[i] as f32+0.5)*level.resolution);
            if materials.is_some_and(|m|m.contains(p)) {soft_floor_samples_removed+=1;continue;}
            let slot = &mut tops[(c[0] as usize / k) * nz + c[2] as usize / k];
            let top = c[1] as f32 + 1.0;
            *slot = Some(slot.map_or(top, |h: f32| h.max(top)));
        }
        let mut road_cells = 0;
        if let Some(road) = &road {
            for x in 0..nx { for z in 0..nz {
                if let Some(y) = road.height_with_apron(level.origin[0]+(x as f32+0.5)*cell_m,
                    level.origin[2]+(z as f32+0.5)*cell_m,road_floor_apron_m) {
                    tops[x*nz+z] = Some((y-level.origin[1])/level.resolution);
                    road_cells += 1;
                }
            }}
        }
        let tops = crate::floor::smooth(&crate::floor::fill_holes(&tops, nx, nz), nx, nz);
        let heights = tops.iter().map(|t| t.map_or(0, |t| (t as u16).saturating_add(1))).collect();
        let tile_m = config.level_collision_tile_m.max(1.0);
        let mut road_obstacles_removed = 0;
        let mut soft_boxes = BTreeMap::<_,Vec<_>>::new();
        let wall_boxes = level.wall_boxes.as_ref().map(|boxes| {
            let mut tiles = BTreeMap::<_, Vec<_>>::new();
            if config.level_walls {
                for original in boxes {
                  let pieces=materials.map_or_else(||vec![(*original,false)],|m|crate::foliage::split_box(*original,level.origin,m));
                  for (b,soft) in pieces {
                    if !soft && road.as_ref().is_some_and(|road| road.blocks_road(&b,level.origin)) {
                        road_obstacles_removed += 1;
                        continue;
                    }
                    // Split boxes at tile boundaries, including non-default tile sizes.
                    let tx0 = (b.min[0] / tile_m).floor() as i32;
                    let tz0 = (b.min[2] / tile_m).floor() as i32;
                    let tx1 = ((b.max[0] - 1e-5) / tile_m).floor() as i32;
                    let tz1 = ((b.max[2] - 1e-5) / tile_m).floor() as i32;
                    for tx in tx0..=tx1 { for tz in tz0..=tz1 {
                        let mut clipped = b;
                        clipped.min[0] = b.min[0].max(tx as f32 * tile_m);
                        clipped.min[2] = b.min[2].max(tz as f32 * tile_m);
                        clipped.max[0] = b.max[0].min((tx + 1) as f32 * tile_m);
                        clipped.max[2] = b.max[2].min((tz + 1) as f32 * tile_m);
                        if soft {soft_boxes.entry((tx,tz)).or_default().push(clipped);}
                        else {tiles.entry((tx,tz)).or_default().push(clipped);}
                    }}
                  }
                }
            }
            tiles
        });
        Self {
            road, road_cells, road_obstacles_removed, road_floor_apron_m,
            wall_boxes, soft_boxes, soft_floor_samples_removed,
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
            floor_max_slope: config.level_floor_max_slope,
            mesh_stride: (config.level_floor_mesh_m / cell_m).round().max(1.0) as usize,
            tiles: BTreeMap::new(),
            empty_tiles: Default::default(),
            generation: 0,
            search_guard: None,
            update_calls: 0,
            searches: 0,
            reused_searches: 0,
        }
    }

    /// Builds the tiles near `centre` (world space) and drops far ones. Returns (built, dropped).
    pub fn update(&mut self, world: &mut PhysicsWorld, centre: [f32; 3]) -> (usize, usize) {
        let (cx, cz) = (centre[0] - self.origin[0], centre[2] - self.origin[2]);
        self.update_calls += 1;
        if let Some(([x,z], radius_sq)) = self.search_guard {
            let (dx,dz) = (cx-x,cz-z);
            if dx*dx + dz*dz < radius_sq {
                self.reused_searches += 1;
                return (0,0);
            }
        }
        self.searches += 1;
        let (tile, build, keep) = (self.tile_m, self.build_m, self.keep_m);
        let reach = (build / tile).ceil() as i32 + 1;
        let (tx0, tz0) = ((cx / tile).floor() as i32, (cz / tile).floor() as i32);
        let (tiles_x, tiles_z) = (self.tiles_along(self.nx), self.tiles_along(self.nz));
        // Tiles beyond this search square are at least one tile farther than the
        // build radius. A half-tile cap also covers entry from outside the level.
        let mut safe_m = tile * 0.5;
        let mut built = 0;
        for tx in (tx0 - reach).max(0)..=(tx0 + reach).min(tiles_x - 1) {
            for tz in (tz0 - reach).max(0)..=(tz0 + reach).min(tiles_z - 1) {
                if self.tiles.contains_key(&(tx, tz)) || self.empty_tiles.contains(&(tx,tz)) {
                    continue;
                }
                let distance = tile_distance(cx, cz, tx, tz, tile);
                if distance > build {
                    // Distance to a square is 1-Lipschitz: a shorter movement
                    // cannot bring this absent tile inside the build radius.
                    safe_m = safe_m.min(distance-build);
                    continue;
                }
                if let Some(body) = self.build_tile(world, tx, tz) {
                    self.tiles.insert((tx, tz), body);
                    built += 1;
                } else {
                    self.empty_tiles.insert((tx,tz));
                }
            }
        }
        let mut dropped = 0;
        self.tiles.retain(|&(tx,tz),body| {
            let distance = tile_distance(cx,cz,tx,tz,tile);
            if distance > keep {
                safe_m = safe_m.min(distance-build);
                world.remove_body_with_colliders(*body,false);
                dropped += 1;
                false
            } else {
                safe_m = safe_m.min(keep-distance);
                true
            }
        });
        // Leave a rounding margin and never reuse a non-finite calculation.
        let tolerance = 1e-4 + f32::EPSILON * cx.abs().max(cz.abs()).max(tile) * 8.0;
        let safe_m = (safe_m-tolerance).max(0.0);
        self.search_guard = (cx.is_finite() && cz.is_finite() && safe_m>0.0)
            .then_some(([cx,cz],safe_m*safe_m));
        if built + dropped > 0 { self.generation += 1; }
        (built, dropped)
    }

    /// Search work, separate from immutable empty tiles and actual collider changes.
    pub fn search_profile(&self) -> serde_json::Value {
        serde_json::json!({"updates":self.update_calls,"full_searches":self.searches,
            "reused_searches":self.reused_searches,"guard_radius_m":self.search_guard.map_or(0.0,|(_,r)|r.sqrt()),
            "active_tiles":self.tiles.len(),"known_empty_tiles":self.empty_tiles.len()})
    }

    /// Changes whenever the set of built tiles does.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Line segments outlining the built colliders, world space, for the debug view: each floor heightfield as a
    /// grid through every `stride`th point (squares left out over holes are skipped), and each wall box's edges.
    /// The flag is true for walls.
    pub fn debug_segments(&self,world:&PhysicsWorld,stride:usize)->Vec<([f32;3],[f32;3],bool)> {
        self.debug_material_segments(world,stride).into_iter().filter(|(_,_,kind)|*kind!=2).map(|(a,b,kind)|(a,b,kind==1)).collect()
    }

    /// Material: 0 floor, 1 hard obstacle, 2 non-blocking foliage.
    pub fn debug_material_segments(&self, world: &PhysicsWorld, stride: usize) -> Vec<([f32; 3], [f32; 3], u8)> {
        let stride = stride.max(1);
        let mut out = Vec::new();
        for body in self.tiles.values() {
            let Some(body) = world.bodies.get(*body) else { continue };
            for handle in body.colliders() {
                let Some(collider) = world.colliders.get(*handle) else { continue };
                let kind=if collider.is_sensor() {2} else {1};
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
                                out.push((point(i, j), point(i, j + 1), 0));
                            }
                        }
                    }
                    for j in (0..cols).step_by(stride) {
                        for i in 0..rows - 1 {
                            if !removed(i, j) {
                                out.push((point(i, j), point(i + 1, j), 0));
                            }
                        }
                    }
                } else if let Some(compound) = collider.shape().as_compound() {
                    for (local, shape) in compound.shapes() {
                        let Some(h) = shape.as_cuboid().map(|c| c.half_extents)
                            .or_else(|| shape.as_round_cuboid().map(|c| c.inner_shape.half_extents + Vector::splat(c.border_radius)))
                            else { continue };
                        let transform = pose * *local;
                        let corner = |k: usize| {
                            let p = transform * Vector::new(if k & 1 == 1 { h.x } else { -h.x },
                                if k & 2 == 2 { h.y } else { -h.y },if k & 4 == 4 { h.z } else { -h.z });
                            p.to_array()
                        };
                        for (a,b) in [(0,1),(1,3),(3,2),(2,0),(4,5),(5,7),(7,6),(6,4),(0,4),(1,5),(2,6),(3,7)] {
                            out.push((corner(a),corner(b),kind));
                        }
                    }
                } else if let Some(h) = collider.shape().as_cuboid().map(|c| c.half_extents)
                    .or_else(|| collider.shape().as_round_cuboid().map(|c| c.inner_shape.half_extents + Vector::splat(c.border_radius))) {
                    let corner = |k: usize| to_world(Vector::new(if k & 1 == 1 { h.x } else { -h.x }, if k & 2 == 2 { h.y } else { -h.y }, if k & 4 == 4 { h.z } else { -h.z }));
                    for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
                        out.push((corner(a), corner(b), kind));
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

    pub fn road_profile(&self) -> Option<serde_json::Value> {
        self.road.as_ref().map(|r|serde_json::json!({"width_m":r.half_width*2.0,
            "end_width_m":r.half_width_at(r.knots.last().unwrap()[2])*2.0,
            "floor_apron_m":self.road_floor_apron_m,
            "z_range":[r.knots[0][2],r.knots.last().unwrap()[2]],"clearance_m":r.clearance,
            "road_grid_cells":self.road_cells,"obstacle_boxes_removed":self.road_obstacles_removed}))
    }

    pub fn foliage_profile(&self)->serde_json::Value {
        serde_json::json!({"soft_pieces":self.soft_boxes.values().map(Vec::len).sum::<usize>(),
            "floor_samples_removed":self.soft_floor_samples_removed,"material_grid_m":crate::foliage::CELL_M,
            "streamed_sensor_compounds":self.tiles.keys().filter(|k|self.soft_boxes.contains_key(k)).count()})
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
            .map(|(x, z)| self.road.as_ref().and_then(|r|r.height_with_apron(
                self.origin[0]+(x as f32+0.5)*self.cell_m,self.origin[2]+(z as f32+0.5)*self.cell_m,self.road_floor_apron_m))
                .map(|y|y-self.origin[1]).or(smooth[(x - ax) * d + z - az]))
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
                let values = corners.map(|(ci,cj)| points[cj * rows + ci]);
                let steep_bridge = if let [Some(a),Some(b),Some(c),Some(d)] = values {
                    let v = [a,b,c,d];
                    let rise = v.iter().copied().fold(f32::NEG_INFINITY,f32::max)
                        - v.iter().copied().fold(f32::INFINITY,f32::min);
                    let edge = [(0,1),(0,2),(1,3),(2,3)].iter()
                        .map(|&(a,b)| (v[a] - v[b]).abs()).fold(0.0,f32::max);
                    self.floor_max_slope > 0.0 && rise > 0.20
                        && edge > stride as f32 * self.cell_m * self.floor_max_slope
                } else { false };
                if values.iter().any(Option::is_none) || steep_bridge {
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

        // Imported obstacles follow actual occupied geometry. Synthetic/legacy callers retain coarse columns.
        if let Some(tiles) = &self.wall_boxes {
            if let Some(boxes) = tiles.get(&(tx,tz)) {
                let mut pieces = Vec::with_capacity(boxes.len());
                for b in boxes {
                    let min = Vector::new(b.min[0], b.min[1], b.min[2]);
                    let max = Vector::new(b.max[0], b.max[1], b.max[2]);
                    let half = (max - min) * 0.5;
                    if half.min_element() <= 0.0 { continue; }
                    // Rounded edges stay inside the occupied voxel bounds. This eases
                    // catches on voxel corners without widening the invisible footprint.
                    let seam = [b.min[0] - tx as f32 * self.tile_m,
                        b.max[0] - (tx + 1) as f32 * self.tile_m,
                        b.min[2] - tz as f32 * self.tile_m,
                        b.max[2] - (tz + 1) as f32 * self.tile_m].iter().any(|v| v.abs() < 1e-4);
                    // Tile cuts are internal seams, not object corners: keep them flat.
                    let radius = if seam { 0.0 } else { 0.02_f32.min(half.min_element() * 0.4) };
                    let shape = if radius == 0.0 { SharedShape::cuboid(half.x,half.y,half.z) }
                        else { SharedShape::round_cuboid(half.x-radius,half.y-radius,half.z-radius,radius) };
                    pieces.push((Pose::new((min + max) * 0.5, Vector::ZERO),shape));
                }
                if !pieces.is_empty() { colliders.push(ColliderBuilder::compound(pieces)); }
            }
        } else if !self.walls.is_empty() {
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

        if let Some(boxes)=self.soft_boxes.get(&(tx,tz)) {
            let pieces:Vec<_>=boxes.iter().map(|b| {
                let min=Vector::from_array(b.min);let max=Vector::from_array(b.max);let half=(max-min)*0.5;
                (Pose::new((min+max)*0.5,Vector::ZERO),SharedShape::cuboid(half.x,half.y,half.z))
            }).collect();
            if !pieces.is_empty() {
                colliders.push(ColliderBuilder::compound(pieces).sensor(true).user_data(crate::foliage::SENSOR_TAG));
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

    #[test]
    fn search_guard_matches_full_search_through_motion_boundaries_and_teleports() {
        for (tile, radius) in [(2.0,4.25),(7.5,9.25),(16.0,32.0)] {
            let mut input=level(); input.origin=[-31.5,-1.0,12.125];
            let config=EngineConfig {level_collision_tile_m:tile,level_collision_radius_m:radius,..Default::default()};
            let (mut guarded,mut reference)=(LevelCollision::new(&input,&config),LevelCollision::new(&input,&config));
            let (mut a,mut b)=(PhysicsWorld::new(),PhysicsWorld::new());
            let mut trace=Vec::new();
            for i in 0..1800 {
                let t=i as f32*0.007;
                trace.push([input.origin[0]+32.0+38.0*t.sin(),0.0,input.origin[2]+32.0+38.0*(t*0.71).cos()]);
            }
            // Tiny movement/stationary frames should reuse a search; exact build/drop
            // boundaries, entering the level and large jumps must always rescan.
            trace.extend((0..120).map(|i|[0.743+(i as f32).sin()*1e-5,7.0,31.679]));
            for k in [0.0,1.0,3.0,8.0] { for delta in [-1e-4,0.0,1e-4] {
                trace.push([input.origin[0]+k*tile-radius+delta,0.0,input.origin[2]+0.713]);
                trace.push([input.origin[0]+k*tile+radius*1.5+delta,0.0,input.origin[2]+0.713]);
            }}
            trace.extend([[-1000.0,0.0,-1000.0],[1000.0,0.0,1000.0],[0.743,0.0,31.679]]);
            for (i,p) in trace.into_iter().enumerate() {
                reference.search_guard=None;
                assert_eq!(guarded.update(&mut a,p),reference.update(&mut b,p),"tile={tile} radius={radius} frame={i} point={p:?}");
                assert_eq!(guarded.tiles.keys().collect::<Vec<_>>(),reference.tiles.keys().collect::<Vec<_>>(),"active tile set at {p:?}");
                assert_eq!(guarded.generation,reference.generation);
                assert_eq!((a.bodies.len(),a.colliders.len()),(b.bodies.len(),b.colliders.len()));
                if i%127==0 {assert_eq!(guarded.debug_material_segments(&a,4),reference.debug_material_segments(&b,4));}
            }
            assert!(guarded.reused_searches>0,"fixture must exercise cached searches");
            assert_eq!(guarded.update_calls,guarded.searches+guarded.reused_searches);
        }
    }

    #[test]
    fn empty_tile_guard_preserves_later_entry_to_occupied_region() {
        let mut input=level();input.cells.retain(|c|c[0]>128);
        let config=EngineConfig {level_collision_tile_m:4.0,level_collision_radius_m:5.0,..Default::default()};
        let (mut guarded,mut reference)=(LevelCollision::new(&input,&config),LevelCollision::new(&input,&config));
        let (mut a,mut b)=(PhysicsWorld::new(),PhysicsWorld::new());
        for i in 0..1200 {
            let p=[-10.0+i as f32*0.07,0.0,31.713];reference.search_guard=None;
            assert_eq!(guarded.update(&mut a,p),reference.update(&mut b,p));
            assert_eq!(guarded.tiles.keys().collect::<Vec<_>>(),reference.tiles.keys().collect::<Vec<_>>());
        }
        assert!(guarded.reused_searches>0);
        assert_eq!(guarded.debug_material_segments(&a,4),reference.debug_material_segments(&b,4));
    }

    #[test]
    fn foliage_is_sensor_only_floor_rejects_leaves_and_hard_neighbours_survive() {
        let config=EngineConfig {level_collision_tile_m:2.0,level_collision_radius_m:4.0, ..Default::default()};
        let mut level=level();
        // Adjacent 25 cm leaf and masonry regions in one source obstacle box.
        level.wall_boxes=Some(vec![crate::svo::WallBox {min:[1.0,1.0,1.0],max:[1.5,1.25,1.25]}]);
        let map=crate::foliage::MaterialMap::from_cells([[4,0,4]]);
        let mut c=LevelCollision::new_with_materials(&level,&config,Some(&map));
        let mut world=PhysicsWorld::new();c.update(&mut world,[1.0,0.0,1.0]);world.step();
        assert!(c.foliage_profile()["soft_pieces"].as_u64().unwrap()>0);
        let soft=world.query_pipeline().intersect_point(Vector::new(1.125,0.125,1.125)).any(|(_,c)|c.is_sensor());
        assert!(soft,"classified leaves become sensors");
        let hard=world.query_pipeline_with_filter(QueryFilter::default().exclude_sensors()).intersect_point(Vector::new(1.375,0.125,1.125)).any(|(_,c)|!c.is_sensor());
        assert!(hard,"adjacent masonry retains solid collision");
        assert!(!world.query_pipeline_with_filter(QueryFilter::default().exclude_sensors()).intersect_point(Vector::new(1.125,0.125,1.125)).any(|(_,c)|c.shape().as_compound().is_some()),"leaf region cannot remain in hard compound");
        assert!(c.debug_material_segments(&world,4).iter().any(|(_,_,m)|*m==2));
        // Leaf tops must not become drivable elevated heightfields.
        level.cells=vec![[4,4,4]];
        let floor_map=crate::foliage::MaterialMap::from_cells([[4,0,4]]);
        let filtered=LevelCollision::new_with_materials(&level,&config,Some(&floor_map));
        assert_eq!(filtered.soft_floor_samples_removed,1);
    }

    #[test]
    fn delft_demo_surface_fills_holes_clears_floaters_and_keeps_outer_walls() {
        let config = EngineConfig { level_path: "levels/trompetstraat-delft".into(), ..Default::default() };
        // A false elevated scan platform at the north end must never replace the demo road.
        let cells=(240..340).step_by(2).flat_map(|x|(3160..3460).step_by(2).map(move|z|[x,200,z])).collect();
        let floor = LevelFloor { cells, origin: [-12.0,-5.0,-80.0], resolution: 0.05,
            dims: [600,600,3520], walls: vec![], wall_dims: [0,0], block_m: 0.2,
            spawn_hint: None, spawn_forward: None, wall_boxes: Some(vec![
                crate::svo::WallBox { min:[12.1,3.5,79.9], max:[12.3,4.5,80.1] },
                crate::svo::WallBox { min:[15.2,3.5,79.9], max:[15.4,4.5,80.1] },
                crate::svo::WallBox { min:[12.8,5.3,166.9], max:[13.0,6.3,167.1] },
            ]) };
        let mut c = LevelCollision::new(&floor,&config);
        assert_eq!(c.road_obstacles_removed,2);
        let road = c.road.clone().unwrap();
        let mut world = PhysicsWorld::new();
        // Continuous coverage in three wheel lanes, across every streamed tile seam.
        for z in -74..=90 {
            let p = road.centre(z as f32).unwrap();
            c.update(&mut world,p); world.step();
            let pipeline = world.query_pipeline();
            for offset in [-1.8,0.0,1.8] {
                // Avoid exact heightfield vertex lines, where f32 ray/triangle edge tests can
                // disagree despite adjacent triangles meeting (neighbour rays are continuous).
                let ray = Ray::new(Vector::new(p[0]+offset+0.003,p[1]+0.3,p[2]),-Vector::Y);
                let (_,t) = pipeline.cast_ray(&ray,1.0,true)
                    .unwrap_or_else(||panic!("missing demo road at {p:?}, offset {offset}"));
                assert!((t-0.3).abs()<0.01,"road seam/height error at {p:?}: {t}");
            }
        }
        let p = road.centre(0.0).unwrap();c.update(&mut world,p);world.step();
        let ray = Ray::new(Vector::new(p[0],-1.0,0.03),Vector::X);
        let (_,t) = world.query_pipeline().cast_ray(&ray,5.0,true).expect("outer facade preserved");
        assert!(t>2.9 && t<3.1,"floating road box removed, outer wall retained: {t}");
    }

    /// A flat 64 × 64 m floor at y = 0 (cells of 0.25 m), no walls.
    fn level() -> LevelFloor {
        let n = 256u32;
        let cells = (0..n).flat_map(|x| (0..n).map(move |z| [x, 3, z])).collect();
        LevelFloor { cells, origin: [0.0, -1.0, 0.0], resolution: 0.25, dims: [n, 8, n], walls: Vec::new(), wall_boxes: None, wall_dims: [0, 0], block_m: 1.0, spawn_hint: None, spawn_forward: None }
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
    fn source_fitted_walls_do_not_block_empty_space_above_low_objects() {
        let mut input=level();
        input.wall_boxes=Some(vec![crate::svo::WallBox {min:[5.0,1.0,5.0],max:[6.0,1.5,6.0]}]);
        // Deliberately retain the legacy candidate; fitted geometry must take precedence.
        input.walls=vec![Some(0);64*64]; input.wall_dims=[64,64];
        let mut collision=LevelCollision::new(&input,&EngineConfig::default());
        let mut world=PhysicsWorld::new(); collision.update(&mut world,[5.0,0.0,5.0]); world.step();
        let query=world.query_pipeline();
        assert!(query.cast_ray(&Ray::new(Vector::new(4.0,1.0,5.5),Vector::X),3.0,true).is_none(),
            "low obstacle must not invent a tall wall above itself");
        assert!(query.cast_ray(&Ray::new(Vector::new(4.0,0.25,5.5),Vector::X),3.0,true).is_some(),
            "the actual obstacle still collides");
        assert!(collision.debug_segments(&world,4).iter().any(|(_,_,wall)| *wall));
    }

    #[test]
    fn fitted_obstacles_split_across_tiles_without_gaps_or_many_broad_phase_colliders() {
        let mut input=level();
        input.wall_boxes=Some((0..20).map(|i| crate::svo::WallBox {
            min:[15.9,1.0,4.0+i as f32*0.2],max:[16.1,1.5,4.1+i as f32*0.2]
        }).collect());
        let mut world=PhysicsWorld::new();
        let mut collision=LevelCollision::new(&input,&EngineConfig::default());
        collision.update(&mut world,[16.0,0.0,5.0]);world.step();
        assert!(world.colliders.len()<=collision.tiles()*2,"one compound plus one floor per tile");
        for x in [15.99,16.0,16.01] {
            let ray=Ray::new(Vector::new(x,2.0,4.05),-Vector::Y);
            let (_,toi)=world.query_pipeline().cast_ray(&ray,3.0,true).unwrap();
            assert!((2.0-toi-0.5).abs()<0.015,"tile boundary must keep the actual obstacle surface: x={x}, top={}",2.0-toi);
        }
    }

    #[test]
    fn floor_does_not_connect_road_to_high_scan_surfaces_with_invisible_ramps() {
        let mut input=level();
        for c in &mut input.cells { if c[0]>=128 {c[1]=23;} }
        let mut world=PhysicsWorld::new();
        let mut collision=LevelCollision::new(&input,&EngineConfig::default());
        collision.update(&mut world,[32.0,0.0,20.0]); world.step();
        let query=world.query_pipeline();
        assert!(query.cast_ray(&Ray::new(Vector::new(30.0,1.0,20.0),Vector::X),5.0,true).is_none(),
            "road-to-roof interpolation must not become an invisible blocking wall");
        for (x,expected) in [(30.0,0.0),(34.0,5.0)] {
            let hit=query.cast_ray(&Ray::new(Vector::new(x,10.0,20.0),-Vector::Y),20.0,true).unwrap();
            assert!((10.0-hit.1-expected).abs()<0.01,"keep the supported surfaces themselves");
        }
    }

    #[test]
    fn the_floor_follows_a_ramp_along_x_and_is_smooth_across_tiles() {
        // 32 × 32 m of 5 cm voxels rising one voxel every 8 cells (40 cm) along x, flat along z: a 12.5 % ramp built
        // of 5 cm stairs, as the voxel floor stores a smooth slope.
        let n = 640u32;
        let cells = (0..n).flat_map(|x| (0..n).map(move |z| [x, x / 8, z])).collect();
        let floor = LevelFloor { cells, origin: [0.0, -1.0, 0.0], resolution: 0.05, dims: [n, 128, n], walls: Vec::new(), wall_boxes: None, wall_dims: [0, 0], block_m: 1.0, spawn_hint: None, spawn_forward: None };
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
