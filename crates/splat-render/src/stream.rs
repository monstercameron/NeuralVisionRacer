//! Level-of-detail streaming for a level's splats: the finest detail near the camera, coarser further away, and only
//! the chunk files that selection needs in memory.
//!
//! A streamed level (`lod-meta.json`) is a tree whose leaves are boxes of splats, each stored at every level of
//! detail (LOD 0 finest). Each frame:
//!
//! 1. **Select**: every leaf picks a LOD from its distance to the camera: LOD 0 within `lod_distance_m`, and each
//!    further LOD doubles the distance. Leaves outside the view (behind the camera or off to the side) go
//!    `behind_bias` LODs coarser, except those near the camera, which keep their detail whichever way it looks. A leaf only switches once it is clearly past a boundary (hysteresis), and a leaf
//!    that would go coarser keeps its detail for `keep_s` first, so glancing away or a brief squeeze on the budget
//!    does not throw detail away that is about to be needed again. The pool is a budget: every leaf starts at its
//!    coarsest LOD and leaves are refined, those in view first and then nearest first, while the extra splats fit,
//!    so detail goes where it is seen, largest first. When the pool cannot hold every leaf's target, the LOD distance
//!    shrinks a little each frame until it can (and grows back when there is room), so the targets fit the pool
//!    and leaves at the edge of the budget stop trading detail.
//! 2. **Request**: chunk files that hold a wanted leaf range and are not loaded are handed to the host to fetch and
//!    decode (`requests`, then `insert`). Leaves with nothing on screen ask for their coarsest file first, so the whole
//!    level appears quickly and refines from there.
//! 3. **Apply**: leaves whose wanted range is loaded are re-encoded into a shared format and written into a fixed
//!    GPU pool (`scene::pool`), in view and nearest first, up to `uploads_per_frame` splats per frame. A leaf's new
//!    range dissolves in over the first half of `fade_s` on top of the range it replaces, which then dissolves out
//!    over the second half, and only then is the old range released (`fades`: the depth-key pass drops a growing or shrinking share of each fading range's splats), so a
//!    change of LOD is a short crossfade instead of a pop, and a leaf never disappears while it changes. One sort and
//!    one draw still cover the whole pool, so the depth order is correct across leaves.
//! 4. **Evict**: a chunk file is dropped from memory once no pending upload needs it, beyond a small cache. Files
//!    for the next finer LOD of leaves the camera is approaching are fetched ahead of need and kept (`prefetch`).
//!
//! The shared format: positions re-quantised to the union of every chunk's range (16 bits per axis in log space, as
//! SOG stores them), codebooks merged over every chunk, and SH centroids in palette slots of the pool's centroid
//! texture. Only leaves finer than `sh_lods` get a palette (view-dependent colour matters close up); the rest point at
//! a reserved all-zero centroid.

use crate::lod::{LodMeta, Range};
use crate::scene::{GsplatScene, POOL_CENTROID_ROWS};
use crate::sog::{self, Bundle, Meta};
use std::collections::{BTreeMap, BTreeSet};

/// SH centroids per palette (one chunk's palette), and rows of the centroid texture per palette.
const PALETTE_CENTROIDS: u32 = 65536;
const PALETTE_ROWS: u32 = PALETTE_CENTROIDS / 64;
const SH_COEFFS: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamSettings {
    /// LOD 0 reaches this far from the camera, in level units (metres); each further LOD doubles it. Used only when
    /// `lod_error_px` is 0.
    pub lod_distance_m: f32,
    /// Screen-space error target: each leaf gets the coarsest LOD whose splats land at most this many pixels apart
    /// (at a 1080-pixel-tall view). Dense scans then use coarse levels where they look the same, and sparse ones keep
    /// fine levels further out. 0 picks by `lod_distance_m` instead.
    pub lod_error_px: f32,
    /// Pool slots: the most splats resident at once. 0 sizes the pool to the level (`AUTO_POOL_SHARE` of its LOD 0
    /// splats, between `AUTO_POOL_MIN` and `AUTO_POOL_MAX`): a dense scan needs more room for the same picture.
    pub capacity: u32,
    /// Leaves farther than this from the camera are not drawn at all (a draw distance), in level units.
    pub max_distance_m: f32,
    /// Extra LODs for leaves outside the view (behind the camera, or off to the side when the view is known).
    pub behind_bias: usize,
    /// Splats re-encoded and uploaded per frame, at most.
    pub uploads_per_frame: u32,
    /// LODs finer than this keep their spherical harmonics.
    pub sh_lods: usize,
    /// Decoded chunk files kept in memory once nothing pending needs them.
    pub cache_chunks: usize,
    /// Chunk files requested but not yet inserted, at most.
    pub max_in_flight: usize,
    /// Seconds a change of LOD crossfades over (0 swaps at once).
    pub fade_s: f32,
    /// Seconds a leaf keeps its finer LOD after its target first goes coarser.
    pub keep_s: f32,
    /// Leaves this close to the LOD centre count as in view whichever way the camera looks: the chase camera swings
    /// round the car (reversing, panning), and the detail around the car must already be there when it does.
    pub any_view_m: f32,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            lod_distance_m: 24.0,
            lod_error_px: 1.5,
            max_distance_m: 150.0,
            capacity: 3_000_000,
            behind_bias: 1,
            uploads_per_frame: 250_000,
            sh_lods: 1,
            cache_chunks: 8,
            max_in_flight: 3,
            fade_s: 0.35,
            keep_s: 1.5,
            any_view_m: 8.0,
        }
    }
}

/// One resident leaf for the debug layer: its box (splat space), the LOD it shows, whether it is waiting for a
/// different one, and whether it is in view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugLeaf {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub lod: usize,
    pub pending: bool,
    pub visible: bool,
}

/// What the stream holds right now, for status lines and the console.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamStats {
    pub resident_splats: u32,
    /// Splats the current selection asks for (resident once nothing is pending).
    pub wanted_splats: u64,
    /// The highest used pool slot: what the sort covers.
    pub active_slots: u32,
    pub leaves_per_lod: Vec<u32>,
    /// Leaves waiting for their first data.
    pub leaves_empty: u32,
    /// Leaves past the draw distance.
    pub leaves_hidden: u32,
    pub leaves_pending: u32,
    pub chunks_loaded: usize,
    pub chunks_in_flight: usize,
    /// Leaves in the view, and ranges crossfading now.
    pub leaves_visible: u32,
    pub fading: u32,
    /// Changes of LOD so far (uploads replacing a resident range), and how many of those were leaves in view
    /// within `lod_distance_m` (the swaps most likely to be seen).
    pub switches: u64,
    pub near_switches: u64,
    /// SH palette slots in use, and resident leaves fine enough for SH whose chunk has it but got no palette.
    pub palettes_in_use: u32,
    pub missing_sh: u32,
    /// Times crossfades were cut short for room, and leaves swapped without one (released before the upload).
    pub fades_cut: u64,
    pub hard_swaps: u64,
    /// The share of drawn leaves at the LOD their distance asks for (1 when the pool holds all the detail wanted).
    pub detail_met: f32,
    /// How far `lod_distance_m` is scaled to fit the pool (1: the full distance).
    pub detail_scale: f32,
}

impl StreamStats { pub fn json(&self)->serde_json::Value { serde_json::json!({"resident_splats":self.resident_splats,"wanted_splats":self.wanted_splats,"active_slots":self.active_slots,"leaves_per_lod":self.leaves_per_lod,"leaves_empty":self.leaves_empty,"leaves_hidden":self.leaves_hidden,"leaves_pending":self.leaves_pending,"chunks_loaded":self.chunks_loaded,"chunks_in_flight":self.chunks_in_flight,"leaves_visible":self.leaves_visible,"fading":self.fading,"switches":self.switches,"near_switches":self.near_switches,"palettes_in_use":self.palettes_in_use,"missing_sh":self.missing_sh,"fades_cut":self.fades_cut,"hard_swaps":self.hard_swaps,"detail_met":self.detail_met,"detail_scale":self.detail_scale}) } }

