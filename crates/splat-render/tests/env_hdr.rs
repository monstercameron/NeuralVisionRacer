//! The bundled test environment must load as HDR, keep its sun above the display range, and convert to a cubemap.

use splat_render::env;
use std::path::Path;

#[test]
fn bundled_hdri_loads_with_its_sun_above_display_range() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/hdri/studio_sun.hdr");
    let panorama = env::load_hdr(&path).expect("studio_sun.hdr loads");
    assert_eq!((panorama.width, panorama.height), (512, 256));
    let brightest = panorama.rgb.iter().map(|c| c[0].max(c[1]).max(c[2])).fold(0.0f32, f32::max);
    assert!(brightest > 10.0, "sun peak {brightest}");

    let cubemap = env::to_cubemap(&panorama, 32, 0.0);
    assert_eq!(cubemap.size, 32);
    let face_peak = cubemap.faces.iter().flatten().map(|c| c[0]).fold(0.0f32, f32::max);
    assert!(face_peak > 10.0, "cubemap lost the sun: {face_peak}");
}
