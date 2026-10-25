//! Measures temporal flicker in a streamed level while the camera drives forward, frame by frame.
//!
//! `cargo run --release --example flicker_probe -- <level dir> <eye xyz> <target xyz> [out dir]`
//!
//! Coordinates are world space with the game's level turn (180° about z). The camera moves along eye → target
//! (flattened) at `SPEED` m/s for `FRAMES` frames at 60 Hz, streaming as the game does (chunks load synchronously).
//! Each frame's flicker is the share of pixels that differ from the average of the frames either side by more than
//! 24 levels in some channel: smooth motion barely moves that average, while something that pops and pops back
//! (a splat toggling, a blur radius jumping) does. Prints each frame's flicker with the crossfades running, then the
//! mean and the worst. With an out dir, every 10th frame is saved as a PNG.
//!
//! Env: SPEED (15), FRAMES (90), SIZE (640x360), DOF (0: off, else its strength), FADE (crossfade seconds), LOD_M,
//! CAPACITY, GRAIN (0). FREEZE=1 keeps the level of detail chosen at the start for the whole drive (a baseline:
//! whatever flicker remains is motion, not streaming). SAVE=a-b saves frames a to b (with an out dir).
//! COVER=1 also renders every frame coloured by level of detail and by distance and reports, per frame, the share of
//! the picture showing LOD 2 or coarser nearer than the sharp distance (1.5 × LOD_M) and LOD 1 or coarser nearer
//! than LOD_M: the rendered check that nothing coarse is seen in front. Tinted frames are saved with the others.
//! REF=1 also streams the level a second time with every leaf at full detail (a pool of REF_CAP splats, default 12M)
//! and compares each frame with it: the share of pixels that differ visibly (more than 24 levels) and the mean
//! difference, the direct measure of what the levels of detail cost the picture. ERR_PX sets the error target.

use splat_render::frame::{Frame, PostSettings};
use splat_render::lod::LodMeta;
use splat_render::{camera, sog};
use std::path::Path;

fn vec3(args: &[String]) -> [f32; 3] {
    [0, 1, 2].map(|i| args[i].parse().expect("coordinate"))
}