#[derive(Debug, Clone, Copy)]
struct Resident {
    lod: usize,
    start: u32,
    count: u32,
    palette: Option<usize>,
    /// When it was uploaded: it fades in over `fade_s` from then.
    born: f32,
}

#[derive(Debug, Clone, Copy)]
struct LeafState {
    /// The LOD to show; `None` past the draw distance.
    want: Option<usize>,
    have: Option<Resident>,
    distance: f32,
    /// In the view frustum (always true without one).
    visible: bool,
    /// Since when the selection has wanted this leaf coarser than it is.
    coarsen_since: Option<f32>,
}

struct Chunk {
    bundle: Bundle,
    last_used: u64,
}

/// Per-chunk mapping into the shared format.
struct Remap {
    meta: Meta,
    scales: [u8; 256],
    sh0: [u8; 256],
    sh_n: [u8; 256],
}

pub struct LevelStream {
    pub settings: StreamSettings,
    meta: LodMeta,
    shared: Meta,
    remaps: Vec<Option<Remap>>,
    /// Codebook index of 0 in the shared SH codebook: the reserved centroid's texels.
    sh_zero: u8,
    chunks: BTreeMap<usize, Chunk>,
    in_flight: BTreeSet<usize>,
    leaves: Vec<LeafState>,
    free: Vec<(u32, u32)>,
    /// Palette slot → (chunk file, leaves using it).
    palettes: Vec<Option<(usize, u32)>>,
    distance_scale: f32,
    frame: u64,
    started: bool,
    /// Seconds, from `select`.
    now: f32,
    /// Replaced ranges dissolving out, and when they started to.
    fading_out: Vec<(Resident, f32)>,
    switches: u64,
    near_switches: u64,
    fades_cut: u64,
    hard_swaps: u64,
    /// Whether the last `apply` had to cut crossfades short for room: the selection is using more of the pool than
    /// leaves crossfades space, so `select` treats it like a shortage.
    squeezed: bool,
    /// The latest LOD changes, for debugging churn: (time, leaf, from, to, distance, in view).
    recent: std::collections::VecDeque<(f32, usize, usize, usize, f32, bool)>,
    /// Per leaf and LOD, the typical distance between neighbouring splats (level units): the square root of the
    /// leaf's dominant face area over its splat count.
    spacing: Vec<Vec<f32>>,
    /// Pixels per unit of size at unit distance (from the projection and view height), for the screen-space error.
    focal_px: f32,
    /// The nearest in-view leaf drawn at LOD 2 or coarser (level units), for the depth of field.
    sharp_m: f32,
    /// `lod_distance_m` scaled to what the pool holds: shrinks while leaves miss their target for lack of room and
    /// grows back while room is spare, slowly either way.
    detail_scale: f32,
}

impl LevelStream {
    /// `chunk_metas[f]` is chunk file `f`'s `meta.json` (`None` if it could not be read: that file's ranges are
    /// skipped). The meta files are small; reading all of them up front gives the shared ranges and codebooks.
    pub fn new(meta: LodMeta, chunk_metas: &[Option<Vec<u8>>], mut settings: StreamSettings) -> Result<Self, String> {
        if settings.capacity == 0 {
            let lod0 = meta.counts.first().copied().unwrap_or(0) as f64;
            settings.capacity = ((lod0 * AUTO_POOL_SHARE) as u32).clamp(AUTO_POOL_MIN, AUTO_POOL_MAX);
        }
        let metas: Vec<Option<Meta>> = (0..meta.filenames.len())
            .map(|f| chunk_metas.get(f).and_then(|b| b.as_ref()).and_then(|b| sog::parse_meta_json(b).ok()))
            .collect();
        let known: Vec<&Meta> = metas.iter().flatten().collect();
        if known.is_empty() {
            return Err("no chunk meta.json could be read".into());
        }
        let mut means_mins = [f32::MAX; 3];
        let mut means_maxs = [f32::MIN; 3];
        for m in &known {
            for i in 0..3 {
                means_mins[i] = means_mins[i].min(m.means_mins[i]);
                means_maxs[i] = means_maxs[i].max(m.means_maxs[i]);
            }
        }
        let scales = sog::merged_codebook(known.iter().map(|m| m.scales_codebook.as_slice()));
        let sh0 = sog::merged_codebook(known.iter().map(|m| m.sh0_codebook.as_slice()));
        let mut sh_n = sog::merged_codebook(known.iter().map(|m| m.sh_n_codebook.as_slice()));
        // The reserved centroid must decode to exactly zero, so one codebook entry is zero.
        let zero = (0..256).min_by(|&a, &b| sh_n[a].abs().total_cmp(&sh_n[b].abs())).unwrap_or(0);
        sh_n[zero] = 0.0;
        let shared = Meta { count: 0, means_mins, means_maxs, scales_codebook: scales, sh0_codebook: sh0, sh_n_codebook: sh_n, sh_n_count: 0 };
        let remaps = metas
            .into_iter()
            .map(|m| {
                m.map(|m| Remap {
                    scales: sog::remap(&m.scales_codebook, &shared.scales_codebook),
                    sh0: sog::remap(&m.sh0_codebook, &shared.sh0_codebook),
                    sh_n: sog::remap(&m.sh_n_codebook, &shared.sh_n_codebook),
                    meta: m,
                })
            })
            .collect();
        let coarsest = meta.levels().saturating_sub(1);
        let leaves = meta.leaves.iter().map(|_| LeafState { want: Some(coarsest), have: None, distance: f32::MAX, visible: true, coarsen_since: None }).collect();
        // Sized for the largest centroid texture; `init_pool` fits it to the pool's.
        let palettes = vec![None; ((POOL_CENTROID_ROWS - 1) / PALETTE_ROWS) as usize];
        let spacing = meta
            .leaves
            .iter()
            .map(|leaf| {
                let mut size = [0, 1, 2].map(|i| (leaf.max[i] - leaf.min[i]).max(0.01));
                size.sort_by(f32::total_cmp);
                let area = size[2] * size[1];
                leaf.lods.iter().map(|r| r.map_or(f32::MAX, |r| (area / r.count.max(1) as f32).sqrt())).collect()
            })
            .collect();
        Ok(Self {
            free: vec![(0, settings.capacity)],
            settings,
            meta,
            shared,
            remaps,
            sh_zero: zero as u8,
            chunks: BTreeMap::new(),
            in_flight: BTreeSet::new(),
            leaves,
            palettes,
            distance_scale: 1.0,
            frame: 0,
            started: false,
            now: 0.0,
            fading_out: Vec::new(),
            switches: 0,
            near_switches: 0,
            fades_cut: 0,
            hard_swaps: 0,
            squeezed: false,
            recent: Default::default(),
            detail_scale: 1.0,
            spacing,
            focal_px: 1000.0,
            sharp_m: f32::MAX,
        })
    }

    /// Builds the stream from `lod-meta.json` and every chunk's `meta.json` given together as one JSON array (in file
    /// order; `null` for one that could not be read). The browser fetches them that way.
    pub fn from_json(lod_meta: &[u8], chunk_metas: &[u8], settings: StreamSettings) -> Result<Self, String> {
        let meta = LodMeta::parse(lod_meta)?;
        let all: serde_json::Value = serde_json::from_slice(chunk_metas).map_err(|e| format!("chunk metas: {e}"))?;
        let metas: Vec<Option<Vec<u8>>> = all
            .as_array()
            .ok_or("chunk metas: expected an array")?
            .iter()
            .map(|m| (!m.is_null()).then(|| serde_json::to_vec(m).unwrap_or_default()))
            .collect();
        Self::new(meta, &metas, settings)
    }

    /// The shared position range and codebooks, for `scene::pool`.
    pub fn shared_meta(&self) -> &Meta {
        &self.shared
    }

    /// The level's tree.
    pub fn lod_meta(&self) -> &LodMeta {
        &self.meta
    }

