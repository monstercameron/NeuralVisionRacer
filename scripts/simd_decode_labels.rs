//! Isolated WASM experiment for the SOG label-blue clearing loop.
//! The scalar and SIMD versions are built as separate modules; this file is not
//! linked into the game. `len` may include an incomplete trailing pixel.

#[no_mangle]
pub extern "C" fn reserve(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
}

#[no_mangle]
pub unsafe extern "C" fn release(ptr: *mut u8, len: usize) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

#[cfg(not(simd_variant))]
#[no_mangle]
pub unsafe extern "C" fn normalize(ptr: *mut u8, len: usize) {
    let pixels = std::slice::from_raw_parts_mut(ptr, len);
    pixels.chunks_exact_mut(4).for_each(|pixel| pixel[2] = 0);
}

#[cfg(simd_variant)]
#[target_feature(enable = "simd128")]
#[no_mangle]
pub unsafe extern "C" fn normalize(ptr: *mut u8, len: usize) {
    use std::arch::wasm32::{i32x4_splat, v128, v128_and, v128_load, v128_store};

    let mask = i32x4_splat(0xff00_ffffu32 as i32);
    let mut offset = 0;
    while len - offset >= 16 {
        let p = ptr.add(offset) as *mut v128;
        v128_store(p, v128_and(v128_load(p), mask));
        offset += 16;
    }
    std::slice::from_raw_parts_mut(ptr.add(offset), len - offset)
        .chunks_exact_mut(4)
        .for_each(|pixel| pixel[2] = 0);
}
