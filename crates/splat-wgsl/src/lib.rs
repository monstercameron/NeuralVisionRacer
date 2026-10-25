//! WGSL sources from the PlayCanvas engine's Gaussian-splat pipeline, plus a preprocessor.
//!
//! Source: PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
//! See `NOTICE.md` in this crate for licence and provenance.

pub mod binding;
pub mod declarations;
pub mod pipeline;
pub mod preprocess;

include!(concat!(env!("OUT_DIR"), "/chunks.rs"));

/// Returns the raw (unprocessed) WGSL for an include key such as `gsplatCenterVS`.
pub fn chunk_by_key(key: &str) -> Option<&'static str> {
    let (_, path) = KEYS.iter().find(|(k, _)| *k == key)?;
    chunk_by_path(path)
}

/// Returns the raw WGSL for a chunk path relative to `shaders/`, e.g. `gsplat/vert/gsplatCenter.wgsl`.
pub fn chunk_by_path(path: &str) -> Option<&'static str> {
    CHUNKS.iter().find(|(p, _)| *p == path).map(|(_, s)| *s)
}