    /// Picks each leaf's LOD for a camera at `eye` looking along `forward`, both in the level's splat space, at time
    /// `now_s` (seconds). `clip` (projection · view · level model, column-major) gives the view frustum; without it
    /// only leaves entirely behind the camera count as out of view.
    ///
    /// Every leaf in the draw distance starts at its coarsest LOD; then, leaves in view first and nearest first, each
    /// is refined to the finest LOD its distance allows (with hysteresis, the out-of-view bias and the keep delay)
    /// whose extra splats still fit the pool. Detail is spent where it is seen, and the selection always fits.
    pub fn select(&mut self, eye: [f32; 3], forward: [f32; 3], clip: Option<&[f32; 16]>, now_s: f32) {
        self.frame += 1;
        self.now = now_s;
        let keep_s = self.settings.keep_s;
        let count = |leaf: &crate::lod::Leaf, lod: usize| u64::from(leaf.lods[lod].map_or(0, |r| r.count));
        let planes = clip.map(|c| (frustum_planes(c, ENTER_MARGIN), frustum_planes(c, LEAVE_MARGIN)));
        let mut budget = i64::from(self.settings.capacity) * SELECT_SHARE / 100;
        let mut targets = vec![None; self.leaves.len()];
        let levels: Vec<f32> = (0..self.leaves.len())
            .map(|i| {
                let leaf = &self.meta.leaves[i];
                self.level_at(i, effective_distance(eye, leaf.min, leaf.max))
            })
            .collect();
        for (i, (state, leaf)) in self.leaves.iter_mut().zip(&self.meta.leaves).enumerate() {
            let distance = effective_distance(eye, leaf.min, leaf.max);
            state.distance = distance;
            if distance > self.settings.max_distance_m {
                continue;
            }
            let Some(coarse) = coarsest_available(&leaf.lods) else { continue };
            // In view: inside the frustum (entering by the narrow margin, leaving by the wide one), or without one, not
            // entirely behind the camera. Leaves near the camera always count.
            state.visible = distance <= self.settings.any_view_m || match &planes {
                Some((enter, leave)) => box_in_frustum(if state.visible { leave } else { enter }, leaf.min, leaf.max),
                None => {
                    let centre = [0, 1, 2].map(|i| (leaf.min[i] + leaf.max[i]) * 0.5);
                    let radius = (0..3).map(|i| (leaf.max[i] - leaf.min[i]).powi(2)).sum::<f32>().sqrt() * 0.5;
                    (0..3).map(|i| (centre[i] - eye[i]) * forward[i]).sum::<f32>() >= -radius
                }
            };
            // Continuous level (`level_at`), plus the bias out of view. The bias goes in before the hysteresis, so both
            // compare the same level.
            let bias = if state.visible { 0.0 } else { self.settings.behind_bias as f32 };
            let x = levels[i] + bias;
            let mut lod = x.floor() as usize;
            // Hysteresis: keep the resident LOD while within a fifth of a level of its band.
            if let Some(have) = state.have {
                if x > have.lod as f32 - 0.2 && x < have.lod as f32 + 1.2 {
                    lod = have.lod;
                }
            }
            // Going coarser waits `keep_s`: the leaf may be back in view, or the budget back, by then.
            match state.have {
                Some(have) if lod > have.lod => {
                    let since = *state.coarsen_since.get_or_insert(now_s);
                    if now_s - since < keep_s {
                        lod = have.lod;
                    }
                }
                _ => state.coarsen_since = None,
            }
            targets[i] = Some((lod.min(coarse), coarse));
            budget -= count(leaf, coarse) as i64;
        }
        let mut order: Vec<usize> = (0..self.leaves.len()).filter(|&i| targets[i].is_some()).collect();
        // Leaves in view first, then nearest, a leaf already at its target LOD counting as nearer.
        let priority = |i: usize| {
            let state = &self.leaves[i];
            let settled = state.have.is_some_and(|h| targets[i].is_some_and(|(t, _)| h.lod <= t));
            state.distance * if settled { STICKY } else { 1.0 }
        };
        order.sort_by(|&a, &b| self.leaves[b].visible.cmp(&self.leaves[a].visible).then(priority(a).total_cmp(&priority(b))));
        for &i in &order {
            let (target, coarse) = targets[i].expect("filtered");
            let leaf = &self.meta.leaves[i];
            let base = count(leaf, coarse) as i64;
            // The finest LOD from the target down that exists and fits what is left.
            let chosen = (target..=coarse)
                .filter(|&l| leaf.lods[l].is_some())
                .find(|&l| count(leaf, l) as i64 - base <= budget)
                .unwrap_or(coarse);
            budget -= count(leaf, chosen) as i64 - base;
            self.leaves[i].want = Some(chosen);
        }
        for (i, state) in self.leaves.iter_mut().enumerate() {
            if targets[i].is_none() {
                state.want = None;
            }
        }
        self.sharp_m = self
            .leaves
            .iter()
            .filter(|s| s.visible && s.want.is_some_and(|w| w >= 2))
            .map(|s| s.distance)
            .fold(f32::MAX, f32::min);
        // How much of the configured detail the pool allowed: the share of leaves that got their distance's LOD.
        let refined = order.iter().filter(|&&i| self.leaves[i].want == targets[i].map(|t| t.0)).count();
        self.distance_scale = if order.is_empty() { 1.0 } else { refined as f32 / order.len() as f32 };
        // When the pool cannot hold every leaf's target, leaves at the edge of the budget trade detail back and forth
        // as the camera moves. Shrinking the LOD distance until the targets fit makes them stable again (the
        // hysteresis and keep delay then hold); spare room lets it grow back. Slowly, so it does not oscillate.
        // Between "a few leaves short" and "15 % of the pool spare" it holds still, so it settles instead of hunting.
        // Crossfades cut short for room count as a shortage too: the selection must leave them space.
        let short = (order.len() - refined) * 50 > order.len() || std::mem::take(&mut self.squeezed);
        let spare = budget > i64::from(self.settings.capacity) * 15 / 100;
        self.detail_scale = if short {
            (self.detail_scale * 0.99).max(0.25)
        } else if spare {
            (self.detail_scale * 1.003).min(1.0)
        } else {
            self.detail_scale
        };
        self.started = true;
    }

    /// Chunk files to fetch now, as (file index, chunk folder), marked as in flight. Leaves with nothing resident ask
    /// for their coarsest file first; the rest for their wanted file; leaves in view first, then nearest.
    pub fn requests(&mut self) -> Vec<(usize, String)> {
        let room = self.settings.max_in_flight.saturating_sub(self.in_flight.len());
        if room == 0 || !self.started {
            return Vec::new();
        }
        let mut wanted: Vec<(bool, bool, f32, usize)> = Vec::new();
        for (state, leaf) in self.leaves.iter().zip(&self.meta.leaves) {
            let Some(want) = state.want else { continue };
            if state.have.is_some_and(|h| h.lod == want) {
                continue;
            }
            let lod = if state.have.is_none() { coarsest_available(&leaf.lods) } else { Some(want) };
            let Some(range) = lod.and_then(|l| leaf.lods[l]) else { continue };
            if self.chunks.contains_key(&range.file) || self.in_flight.contains(&range.file) || self.remaps[range.file].is_none() {
                continue;
            }
            wanted.push((state.have.is_some(), !state.visible, state.distance, range.file));
        }
        wanted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
        // Then prefetch: files the car is about to need, so detail is there when it arrives.
        let ahead = self.prefetch().into_iter().map(|(distance, file)| (true, true, distance, file));
        let mut out = Vec::new();
        for (_, _, _, file) in wanted.into_iter().chain(ahead) {
            if out.len() >= room {
                break;
            }
            if self.chunks.contains_key(&file) || self.remaps[file].is_none() {
                continue;
            }
            if self.in_flight.insert(file) {
                out.push((file, self.meta.dir_of(file).unwrap_or_default().to_string()));
            }
        }
        out
    }