fn env<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 7 {
        eprintln!("usage: flicker_probe <level dir> <eye xyz> <target xyz> [out dir]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[0]);
    let (mut eye, mut target) = (vec3(&args[1..4]), vec3(&args[4..7]));
    let out = args.get(7).map(Path::new);
    let (width, height): (u32, u32) = std::env::var("SIZE").ok().and_then(|s| { let (w, h) = s.split_once('x')?; Some((w.parse().ok()?, h.parse().ok()?)) }).unwrap_or((640, 360));
    let (speed, frames) = (env("SPEED", 15.0f32), env("FRAMES", 90usize));

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame = pollster::block_on(Frame::new(&instance, None, (width, height), false, None)).expect("frame");
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).expect("lod-meta.json")).expect("parse");
    let metas: Vec<Option<Vec<u8>>> = meta.filenames.iter().map(|f| std::fs::read(dir.join(f)).ok()).collect();
    let defaults = splat_render::stream::StreamSettings::default();
    let settings = splat_render::stream::StreamSettings {
        capacity: env("CAPACITY", 0u32),
        lod_distance_m: env("LOD_M", defaults.lod_distance_m),
        fade_s: env("FADE", defaults.fade_s),
        lod_error_px: env("ERR_PX", defaults.lod_error_px),
        uploads_per_frame: env("UPLOADS", defaults.uploads_per_frame),
        any_view_m: env("ANY_VIEW", defaults.any_view_m),
        behind_bias: env("BEHIND", defaults.behind_bias),
        sh_lods: env("SH_LODS", defaults.sh_lods),
        ..defaults
    };
    let mut stream = splat_render::stream::LevelStream::new(meta.clone(), &metas, settings).expect("stream");
    // REF=1: a second frame streaming every leaf at full detail, for comparison.
    let reference = std::env::var("REF").is_ok_and(|v| v == "1");
    let mut ref_parts = reference.then(|| {
        let mut f = pollster::block_on(Frame::new(&instance, None, (width, height), false, None)).expect("reference frame");
        let ref_settings = splat_render::stream::StreamSettings { capacity: env("REF_CAP", 12_000_000u32), lod_error_px: 0.01, keep_s: 0.0, fade_s: 0.0, uploads_per_frame: 20_000_000, max_in_flight: 32, ..settings };
        let mut st = splat_render::stream::LevelStream::new(meta.clone(), &metas, ref_settings).expect("reference stream");
        f.start_level_stream(&mut st).expect("reference pool");
        (f, st)
    });
    frame.start_level_stream(&mut stream).expect("pool");
    // The game's level turn: 180° about z.
    frame.set_level_model(camera::mul(&camera::rotation_x(std::f32::consts::PI), &[-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]));
    if let Some((f, _)) = ref_parts.as_mut() {
        f.set_level_model(camera::mul(&camera::rotation_x(std::f32::consts::PI), &[-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]));
    }
    frame.set_post(PostSettings {
        dof: env("DOF", 0.0),
        dof_focus_m: 0.0,
        dof_range_m: 25.0,
        dof_radius_px: 8.0,
        grain: env("GRAIN", 0.0),
        ..PostSettings::default()
    });

    let view_at = |eye: [f32; 3], target: [f32; 3]| splat_render::scene::View {
        model: camera::IDENTITY,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), width as f32 / height as f32, 0.05, 2000.0),
        width,
        height,
        near: 0.05,
        far: 2000.0,
    };
    // FOCUS_AHEAD=m centres the detail that far ahead of the camera, where the game's car is (0: on the camera).
    let focus_ahead: f32 = env("FOCUS_AHEAD", 6.0);
    let step = |frame: &mut Frame, stream: &mut splat_render::stream::LevelStream, view: &splat_render::scene::View, t: f32| {
        if focus_ahead > 0.0 {
            let inv = camera::inverse(&view.view).expect("view");
            let (eye, fwd) = ([inv[12], inv[13], inv[14]], [-inv[8], -inv[9], -inv[10]]);
            frame.set_lod_focus(Some([0, 1, 2].map(|i| eye[i] + fwd[i] * focus_ahead)));
        }
        frame.stream_level(stream, view, t);
        if let Err(e) = stream.check_pool() {
            println!("POOL PROBLEM at {t:.2}s: {e}");
        }
        for (file, chunk) in stream.requests() {
            match sog::load_dir(&dir.join(chunk)) {
                Ok(bundle) => stream.insert(file, bundle),
                Err(_) => stream.failed(file),
            }
        }
    };
    // Settle at the start, so the run measures driving, not the first load.
    for i in 0..240 {
        step(&mut frame, &mut stream, &view_at(eye, target), i as f32 / 60.0);
        if let Some((f, st)) = ref_parts.as_mut() {
            step(f, st, &view_at(eye, target), i as f32 / 60.0);
        }
    }
    let along = {
        let d = [target[0] - eye[0], 0.0, target[2] - eye[2]];
        let n = (d[0] * d[0] + d[2] * d[2]).sqrt().max(1e-6);
        [d[0] / n, 0.0, d[2] / n]
    };
    let freeze = std::env::var("FREEZE").is_ok_and(|v| v == "1");
    let save = std::env::var("SAVE").ok().and_then(|r| {
        let (a, b) = r.split_once('-')?;
        Some(a.parse::<usize>().ok()?..=b.parse::<usize>().ok()?)
    });
    // FADECHECK=1: jump 15 m ahead, then hold still and compare frames during the fade-in (does the fade show?).
    if std::env::var("FADECHECK").is_ok() {
        for i in 0..3 {
            eye[i] += along[i] * 15.0;
            target[i] += along[i] * 15.0;
        }
        let view = view_at(eye, target);
        let mut shots = Vec::new();
        for (k, t) in [4.0f32, 4.1, 4.5, 4.9, 5.5, 6.5, 8.0].into_iter().enumerate() {
            step(&mut frame, &mut stream, &view, t);
            let st = stream.stats();
            frame.set_lod_debug(std::env::var("BOXES").is_ok().then_some(&stream));
            let px = pollster::block_on(frame.read_rgba(&view));
            if let Some(out) = out {
                std::fs::create_dir_all(out).ok();
                image::RgbaImage::from_raw(width, height, px.clone()).expect("size").save(out.join(format!("fade_{k}.png"))).ok();
            }
            let fades = stream.fades();
            let shares: Vec<String> = fades.iter().take(6).map(|f| format!("{}..{}={:.2}", f.0, f.1, f.2)).collect();
            let (zero, one) = (fades.iter().filter(|f| f.2 <= 0.0).count(), fades.iter().filter(|f| f.2 >= 1.0).count());
            println!("t {t:.1}: {} fading ({zero} at 0, {one} at 1), switches {}, palettes {}, missing SH {}, e.g. {}", st.fading, st.switches, st.palettes_in_use, st.missing_sh, shares.iter().take(2).cloned().collect::<Vec<_>>().join(" "));
            shots.push(px);
        }
        let recent: Vec<Vec<f64>> = serde_json::from_str(&stream.recent_switches_json()).unwrap_or_default();
        {
            let level_model = camera::mul(&camera::rotation_x(std::f32::consts::PI), &[-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
            let to_level = camera::inverse(&level_model).unwrap();
            let e = camera::transform_point(&to_level, eye);
            let t = camera::transform_point(&to_level, target);
            println!("  eye in splat space {e:?}, looking towards {t:?}");
            // For every leaf in the late wave: how far its box reaches in front of the camera, and sideways.
            let fwd = { let d = [t[0] - e[0], t[1] - e[1], t[2] - e[2]]; let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(); d.map(|c| c / n) };
            for r in recent.iter().filter(|r| r[0] > 5.0) {
                let l = &stream.lod_meta().leaves[r[1] as usize];
                let mut ahead = f32::MIN;
                for k in 0..8 {
                    let c = [0, 1, 2].map(|i| if k >> i & 1 == 1 { l.max[i] } else { l.min[i] });
                    ahead = ahead.max((0..3).map(|i| (c[i] - e[i]) * fwd[i]).sum::<f32>());
                }
                let size = [0, 1, 2].map(|i| (l.max[i] - l.min[i]).round());
                if ahead > 0.0 {
                    println!("  wave leaf {} reaches {ahead:.1} m ahead of the camera, box {size:?}, d {:.1}", r[1], r[4]);
                }
            }
            let near = stream.nearest_leaves_json(5000).replace("Some(", "").replace(")", "").replace("None", "null");
            let rows: Vec<serde_json::Value> = serde_json::from_str(&near).unwrap_or_default();
            for id in [557usize, 736, 561] {
                let l = &stream.lod_meta().leaves[id];
                // Sample the box and project with the probe's own view: how many sample points land on screen?
                let v = view_at(eye, target);
                let clip = camera::mul(&v.projection, &camera::mul(&v.view, &level_model));
                let mut on = 0;
                let n: i32 = 6;
                for a in 0..=n { for b in 0..=n { for c in 0..=n {
                    let p = [a, b, c].iter().enumerate().map(|(i, &k)| l.min[i] + (l.max[i] - l.min[i]) * k as f32 / n as f32).collect::<Vec<_>>();
                    let x = clip[0] * p[0] + clip[4] * p[1] + clip[8] * p[2] + clip[12];
                    let y = clip[1] * p[0] + clip[5] * p[1] + clip[9] * p[2] + clip[13];
                    let w = clip[3] * p[0] + clip[7] * p[1] + clip[11] * p[2] + clip[15];
                    if w > 0.0 && x.abs() <= w && y.abs() <= w { on += 1; }
                }}}
                println!("  leaf {id}: box {:?}..{:?}, {on} of {} sample points on screen", l.min, l.max, (n + 1).pow(3));
            }
            for r in rows.iter().filter(|r| r["have"] == 0 && r["sh"] == false) {
                println!("  LOD 0 without SH: leaf {} at {} m, in view {}", r["leaf"], r["d"], r["visible"]);
            }
            for id in [557, 736, 729, 728, 561] {
                if let Some(r) = rows.iter().find(|r| r["leaf"] == id) {
                    println!("  leaf {id} now: {r}");
                }
            }
            for &leaf in &[] as &[usize] {
                let l = stream.lod_meta().leaves[leaf].clone();
                println!("  leaf {leaf}: box min {:?} max {:?}", l.min, l.max);
                for (lod, range) in l.lods.iter().enumerate() {
                    let Some(r) = range else { continue };
                    let chunk = stream.lod_meta().dir_of(r.file).unwrap_or_default().to_string();
                    let Ok(bundle) = sog::load_dir(&dir.join(&chunk)) else { continue };
                    let centres = sog::centres(&bundle);
                    let part = &centres[r.offset as usize..(r.offset + r.count) as usize];
                    let mut lo = [f32::MAX; 3];
                    let mut hi = [f32::MIN; 3];
                    let mut sorted: Vec<Vec<f32>> = vec![Vec::new(); 3];
                    for c in part {
                        for k in 0..3 {
                            lo[k] = lo[k].min(c[k]);
                            hi[k] = hi[k].max(c[k]);
                            sorted[k].push(c[k]);
                        }
                    }
                    let med: Vec<f32> = sorted.iter_mut().map(|v| { v.sort_by(f32::total_cmp); v[v.len() / 2] }).collect();
                    println!("    LOD {lod} ({chunk}, {} splats): centres min {lo:?} max {hi:?} median {med:?}", r.count);
                }
            }
        }
        for r in recent.iter().filter(|r| r[0] > 5.0) {
            println!("  switch at {:.1}s: leaf {} LOD {} -> {}, {:.1} m, in view {}", r[0], r[1], r[2], r[3], r[4], r[5]);
        }
        for k in 1..shots.len() {
            let diff: f64 = shots[k].iter().zip(&shots[k - 1]).map(|(a, b)| (f64::from(*a) - f64::from(*b)).abs()).sum::<f64>() / shots[k].len() as f64;
            println!("shot {} vs {}: mean difference {diff:.2} levels", k - 1, k);
        }
        return;
    }
    let cover = std::env::var("COVER").is_ok_and(|v| v == "1");
    let lod_m = env("LOD_M", defaults.lod_distance_m);
    let mut worst_cover = (0.0f32, 0.0f32, 0usize);
    let (mut ref_worst, mut ref_sum, mut ref_mean_worst) = (0.0f32, 0.0f32, 0.0f32);
    let mut ref_bands = [0.0f32; 4];
    let mut images: Vec<Vec<u8>> = Vec::new();
    let mut scores = Vec::new();
    for f in 0..frames {
        let t = 4.0 + f as f32 / 60.0;
        for i in 0..3 {
            eye[i] += along[i] * speed / 60.0;
            target[i] += along[i] * speed / 60.0;
        }
        let view = view_at(eye, target);
        if !freeze {
            step(&mut frame, &mut stream, &view, t);
        }
        let st = stream.stats();
        let fading = format!("{} fading, scale {:.2}, switches {}, fades cut {}, hard swaps {}, uploaded {}", st.fading, st.detail_scale, st.switches, st.fades_cut, st.hard_swaps, st.resident_splats);
        let pixels = pollster::block_on(frame.read_rgba(&view));
        if let Some(out) = out.filter(|_| save.as_ref().map_or(f % 10 == 0, |r| r.contains(&f))) {
            std::fs::create_dir_all(out).ok();
            image::RgbaImage::from_raw(width, height, pixels.clone()).expect("size").save(out.join(format!("frame_{f:03}.png"))).ok();
        }
        if cover {
            frame.set_debug_colour(1);
            let lods = pollster::block_on(frame.read_rgba(&view));
            frame.set_debug_colour(2);
            let dist = pollster::block_on(frame.read_rgba(&view));
            frame.set_debug_colour(0);
            let to_linear = |v: u8| { let c = f32::from(v) / 255.0; if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } };
            // The splat shader decodes its colour from gamma 2.2 before blending (`prepareOutputFromGamma`), so an
            // encoded value comes back as its 2.2th power: undo both the sRGB target and that.
            let value = |v: u8| to_linear(v).powf(1.0 / 2.2);
            // SHARP_M overrides the threshold (a control: with a small LOD_M, coarse leaves inside it must be caught).
            let sharp = env("SHARP_M", lod_m * stream.stats().detail_scale * 1.5);
            let near = lod_m * stream.stats().detail_scale;
            // Each pixel's level (from the tint) and distance; a pixel counts as coarse in front only if every pixel
            // within 2 of it is too, so the blend of a near surface's edge with far coarse background behind it
            // (silhouettes against distant buildings or the sky) is not mistaken for a coarse tile.
            let (w, h) = (width as usize, height as usize);
            let mut flags = vec![(false, false, false); w * h]; // (seen, LOD 2+ within sharp, LOD 1+ within near)
            for (k, flag) in flags.iter_mut().enumerate() {
                let p = k * 4;
                let (r, g) = (lods[p], lods[p + 1]);
                if r < 40 && g < 40 {
                    continue; // nothing streamed here (sky)
                }
                // The tint colours as read back: green (26, 255, 51), yellow (255, 242, 26), orange (255, 128, 13), red (255, 26, 26).
                let lod = if r < 170 { 0 } else if g > 200 { 1 } else if g > 70 { 2 } else { 3 };
                let d = value(dist[p]) * 128.0;
                *flag = (true, lod >= 2 && d < sharp, lod >= 1 && d < near);
            }
            let solid = |x: usize, y: usize, which: fn(&(bool, bool, bool)) -> bool| {
                (y.saturating_sub(2)..(y + 3).min(h)).all(|yy| (x.saturating_sub(2)..(x + 3).min(w)).all(|xx| which(&flags[yy * w + xx])))
            };
            let (mut coarse_front, mut soft_near, mut seen) = (0usize, 0usize, 0usize);
            let mut mask = vec![0u8; lods.len()];
            for y in 0..h {
                for x in 0..w {
                    let k = y * w + x;
                    seen += usize::from(flags[k].0);
                    let coarse = flags[k].1 && solid(x, y, |f| f.1);
                    coarse_front += usize::from(coarse);
                    soft_near += usize::from(flags[k].2 && solid(x, y, |f| f.2));
                    let p = k * 4;
                    let c = if coarse { [255, 0, 255, 255] } else { [pixels[p] / 3, pixels[p + 1] / 3, pixels[p + 2] / 3, 255] };
                    mask[p..p + 4].copy_from_slice(&c);
                }
            }
            let total = (width * height) as f32 / 100.0;
            let (a, b) = (coarse_front as f32 / total, soft_near as f32 / total);
            if a > worst_cover.0 {
                worst_cover = (a, b, f);
            }
            worst_cover.1 = worst_cover.1.max(b);
            if f == 0 {
                let near_json = stream.nearest_leaves_json(3000).replace("Some(", "").replace(")", "").replace("None", "null");
                let rows: Vec<serde_json::Value> = serde_json::from_str(&near_json).unwrap_or_default();
                for r in rows.iter().filter(|r| r["d"].as_f64().unwrap_or(99.0) < 30.0 && r["have"].as_u64().unwrap_or(0) >= 1) {
                    println!("  coarse leaf near the camera: {r}");
                }
            }
            println!("frame {f:3}: LOD 2+ within {sharp:.0} m: {a:.2}% of the picture, LOD 1+ within {near:.0} m: {b:.2}% ({:.0}% of pixels show the level)", seen as f32 / total);
            if let Some(out) = out.filter(|_| f % 15 == 0) {
                std::fs::create_dir_all(out).ok();
                image::RgbaImage::from_raw(width, height, lods).expect("size").save(out.join(format!("lod_{f:03}.png"))).ok();
                image::RgbaImage::from_raw(width, height, mask).expect("size").save(out.join(format!("coarse_{f:03}.png"))).ok();
            }
        }
        if std::env::var("BUDGET").is_ok() && f == 30 {
            let near_json = stream.nearest_leaves_json(5000).replace("Some(", "").replace(")", "").replace("None", "null");
            let rows: Vec<serde_json::Value> = serde_json::from_str(&near_json).unwrap_or_default();
            let mut bands: std::collections::BTreeMap<String, (usize, u64, [usize; 5])> = Default::default();
            for r in &rows {
                let Some(have) = r["have"].as_u64() else { continue };
                let d = r["d"].as_f64().unwrap_or(0.0);
                let band = if d < 12.0 { "00-12" } else if d < 24.0 { "12-24" } else if d < 48.0 { "24-48" } else if d < 96.0 { "48-96" } else { "96+" };
                let key = format!("{band} {}", if r["visible"] == true { "view" } else { "hidden" });
                let e = bands.entry(key).or_default();
                e.0 += 1;
                e.1 += r["counts"][have as usize].as_u64().unwrap_or(0);
                e.2[(have as usize).min(4)] += 1;
            }
            let st = stream.stats();
            println!("budget at frame 30: resident {} of {}, scale {:.2}", st.resident_splats, stream.settings.capacity, st.detail_scale);
            for (k, (n, splats, lods)) in bands {
                println!("  {k}: {n} leaves, {}k splats, by LOD {lods:?}", splats / 1000);
            }
        }
        if let Some((f2, st2)) = ref_parts.as_mut() {
            step(f2, st2, &view, t);
            let reference_px = pollster::block_on(f2.read_rgba(&view));
            // The reference's distances (debug colour 2: red = metres / 128, after the gamma the shader applies).
            f2.set_debug_colour(2);
            let ref_dist = pollster::block_on(f2.read_rgba(&view));
            f2.set_debug_colour(0);
            let metres = |v: u8| {
                let c = f32::from(v) / 255.0;
                let lin = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
                lin.powf(1.0 / 2.2) * 128.0
            };
            let (mut off, mut total) = (0usize, 0.0f64);
            let mut by_band = [0usize; 4];
            for p in (0..pixels.len()).step_by(4) {
                let d = (0..3).map(|k| (f32::from(pixels[p + k]) - f32::from(reference_px[p + k])).abs()).fold(0.0, f32::max);
                if d > 24.0 {
                    off += 1;
                    let m = metres(ref_dist[p]);
                    by_band[if m < 15.0 { 0 } else if m < 30.0 { 1 } else if m < 60.0 { 2 } else { 3 }] += 1;
                }
                total += f64::from(d);
            }
            let pct = |n: usize| n as f32 / (width * height) as f32 * 100.0;
            for (b, n) in by_band.iter().enumerate() {
                ref_bands[b] = ref_bands[b].max(pct(*n));
            }
            let share = off as f32 / (width * height) as f32 * 100.0;
            let mean = (total / f64::from(width * height)) as f32;
            ref_worst = ref_worst.max(share);
            ref_mean_worst = ref_mean_worst.max(mean);
            ref_sum += share;
            println!("frame {f:3}: vs full detail: {share:.2}% of pixels differ visibly (under 15 m {:.2}%, 15-30 m {:.2}%), mean difference {mean:.2} levels", pct(by_band[0]), pct(by_band[1]));
            if let Some(out) = out.filter(|_| save.as_ref().map_or(f % 15 == 0, |r| r.contains(&f))) {
                std::fs::create_dir_all(out).ok();
                image::RgbaImage::from_raw(width, height, reference_px).expect("size").save(out.join(format!("reference_{f:03}.png"))).ok();
            }
        }
        images.push(pixels);
        if images.len() == 3 {
            let (a, b, c) = (&images[0], &images[1], &images[2]);
            let mut flicker = 0usize;
            for p in (0..b.len()).step_by(4) {
                let off = (0..3).any(|k| (f32::from(b[p + k]) - (f32::from(a[p + k]) + f32::from(c[p + k])) / 2.0).abs() > 24.0);
                flicker += usize::from(off);
            }
            let share = flicker as f32 / (width * height) as f32 * 100.0;
            scores.push(share);
            println!("frame {f:3}: flicker {share:5.2}% of pixels, {fading}");
            images.remove(0);
        }
    }
    let mean = scores.iter().sum::<f32>() / scores.len().max(1) as f32;
    let worst = scores.iter().copied().fold(0.0, f32::max);
    println!("flicker: mean {mean:.2}%, worst {worst:.2}% of pixels per frame");
    if ref_parts.is_some() {
        println!("vs full detail: worst {ref_worst:.2}% of pixels differ visibly (mean over the drive {:.2}%), worst mean difference {ref_mean_worst:.2} levels", ref_sum / frames.max(1) as f32);
        println!("  by distance, worst frame: under 15 m {:.2}%, 15-30 m {:.2}%, 30-60 m {:.2}%, beyond 60 m {:.2}%", ref_bands[0], ref_bands[1], ref_bands[2], ref_bands[3]);
    }
    if cover {
        println!("coarse in front: worst {:.2}% of the picture (frame {}); LOD 1+ near: worst {:.2}%", worst_cover.0, worst_cover.2, worst_cover.1);
    }
}
