//! Standalone browser benchmark wrapper around the actual production SOG decoder.
//! This crate and its optional SIMD build are not linked into the game.
#[allow(dead_code)]
#[path = "../../../crates/splat-render/src/sog.rs"]
mod sog;
#[path = "../../../scripts/decode_remap_candidate.rs"]
mod remap_candidate;

pub struct Outcome {
    texture: Option<sog::Texture>,
    error: Vec<u8>,
}

#[no_mangle]
pub extern "C" fn reserve(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
}

#[no_mangle]
pub unsafe extern "C" fn release(ptr: *mut u8, len: usize) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

#[no_mangle]
pub unsafe extern "C" fn decode(stream: u32, ptr: *const u8, len: usize) -> *mut Outcome {
    let names = ["means_l", "means_u", "quats", "scales", "sh0", "sh_labels", "sh_centroids"];
    let name = names.get(stream as usize).copied().unwrap_or("invalid");
    let input = std::slice::from_raw_parts(ptr, len);
    let result = match sog::decode_stream(name, input) {
        Ok(texture) => Outcome { texture: Some(texture), error: Vec::new() },
        Err(error) => Outcome { texture: None, error: error.to_string().into_bytes() },
    };
    Box::into_raw(Box::new(result))
}

#[no_mangle]
pub unsafe extern "C" fn succeeded(out: *const Outcome) -> bool { (*out).texture.is_some() }
#[no_mangle]
pub unsafe extern "C" fn width(out: *const Outcome) -> u32 { (*out).texture.as_ref().map_or(0, |t| t.width) }
#[no_mangle]
pub unsafe extern "C" fn height(out: *const Outcome) -> u32 { (*out).texture.as_ref().map_or(0, |t| t.height) }
#[no_mangle]
pub unsafe extern "C" fn pixels(out: *const Outcome) -> *const u8 {
    (*out).texture.as_ref().map_or(std::ptr::null(), |t| t.rgba.as_ptr())
}
#[no_mangle]
pub unsafe extern "C" fn pixels_len(out: *const Outcome) -> usize { (*out).texture.as_ref().map_or(0, |t| t.rgba.len()) }
#[no_mangle]
pub unsafe extern "C" fn error(out: *const Outcome) -> *const u8 { (*out).error.as_ptr() }
#[no_mangle]
pub unsafe extern "C" fn error_len(out: *const Outcome) -> usize { (*out).error.len() }
#[no_mangle]
pub unsafe extern "C" fn release_outcome(out: *mut Outcome) { drop(Box::from_raw(out)); }

#[no_mangle]
pub extern "C" fn reserve_floats(len: usize) -> *mut f32 {
    Box::into_raw(vec![0.0f32; len].into_boxed_slice()) as *mut f32
}
#[no_mangle]
pub unsafe extern "C" fn release_floats(ptr: *mut f32, len: usize) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

#[no_mangle]
pub unsafe extern "C" fn merged_book(data: *const f32, len: usize, out: *mut f32) {
    let all = std::slice::from_raw_parts(data, len);
    let merged = sog::merged_codebook(std::iter::once(all));
    std::ptr::copy_nonoverlapping(merged.as_ptr(), out, 256);
}

#[no_mangle]
pub unsafe extern "C" fn reserve_zero_book(data: *mut f32) {
    let values = std::slice::from_raw_parts_mut(data, 256);
    let zero = (0..256).min_by(|&a, &b| values[a].abs().total_cmp(&values[b].abs())).unwrap_or(0);
    values[zero] = 0.0;
}

#[no_mangle]
pub unsafe extern "C" fn remap_one(variant: u32, from: *const f32, from_len: usize, to: *const f32, to_len: usize, out: *mut u8) {
    let from = std::slice::from_raw_parts(from, from_len);
    let to = std::slice::from_raw_parts(to, to_len);
    let map = if variant == 0 { sog::remap(from, to) } else { remap_candidate::candidate(from, to) };
    std::ptr::copy_nonoverlapping(map.as_ptr(), out, map.len());
}

/// The same three codebook maps per chunk as LevelStream::new, repeated for measurable samples.
#[no_mangle]
pub unsafe extern "C" fn remap_batch(variant: u32, books: *const f32, chunks: usize, targets: *const f32, repetitions: usize, out: *mut u8) {
    let books = std::slice::from_raw_parts(books, chunks * 3 * 256);
    let targets = std::slice::from_raw_parts(targets, 3 * 256);
    let out = std::slice::from_raw_parts_mut(out, chunks * 3 * 256);
    for _ in 0..repetitions {
        for i in 0..chunks * 3 {
            let from = std::hint::black_box(&books[i * 256..(i + 1) * 256]);
            let which = i % 3;
            let to = std::hint::black_box(&targets[which * 256..(which + 1) * 256]);
            let map = if variant == 0 { sog::remap(from, to) } else { remap_candidate::candidate(from, to) };
            out[i * 256..(i + 1) * 256].copy_from_slice(&map);
        }
    }
}