    /// Chunk files worth having before they are wanted, nearest first, as (distance, file): for each leaf in view
    /// that would want its next finer LOD if it were `PREFETCH_REACH` times nearer (far enough ahead to arrive in
    /// time at 100 km/h), that LOD's file. Fetched
    /// when nothing more urgent is, and kept in memory while they qualify, so a leaf refines as soon as the car is
    /// near enough instead of waiting for its chunk to download and decode.
    fn prefetch(&self) -> Vec<(f32, usize)> {
        const PREFETCH_REACH: f32 = 2.5;
        let mut files: BTreeMap<usize, f32> = BTreeMap::new();
        for (i, (state, leaf)) in self.leaves.iter().zip(&self.meta.leaves).enumerate() {
            let Some(want) = state.want.filter(|&w| w > 0) else { continue };
            if !state.visible {
                continue;
            }
            // Would it want a finer LOD `PREFETCH_REACH` times nearer?
            if self.level_at(i, state.distance / PREFETCH_REACH).floor() as usize >= want {
                continue;
            }
            if let Some(range) = leaf.lods[want - 1] {
                let d = files.entry(range.file).or_insert(f32::MAX);
                *d = d.min(state.distance);
            }
        }
        let mut out: Vec<(f32, usize)> = files.into_iter().map(|(f, d)| (d, f)).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// A requested chunk file arrived.
    pub fn insert(&mut self, file: usize, bundle: Bundle) {
        self.in_flight.remove(&file);
        self.chunks.insert(file, Chunk { bundle, last_used: self.frame });
    }

    /// A requested chunk file could not be loaded; it may be requested again later.
    pub fn failed(&mut self, file: usize) {
        self.in_flight.remove(&file);
    }

    /// Writes the reserved zero centroid and sizes the palettes to the pool's centroid texture. Call once on a new pool.
    pub fn init_pool(&mut self, pool: &GsplatScene, queue: &wgpu::Queue) {
        self.palettes = vec![None; (pool.centroid_rows().saturating_sub(1) / PALETTE_ROWS) as usize];
        let row = vec![self.sh_zero, self.sh_zero, self.sh_zero, 255].repeat(64 * SH_COEFFS);
        pool.write_centroid_rows(queue, 0, &row);
    }

    /// Uploads leaves whose wanted range is loaded, in view and nearest first, within the per-frame budget; releases
    /// ranges that have faded out; hands the pool the ranges still fading; then drops chunks nothing pending needs.
    /// Returns the splats uploaded.
    pub fn apply(&mut self, pool: &GsplatScene, queue: &wgpu::Queue) -> u32 {
        let now = self.now;
        // Ranges done fading out give their slots back.
        let fade_s = self.settings.fade_s;
        let (done, still): (Vec<_>, Vec<_>) = self.fading_out.drain(..).partition(|&(_, t0)| now - t0 >= fade_s);
        self.fading_out = still;
        for (old, _) in done {
            self.free_resident(pool, queue, old);
        }
        // Leaves past the draw distance fade out.
        for i in 0..self.leaves.len() {
            if self.leaves[i].want.is_none() {
                if let Some(old) = self.leaves[i].have.take() {
                    self.retire(pool, queue, old);
                }
            }
        }
        let mut order: Vec<usize> = (0..self.leaves.len())
            .filter(|&i| self.leaves[i].want.is_some_and(|w| self.leaves[i].have.is_none_or(|h| h.lod != w)) || self.wants_palette(i))
            .collect();
        // Empty leaves first, then those in view, then nearest.
        order.sort_by(|&a, &b| {
            let (sa, sb) = (&self.leaves[a], &self.leaves[b]);
            sa.have.is_some().cmp(&sb.have.is_some()).then(sb.visible.cmp(&sa.visible)).then(sa.distance.total_cmp(&sb.distance))
        });
        let mut budget = self.settings.uploads_per_frame;
        let mut uploaded = 0;
        for i in order {
            let Some(want) = self.leaves[i].want else { continue };
            let leaf_lods = self.meta.leaves[i].lods.clone();
            // The wanted range if its chunk is here; an empty leaf takes any loaded range instead, nearest LOD first.
            let target = if leaf_lods[want].is_some_and(|r| self.chunks.contains_key(&r.file)) {
                Some(want)
            } else if self.leaves[i].have.is_none() {
                (0..leaf_lods.len())
                    .filter(|&l| leaf_lods[l].is_some_and(|r| self.chunks.contains_key(&r.file)))
                    .min_by_key(|&l| l.abs_diff(want))
            } else {
                None
            };
            let Some(lod) = target else { continue };
            let range = leaf_lods[lod].expect("checked above");
            if range.count > budget && uploaded > 0 {
                break;
            }
            // A new range normally goes in beside the old one, which fades out, so the leaf never blinks out. Out of
            // room, the oldest crossfades finish early, one at a time and only as many as it takes (cutting them all
            // at once would pop many leaves in one frame); then the leaf's old range is released first; if even that
            // does not make room, the leaf keeps its old LOD.
            let mut start = self.allocate(range.count);
            while start.is_none() && !self.fading_out.is_empty() {
                let oldest = (0..self.fading_out.len()).min_by(|&a, &b| self.fading_out[a].1.total_cmp(&self.fading_out[b].1)).expect("not empty");
                let (old, _) = self.fading_out.swap_remove(oldest);
                self.free_resident(pool, queue, old);
                self.fades_cut += 1;
                self.squeezed = true;
                start = self.allocate(range.count);
            }
            let start = match start {
                Some(start) => start,
                None => {
                    let Some(old) = self.leaves[i].have else { continue };
                    self.release(old.start, old.count);
                    match self.allocate(range.count) {
                        Some(start) => {
                            self.hard_swaps += 1;
                            pool.release(queue, old.start, old.count);
                            if let Some(p) = old.palette {
                                self.unuse_palette(p);
                            }
                            self.leaves[i].have = None;
                            start
                        }
                        None => {
                            self.take_exact(old.start, old.count);
                            continue;
                        }
                    }
                }
            };
            let palette = if lod < self.settings.sh_lods { self.palette_for(range.file, pool, queue) } else { None };
            self.upload(pool, queue, range, start, palette);
            pool.write_lod(queue, start, range.count, lod, i);
            if let Some(old) = self.leaves[i].have {
                let state = &self.leaves[i];
                self.recent.push_back((now, i, old.lod, lod, state.distance, state.visible));
                if self.recent.len() > 512 {
                    self.recent.pop_front();
                }
                self.switches += 1;
                if self.leaves[i].visible && self.leaves[i].distance <= self.settings.lod_distance_m {
                    self.near_switches += 1;
                }
            }
            if let Some(old) = self.leaves[i].have.take() {
                self.retire(pool, queue, old);
            }
            self.leaves[i].have = Some(Resident { lod, start, count: range.count, palette, born: now });
            if let Some(chunk) = self.chunks.get_mut(&range.file) {
                chunk.last_used = self.frame;
            }
            budget = budget.saturating_sub(range.count);
            uploaded += range.count;
        }
        pool.set_active(queue, self.high_water());
        pool.set_fades(queue, &self.fades());
        self.evict();
        uploaded
    }

    /// Whether a resident leaf showing a LOD that gets spherical harmonics went without them (every palette slot was
    /// taken when it was uploaded) and could have them now: its chunk is loaded and a slot is free or already holds
    /// its chunk. Such a leaf is uploaded again, crossfading to the same LOD with its view-dependent colour.
    fn wants_palette(&self, i: usize) -> bool {
        let (state, leaf) = (&self.leaves[i], &self.meta.leaves[i]);
        let Some(have) = state.have else { return false };
        if have.palette.is_some() || have.lod >= self.settings.sh_lods || state.want != Some(have.lod) {
            return false;
        }
        let Some(range) = leaf.lods[have.lod] else { return false };
        let has_sh = self.remaps[range.file].as_ref().is_some_and(|m| m.meta.sh_n_count > 0);
        let room = self.palettes.iter().any(|p| p.is_none_or(|(f, users)| users == 0 || f == range.file));
        has_sh && room && self.chunks.contains_key(&range.file)
    }

    /// The ranges crossfading now, by start: (first slot, one past the last, share of its splats drawn). A new range
    /// grows from none to all over the first half of `fade_s` while the range it replaces stays whole; the old one
    /// then shrinks to none over the second half. One of the two is always whole, so the leaf never thins out.
    pub fn fades(&self) -> Vec<(u32, u32, f32)> {
        let fade_s = self.settings.fade_s;
        if fade_s <= 0.0 {
            return Vec::new();
        }
        let half = fade_s / 2.0;
        let progress = |t: f32| (t / half).clamp(0.0, 1.0);
        let mut out: Vec<(u32, u32, f32)> = self
            .leaves
            .iter()
            .filter_map(|s| s.have)
            .filter(|r| self.now - r.born < half)
            .map(|r| (r.start, r.start + r.count, progress(self.now - r.born)))
            .chain(self.fading_out.iter().map(|(r, t0)| (r.start, r.start + r.count, 1.0 - progress(self.now - t0 - half))))
            .collect();
        out.sort_by_key(|f| f.0);
        out
    }

    /// A range a leaf no longer shows: it fades out, or goes at once when fading is off.
    fn retire(&mut self, pool: &GsplatScene, queue: &wgpu::Queue, old: Resident) {
        if self.settings.fade_s > 0.0 {
            self.fading_out.push((old, self.now));
        } else {
            self.free_resident(pool, queue, old);
        }
    }

    fn free_resident(&mut self, pool: &GsplatScene, queue: &wgpu::Queue, old: Resident) {
        pool.release(queue, old.start, old.count);
        self.release(old.start, old.count);
        if let Some(p) = old.palette {
            self.unuse_palette(p);
        }
    }

    /// How far from the LOD centre the level is sharp: the nearest leaf in view drawn at LOD 2 or coarser (by the
    /// distance bands, half way through the LOD 1 band). The depth of field starts softening there.
    pub fn sharp_distance(&self) -> f32 {
        if self.settings.lod_error_px > 0.0 {
            self.sharp_m.min(self.settings.max_distance_m)
        } else {
            self.settings.lod_distance_m * self.detail_scale * 1.5
        }
    }

    /// The view's pixels per unit of size at unit distance: projection[5] (1 / tan of half the vertical field of view)
    /// times half the view height. The screen-space error is measured at a 1080-pixel-tall view whatever the window,
    /// so the detail does not change with the window size; pass `projection[5] * 540`.
    pub fn set_focal_px(&mut self, focal_px: f32) {
        self.focal_px = focal_px.max(1.0);
    }

    /// The continuous level a leaf wants at `distance`: by screen-space error, the LOD whose splats land
    /// `lod_error_px` apart, interpolated between levels (2.3 is 30 % of the way from LOD 2 to LOD 3); by distance
    /// bands, 0 inside d0, 1 at 2·d0, 2 at 4·d0.
    fn level_at(&self, leaf: usize, distance: f32) -> f32 {
        let error_px = self.settings.lod_error_px;
        if error_px <= 0.0 {
            let d0 = (self.settings.lod_distance_m * self.detail_scale).max(0.1);
            return if distance <= d0 { 0.0 } else { (distance / d0).log2() + 1.0 };
        }
        // A smaller budget allows a larger error.
        let target = error_px / self.detail_scale;
        let error = |l: usize| self.spacing[leaf][l] * self.focal_px / distance.max(0.05);
        let levels = self.spacing[leaf].len();
        if error(0) >= target {
            return 0.0;
        }
        for l in 0..levels.saturating_sub(1) {
            let (a, b) = (error(l), error(l + 1));
            if b > target {
                // Between l (fine enough) and l + 1 (too coarse): how far in log space.
                return l as f32 + ((target / a).ln() / (b / a).max(1.0001).ln()).clamp(0.0, 0.999);
            }
        }
        (levels - 1) as f32
    }

    /// Checks the pool bookkeeping: resident and fading ranges never overlap each other or the free list, and
    /// together with it they cover the pool exactly. Returns the first problem found, for tests and probes.
    pub fn check_pool(&self) -> Result<(), String> {
        let mut used: Vec<(u32, u32, String)> = self
            .leaves
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.have.map(|h| (h.start, h.count, format!("leaf {i} LOD {}", h.lod))))
            .chain(self.fading_out.iter().map(|(r, _)| (r.start, r.count, format!("fading LOD {}", r.lod))))
            .chain(self.free.iter().map(|&(s, c)| (s, c, "free".to_string())))
            .collect();
        used.sort_by_key(|u| u.0);
        let mut at = 0u32;
        for (start, count, what) in &used {
            if *start < at {
                return Err(format!("{what} at {start}..{} overlaps what ends at {at}", start + count));
            }
            if *start > at {
                return Err(format!("slots {at}..{start} are neither used nor free"));
            }
            at = start + count;
        }
        if at != self.settings.capacity {
            return Err(format!("bookkeeping ends at {at}, not the capacity {}", self.settings.capacity));
        }
        Ok(())
    }

    /// Every leaf with something resident, for the debug layer (`Frame::set_lod_debug`).
    pub fn debug_leaves(&self) -> Vec<DebugLeaf> {
        self.leaves
            .iter()
            .zip(&self.meta.leaves)
            .filter_map(|(state, leaf)| {
                let have = state.have?;
                Some(DebugLeaf { min: leaf.min, max: leaf.max, lod: have.lod, pending: state.want.is_some_and(|w| w != have.lod), visible: state.visible })
            })
            .collect()
    }

    /// The latest LOD changes as a JSON array of [time, leaf, from, to, distance, in view], oldest first.
    pub fn recent_switches_json(&self) -> String {
        let rows: Vec<String> = self.recent.iter().map(|(t, i, a, b, d, v)| format!("[{t:.3},{i},{a},{b},{d:.1},{}]", u8::from(*v))).collect();
        format!("[{}]", rows.join(","))
    }

    /// The `n` leaves nearest the LOD centre, nearest first, as JSON objects: distance (effective), box size, wanted
    /// and resident LOD, in view, and whether the wanted range's chunk is loaded or in flight. For debugging pop-in.
    pub fn nearest_leaves_json(&self, n: usize) -> String {
        let mut order: Vec<usize> = (0..self.leaves.len()).collect();
        order.sort_by(|&a, &b| self.leaves[a].distance.total_cmp(&self.leaves[b].distance));
        let rows: Vec<String> = order
            .into_iter()
            .take(n)
            .map(|i| {
                let (state, leaf) = (&self.leaves[i], &self.meta.leaves[i]);
                let file = state.want.and_then(|w| leaf.lods[w]).map(|r| r.file);
                let size = [0, 1, 2].map(|k| (leaf.max[k] - leaf.min[k]).round());
                let counts: Vec<u32> = leaf.lods.iter().map(|r| r.map_or(0, |r| r.count)).collect();
                format!(
                    "{{\"leaf\":{i},\"d\":{:.1},\"size\":{size:?},\"want\":{:?},\"have\":{:?},\"visible\":{},\"loaded\":{},\"in_flight\":{},\"sh\":{},\"counts\":{counts:?}}}",
                    state.distance,
                    state.want,
                    state.have.map(|h| h.lod),
                    state.visible,
                    file.is_some_and(|f| self.chunks.contains_key(&f)),
                    file.is_some_and(|f| self.in_flight.contains(&f)),
                    state.have.is_some_and(|h| h.palette.is_some()),
                )
            })
            .collect();
        format!("[{}]", rows.join(","))
    }

    pub fn stats(&self) -> StreamStats {
        let mut per_lod = vec![0u32; self.meta.levels()];
        let mut empty = 0;
        let mut pending = 0;
        let mut resident = 0;
        let mut hidden = 0;
        let visible = self.leaves.iter().filter(|s| s.want.is_some() && s.visible).count() as u32;
        for s in &self.leaves {
            match (s.have, s.want) {
                (_, None) => hidden += 1,
                (Some(h), Some(want)) => {
                    per_lod[h.lod] += 1;
                    resident += h.count;
                    if h.lod != want {
                        pending += 1;
                    }
                }
                (None, Some(_)) => empty += 1,
            }
        }
        let wanted = self
            .leaves
            .iter()
            .zip(&self.meta.leaves)
            .filter_map(|(s, l)| s.want.and_then(|w| l.lods[w]).map(|r| u64::from(r.count)))
            .sum();
        StreamStats {
            resident_splats: resident,
            wanted_splats: wanted,
            active_slots: self.high_water(),
            leaves_per_lod: per_lod,
            leaves_empty: empty,
            leaves_hidden: hidden,
            leaves_pending: pending,
            chunks_loaded: self.chunks.len(),
            chunks_in_flight: self.in_flight.len(),
            leaves_visible: visible,
            fading: self.fades().len() as u32,
            switches: self.switches,
            near_switches: self.near_switches,
            palettes_in_use: self.palettes.iter().filter(|p| p.is_some_and(|(_, users)| users > 0)).count() as u32,
            missing_sh: self
                .leaves
                .iter()
                .zip(&self.meta.leaves)
                .filter(|(s, l)| {
                    s.have.is_some_and(|h| {
                        h.lod < self.settings.sh_lods
                            && h.palette.is_none()
                            && l.lods[h.lod].is_some_and(|r| self.remaps[r.file].as_ref().is_some_and(|m| m.meta.sh_n_count > 0))
                    })
                })
                .count() as u32,
            fades_cut: self.fades_cut,
            hard_swaps: self.hard_swaps,
            detail_met: self.distance_scale,
            detail_scale: self.detail_scale,
        }
    }

    /// Re-encodes a leaf range from its chunk into the shared format and writes it at `start`.
    fn upload(&self, pool: &GsplatScene, queue: &wgpu::Queue, range: Range, start: u32, palette: Option<usize>) {
        let (Some(chunk), Some(remap)) = (self.chunks.get(&range.file), self.remaps[range.file].as_ref()) else { return };
        let b = &chunk.bundle;
        let n = range.count as usize;
        let (first, last) = (range.offset as usize * 4, (range.offset as usize + n) * 4);
        let src = |name: &str| &b.textures[name].rgba[first..last.min(b.textures[name].rgba.len())];
        let (src_l, src_u, src_s, src_c, src_n) = (src("means_l"), src("means_u"), src("scales"), src("sh0"), src("sh_labels"));
        let shared = &self.shared;
        // Per axis: chunk value v = lo + span·q/65535 (log space), re-quantised against the shared range.
        let factor = [0, 1, 2].map(|c| {
            let span = (shared.means_maxs[c] - shared.means_mins[c]).max(f32::MIN_POSITIVE);
            ((remap.meta.means_maxs[c] - remap.meta.means_mins[c]) / span, (remap.meta.means_mins[c] - shared.means_mins[c]) / span * 65535.0)
        });
        let mut means_l = vec![0u8; n * 4];
        let mut means_u = vec![0u8; n * 4];
        let mut scales = vec![0u8; n * 4];
        let mut sh0 = vec![0u8; n * 4];
        let mut labels = vec![0u8; n * 4];
        let mut centres = vec![[0.0f32; 3]; n];
        let label_base = palette.map(|p| 64 + p as u32 * PALETTE_CENTROIDS);
        for i in 0..n.min(src_l.len() / 4) {
            let s = i * 4;
            for c in 0..3 {
                let q = f32::from(src_l[s + c]) + 256.0 * f32::from(src_u[s + c]);
                let v = (q * factor[c].0 + factor[c].1).round().clamp(0.0, 65535.0);
                let n16 = v as u32;
                means_l[s + c] = (n16 & 255) as u8;
                means_u[s + c] = (n16 >> 8) as u8;
                centres[i][c] = sog::unlog(shared.means_mins[c] + (shared.means_maxs[c] - shared.means_mins[c]) * v / 65535.0);
            }
            means_l[s + 3] = 255;
            means_u[s + 3] = 255;
            scales[s..s + 4].copy_from_slice(&[remap.scales[src_s[s] as usize], remap.scales[src_s[s + 1] as usize], remap.scales[src_s[s + 2] as usize], src_s[s + 3]]);
            sh0[s..s + 4].copy_from_slice(&[remap.sh0[src_c[s] as usize], remap.sh0[src_c[s + 1] as usize], remap.sh0[src_c[s + 2] as usize], src_c[s + 3]]);
            let label = label_base.map_or(0, |base| base + u32::from(src_n[s]) + 256 * u32::from(src_n[s + 1]));
            labels[s..s + 4].copy_from_slice(&[(label & 255) as u8, ((label >> 8) & 255) as u8, (label >> 16) as u8, 255]);
        }
        let quats = src("quats");
        pool.write_splats(
            queue,
            start,
            &[("means_l", &means_l), ("means_u", &means_u), ("quats", quats), ("scales", &scales), ("sh0", &sh0), ("sh_labels", &labels)],
            &centres,
        );
    }

    /// A palette slot holding `file`'s SH centroids, uploading them if needed. `None` when every slot is in use.
    fn palette_for(&mut self, file: usize, pool: &GsplatScene, queue: &wgpu::Queue) -> Option<usize> {
        if let Some(slot) = self.palettes.iter().position(|p| p.is_some_and(|(f, _)| f == file)) {
            if let Some((_, users)) = self.palettes[slot].as_mut() {
                *users += 1;
            }
            return Some(slot);
        }
        let (chunk, remap) = (self.chunks.get(&file)?, self.remaps[file].as_ref()?);
        if remap.meta.sh_n_count == 0 {
            // No spherical harmonics in this chunk: its labels point at the reserved zero centroid.
            return None;
        }
        let slot = self.palettes.iter().position(|p| p.is_none_or(|(_, users)| users == 0))?;
        let centroids = &chunk.bundle.textures["sh_centroids"];
        let rows = (centroids.height.min(PALETTE_ROWS)) as usize;
        let bytes = rows * centroids.width as usize * 4;
        let data: Vec<u8> = centroids.rgba[..bytes.min(centroids.rgba.len())]
            .chunks_exact(4)
            .flat_map(|t| [remap.sh_n[t[0] as usize], remap.sh_n[t[1] as usize], remap.sh_n[t[2] as usize], t[3]])
            .collect();
        pool.write_centroid_rows(queue, 1 + slot as u32 * PALETTE_ROWS, &data);
        self.palettes[slot] = Some((file, 1));
        Some(slot)
    }

    fn unuse_palette(&mut self, slot: usize) {
        if let Some((_, users)) = self.palettes[slot].as_mut() {
            *users = users.saturating_sub(1);
        }
    }

    /// First fit from the free list.
    fn allocate(&mut self, count: u32) -> Option<u32> {
        let index = self.free.iter().position(|&(_, len)| len >= count)?;
        let (start, len) = self.free[index];
        if len == count {
            self.free.remove(index);
        } else {
            self.free[index] = (start + count, len - count);
        }
        Some(start)
    }

    /// Takes exactly [start, start + count) out of the free list (it must be free).
    fn take_exact(&mut self, start: u32, count: u32) {
        let Some(index) = self.free.iter().position(|&(s, len)| s <= start && start + count <= s + len) else { return };
        let (s, len) = self.free.remove(index);
        let mut at = index;
        if s < start {
            self.free.insert(at, (s, start - s));
            at += 1;
        }
        if start + count < s + len {
            self.free.insert(at, (start + count, s + len - start - count));
        }
    }

    /// Returns a range to the free list, merging it with its neighbours.
    fn release(&mut self, start: u32, count: u32) {
        let at = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(at, (start, count));
        if at + 1 < self.free.len() && self.free[at].0 + self.free[at].1 == self.free[at + 1].0 {
            self.free[at].1 += self.free[at + 1].1;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].0 + self.free[at - 1].1 == self.free[at].0 {
            self.free[at - 1].1 += self.free[at].1;
            self.free.remove(at);
        }
    }

    /// One past the highest used slot.
    fn high_water(&self) -> u32 {
        match self.free.last() {
            Some(&(start, len)) if start + len == self.settings.capacity => start,
            _ => self.settings.capacity,
        }
    }

    /// Drops chunks no pending upload needs, keeping the `cache_chunks` most recently used.
    fn evict(&mut self) {
        let mut needed = BTreeSet::new();
        for (state, leaf) in self.leaves.iter().zip(&self.meta.leaves) {
            if let Some(want) = state.want.filter(|&w| state.have.is_none_or(|h| h.lod != w)) {
                if let Some(r) = leaf.lods[want] {
                    needed.insert(r.file);
                }
            }
        }
        needed.extend(self.prefetch().into_iter().map(|(_, file)| file));
        let mut idle: Vec<(u64, usize)> = self.chunks.iter().filter(|(f, _)| !needed.contains(f)).map(|(f, c)| (c.last_used, *f)).collect();
        idle.sort();
        let excess = idle.len().saturating_sub(self.settings.cache_chunks);
        for (_, file) in idle.into_iter().take(excess) {
            self.chunks.remove(&file);
        }
    }
}

/// How far a leaf's splats are from `p`, for choosing its detail: the distance to the nearest point of its box. A
/// long leaf (a facade down a street) is seen up close at its near end, so that end decides its detail; measuring to
/// its middle instead showed the near end of long walls coarse.
fn effective_distance(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    box_distance(p, min, max)
}

/// Distance from `p` to the box [min, max] (0 inside).
fn box_distance(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    (0..3).map(|i| (min[i] - p[i]).max(0.0).max(p[i] - max[i]).powi(2)).sum::<f32>().sqrt()
}

/// A leaf comes into view once it is within this many half-screens of the centre, and leaves it only past the wider
/// margin, so leaves at the edge of the view do not flip in and out as the camera sways. The entry margin matches the
/// depth-key pass's (`CULL_MARGIN` in `depth_keys.wgsl`): splats that far off screen are still drawn, as large ones
/// reach into the picture, so their leaves must count as seen.
const ENTER_MARGIN: f32 = 1.5;
const LEAVE_MARGIN: f32 = 1.8;
/// A leaf already showing the LOD it wants competes for the budget as if this much nearer, so leaves at the edge of
/// the budget keep their detail instead of trading it back and forth as the camera moves.
const STICKY: f32 = 0.7;
/// An automatic pool (`capacity` 0) holds this share of the level's LOD 0 splats, within these bounds. Measured on
/// the scans here: a pool of about 80 % of LOD 0 keeps the picture in front of the camera within about 0.1 % of full
/// detail (60 % left 0.4-0.5 % differing at 15-30 m on the denser scans).
const AUTO_POOL_SHARE: f64 = 0.8;
const AUTO_POOL_MIN: u32 = 3_000_000;
const AUTO_POOL_MAX: u32 = 8_000_000;

/// The share of the pool the selection may fill; the rest holds ranges while they crossfade.
const SELECT_SHARE: i64 = 85;

/// The view frustum's side planes, widened to `margin` half-screens, and its front (the plane through the camera),
/// as (a, b, c, d) with a·x + b·y + c·z + d ≥ 0 inside, from a column-major clip matrix.
fn frustum_planes(clip: &[f32; 16], margin: f32) -> [[f32; 4]; 5] {
    let row = |r: usize| [clip[r], clip[4 + r], clip[8 + r], clip[12 + r]];
    let (x, y, w) = (row(0), row(1), row(3));
    let plane = |s: f32, a: [f32; 4]| [0, 1, 2, 3].map(|k| margin * w[k] + s * a[k]);
    [plane(1.0, x), plane(-1.0, x), plane(1.0, y), plane(-1.0, y), w]
}

/// Whether the box [min, max] reaches inside every plane (it may still lie outside near a corner, which is fine).
fn box_in_frustum(planes: &[[f32; 4]; 5], min: [f32; 3], max: [f32; 3]) -> bool {
    planes.iter().all(|p| {
        // The box corner farthest along the plane's normal.
        let c = [0, 1, 2].map(|i| if p[i] >= 0.0 { max[i] } else { min[i] });
        p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] >= 0.0
    })
}

fn coarsest_available(lods: &[Option<Range>]) -> Option<usize> {
    (0..lods.len()).rev().find(|&l| lods[l].is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of `n` leaves along x, 10 m apart, each with two LODs in files 0 (fine) and 1 (coarse).
    fn tree(n: usize) -> LodMeta {
        let leaves = (0..n)
            .map(|i| crate::lod::Leaf {
                min: [i as f32 * 10.0, 0.0, 0.0],
                max: [i as f32 * 10.0 + 5.0, 1.0, 1.0],
                lods: vec![
                    Some(Range { file: 0, offset: i as u32 * 100, count: 100 }),
                    Some(Range { file: 1, offset: i as u32 * 10, count: 10 }),
                ],
            })
            .collect();
        LodMeta { counts: vec![100 * n as u64, 10 * n as u64], filenames: vec!["0_0/meta.json".into(), "1_0/meta.json".into()], leaves }
    }

    fn meta_json() -> Vec<u8> {
        let book: Vec<String> = (0..256).map(|i| format!("{}", i as f32 * 0.01)).collect();
        let book = book.join(",");
        format!(r#"{{"version":2,"count":1,"means":{{"mins":[-1,-1,-1],"maxs":[1,1,1]}},"scales":{{"codebook":[{book}]}},"sh0":{{"codebook":[{book}]}},"shN":{{"count":64,"codebook":[{book}]}}}}"#).into_bytes()
    }

    fn stream(capacity: u32) -> LevelStream {
        let settings = StreamSettings { capacity, lod_distance_m: 12.0, lod_error_px: 0.0, behind_bias: 0, ..StreamSettings::default() };
        LevelStream::new(tree(6), &[Some(meta_json()), Some(meta_json())], settings).unwrap()
    }

    #[test]
    fn near_leaves_want_lod_0_and_far_ones_coarser() {
        let mut s = stream(10_000);
        s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
        let wants: Vec<Option<usize>> = s.leaves.iter().map(|l| l.want).collect();
        assert_eq!(wants, [Some(0), Some(0), Some(1), Some(1), Some(1), Some(1)]);
    }

    #[test]
    fn a_small_pool_refines_the_nearest_leaves_first() {
        let mut s = stream(150);
        s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
        let total: u32 = s.leaves.iter().zip(&s.meta.leaves).map(|(st, l)| l.lods[st.want.unwrap()].unwrap().count).sum();
        assert!(total * 10 <= 150 * 9, "selection of {total} splats fits 150 slots");
        assert_eq!(s.leaves[0].want, Some(1), "nothing fits a 100-splat refinement after the coarse base of 60");
        let mut roomy = stream(400);
        roomy.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
        let wants: Vec<Option<usize>> = roomy.leaves.iter().map(|l| l.want).collect();
        assert_eq!(wants[..3], [Some(0), Some(0), Some(1)], "the two nearest get LOD 0, the rest stay coarse: {wants:?}");
    }

    /// A camera at the origin looking along +x (column-major projection · view, 90° field of view).
    fn looking_along_x() -> [f32; 16] {
        let view = crate::camera::look_at([0.0, 0.5, 0.5], [1.0, 0.5, 0.5], [0.0, 1.0, 0.0]);
        crate::camera::mul(&crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0), &view)
    }

    #[test]
    fn leaves_out_of_view_go_coarser_but_not_those_near_the_camera() {
        // Leaves at x = 0..55; the camera stands at x = 27 looking along +x. With LOD 0 out to 4 m, leaves within 8 m
        // count as in view whichever way the camera looks.
        let mut s = stream(10_000);
        s.settings.lod_distance_m = 4.0;
        s.settings.behind_bias = 1;
        let view = crate::camera::look_at([27.0, 0.5, 0.5], [28.0, 0.5, 0.5], [0.0, 1.0, 0.0]);
        let clip = crate::camera::mul(&crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0), &view);
        s.select([27.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&clip), 0.0);
        let visible: Vec<bool> = s.leaves.iter().map(|l| l.visible).collect();
        assert_eq!(visible, [false, false, true, true, true, true], "behind the camera, only the leaf 2 m away counts");
        assert_eq!(s.leaves[2].want, Some(0), "the near leaf behind the camera keeps LOD 0");
    }

    #[test]
    fn going_coarser_waits_the_keep_delay() {
        let mut s = stream(10_000);
        s.settings.keep_s = 1.0;
        s.select([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&looking_along_x()), 0.0);
        assert_eq!(s.leaves[1].want, Some(0));
        s.leaves[1].have = Some(Resident { lod: 0, start: 0, count: 100, palette: None, born: 0.0 });
        // Backing away to 30 m from it wants it a level coarser...
        let view = crate::camera::look_at([-20.0, 0.5, 0.5], [-19.0, 0.5, 0.5], [0.0, 1.0, 0.0]);
        let clip = crate::camera::mul(&crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0), &view);
        s.select([-20.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&clip), 0.5);
        assert_eq!(s.leaves[1].want, Some(0), "...but not for the first second");
        s.select([-20.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&clip), 1.6);
        assert_eq!(s.leaves[1].want, Some(1), "after it");
        // Coming back resets the delay.
        s.select([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&looking_along_x()), 1.7);
        assert_eq!(s.leaves[1].coarsen_since, None);
    }

    #[test]
    fn an_out_of_view_leaf_does_not_flip_between_two_levels() {
        // A leaf out of view just short of its band edge (22.4 m from a 12 m LOD distance is level 1.9) must stay
        // put, frame after frame, rather than flip between levels 2 and 3.
        let mut s = stream(10_000);
        s.settings.behind_bias = 1;
        s.settings.keep_s = 0.0;
        let back = crate::camera::look_at([-2.4, 0.5, 0.5], [-3.4, 0.5, 0.5], [0.0, 1.0, 0.0]);
        let clip = crate::camera::mul(&crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0), &back);
        let mut wants = Vec::new();
        for frame in 0..6 {
            s.select([-2.4, 0.5, 0.5], [-1.0, 0.0, 0.0], Some(&clip), frame as f32);
            let want = s.leaves[2].want.unwrap();
            s.leaves[2].have = Some(Resident { lod: want, start: 0, count: 10, palette: None, born: 0.0 });
            wants.push(want);
        }
        assert!(wants.windows(2).all(|w| w[0] == w[1]), "the leaf 22 m behind the camera settles: {wants:?}");
    }

    #[test]
    fn by_screen_space_error_a_denser_leaf_goes_coarser_at_the_same_distance() {
        // Two leaves the same size and distance; the second holds four times the splats at every LOD, so its splats
        // sit half as far apart and it can drop two levels for the same error.
        let mut meta = tree(2);
        meta.leaves[1].min = meta.leaves[0].min;
        meta.leaves[1].max = meta.leaves[0].max;
        for l in 0..2 {
            let r = meta.leaves[1].lods[l].as_mut().unwrap();
            r.count *= 4;
        }
        let settings = StreamSettings { capacity: 100_000, lod_error_px: 1.0, behind_bias: 0, ..StreamSettings::default() };
        let mut s = LevelStream::new(meta, &[Some(meta_json()), Some(meta_json())], settings).unwrap();
        s.set_focal_px(1000.0);
        // Leaf 0: 100 splats on 5 × 1 m, 0.22 m apart; at 150 m that is 1.5 px: LOD 0 is already too coarse.
        // Leaf 1: 0.11 m apart, 0.75 px: LOD 0 fits, LOD 1 (0.35 m, 2.4 px) does not.
        assert!(s.level_at(0, 150.0) < 0.5 && s.level_at(1, 150.0) >= 0.0);
        assert!(s.level_at(1, 300.0) > s.level_at(0, 300.0), "the denser leaf is allowed a coarser level");
        // Far enough away, both reach their coarsest.
        assert_eq!(s.level_at(0, 10_000.0), 1.0);
    }

    #[test]
    fn a_leaf_at_the_edge_of_the_budget_keeps_its_detail() {
        // Room for one refinement: leaves 1 and 2 are 10 and 20 m away... the camera moves so that leaf 2 is a little
        // nearer than leaf 1, but leaf 1 already has LOD 0 and keeps it.
        let mut s = stream(200);
        s.settings.lod_distance_m = 25.0;
        s.select([12.0, 0.5, 0.5], [1.0, 0.0, 0.0], None, 0.0);
        let first: Vec<usize> = (0..6).filter(|&i| s.leaves[i].want == Some(0)).collect();
        assert_eq!(first, [1], "the nearest leaf gets the one refinement");
        s.leaves[1].have = Some(Resident { lod: 0, start: 0, count: 100, palette: None, born: 0.0 });
        s.select([16.0, 0.5, 0.5], [1.0, 0.0, 0.0], None, 0.1);
        assert_eq!(s.leaves[1].want, Some(0), "1 m farther than leaf 2 now, but settled: it keeps LOD 0");
        assert_eq!(s.leaves[2].want, Some(1));
    }

    #[test]
    fn new_ranges_fade_in_while_replaced_ones_fade_out() {
        let mut s = stream(10_000);
        s.settings.fade_s = 0.4;
        s.select([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], None, 1.0);
        s.leaves[0].have = Some(Resident { lod: 0, start: 100, count: 100, palette: None, born: 0.9 });
        s.leaves[1].have = Some(Resident { lod: 0, start: 300, count: 100, palette: None, born: 0.0 });
        s.fading_out.push((Resident { lod: 1, start: 0, count: 10, palette: None, born: 0.0 }, 0.7));
        let fades = s.fades();
        assert_eq!(fades.len(), 2, "the settled leaf at slot 300 is not fading: {fades:?}");
        assert_eq!((fades[0].0, fades[0].1), (0, 10), "sorted by start");
        // A 0.4 s fade: the new range fades in over 0.2 s, then the old one out over the next 0.2 s.
        assert!((fades[0].2 - 0.5).abs() < 1e-5, "0.3 s after it was replaced, half the old range is left: {}", fades[0].2);
        assert!((fades[1].2 - 0.5).abs() < 1e-5, "0.1 s in, half the new range is drawn: {}", fades[1].2);
        s.now = 0.8;
        assert!((s.fades()[0].2 - 1.0).abs() < 1e-5, "the old range stays whole while the new one fades in");
        s.settings.fade_s = 0.0;
        assert!(s.fades().is_empty(), "no fading when it is off");
    }

    #[test]
    fn leaves_past_the_draw_distance_are_not_drawn() {
        let mut s = stream(10_000);
        s.settings.max_distance_m = 25.0;
        s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
        let shown = s.leaves.iter().filter(|l| l.want.is_some()).count();
        assert_eq!(shown, 3, "leaves at 0, 10 and 20 m are within 25 m; 30 m and beyond are hidden");
    }

    #[test]
    fn empty_leaves_request_their_coarsest_file_first() {
        let mut s = stream(10_000);
        s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
        let first = s.requests();
        assert_eq!(first[0].0, 1, "the coarse file comes first: {first:?}");
    }

    #[test]
    fn the_allocator_reuses_and_merges_freed_ranges() {
        let mut s = stream(1000);
        let a = s.allocate(100).unwrap();
        let b = s.allocate(200).unwrap();
        let c = s.allocate(50).unwrap();
        assert_eq!((a, b, c), (0, 100, 300));
        assert_eq!(s.high_water(), 350);
        s.release(b, 200);
        assert_eq!(s.allocate(150), Some(100), "first fit reuses the hole");
        s.release(100, 150);
        s.release(a, 100);
        s.release(c, 50);
        assert_eq!(s.free, vec![(0, 1000)], "everything merges back");
        assert_eq!(s.high_water(), 0);
        s.take_exact(200, 100);
        assert_eq!(s.free, vec![(0, 200), (300, 700)], "an exact range comes out of the middle");
        assert_eq!(s.allocate(250), Some(300));
    }
}
