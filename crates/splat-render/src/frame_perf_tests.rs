use super::*;
fn view(size: u32) -> View {
    View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 1.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(50f32.to_radians(), 1.0, 0.01, 100.0),
        width: size,
        height: size,
        near: 0.01,
        far: 100.0,
    }
}
#[test]
fn timestamp_queries_measure_render_work_and_pending_frames_are_bounded() {
    if crate::try_device().is_none() {
        return;
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame = pollster::block_on(Frame::new(&instance, None, (64, 64), false, None)).unwrap();
    let v = view(64);
    for i in 0..48 {
        if i % 16 < 8 {
            frame.set_overlay(Some((0, 0, 1, 1, &[255, 0, 0, 255])));
        } else {
            frame.set_overlay(None);
        }
        frame.render(&v);
        assert!(frame.pending_frames() <= 2);
        // Allow all states to reach the GPU, exercising both post and HUD timestamp endpoints.
        if i % 4 == 3 {
            frame.wait_idle();
        }
    }
    frame.wait_idle();
    assert_eq!(frame.pending_frames(), 0);
    if frame.gpu_timer.is_some() {
        assert!(
            frame.gpu_ms() > 0.0 && frame.gpu_ms().is_finite(),
            "timestamps must measure actual graphics passes: {}",
            frame.profile_json()
        );
    }
}
#[test]
fn bypassing_disabled_depth_of_field_preserves_the_image() {
    if crate::try_device().is_none() {
        return;
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame = pollster::block_on(Frame::new(&instance, None, (64, 64), false, None)).unwrap();
    let cube = crate::env::to_cubemap(&crate::env::procedural(64, 32, [0.4, 0.7, 0.5]), 16, 0.0);
    let sky = frame.build_sky(&cube);
    frame.set_sky(Some(sky), 1.0);
    let v = view(64);
    frame.set_post(PostSettings::default());
    let direct = pollster::block_on(frame.read_rgba(&v));
    frame.set_post(PostSettings {
        dof: 1.0,
        dof_radius_px: 8.0,
        dof_focus_m: 1e6,
        dof_range_m: 1e6,
        ..Default::default()
    });
    let copied = pollster::block_on(frame.read_rgba(&v));
    assert!(
        direct.chunks_exact(4).any(|p| p[0] > 10 && p[1] > 10),
        "fixture must contain color"
    );
    assert!(
        direct.iter().zip(copied).all(|(a, b)| a.abs_diff(b) <= 1),
        "an in-focus copy must match the bypass within sRGB rounding"
    );
}

#[test]
fn pass_profile_tracks_conditional_passes_reset_and_preserves_pixels() {
    if crate::try_device().is_none() {
        return;
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame = pollster::block_on(Frame::new(&instance, None, (64, 64), false, None)).unwrap();
    let v = view(64);
    frame.profile_sample_every(1);
    frame.set_overlay(Some((0, 0, 1, 1, &[255, 0, 0, 255])));
    frame.set_post(PostSettings {
        dof: 1.0,
        dof_radius_px: 8.0,
        ..Default::default()
    });
    let sampled = pollster::block_on(frame.read_rgba(&v));
    frame.wait_idle();
    let j = frame.profile_json();
    if frame.gpu_timer.is_some() {
        let passes = j["passes"].as_array().unwrap();
        for id in [
            "sky",
            "level_depth",
            "car_depth",
            "depth_of_field",
            "post",
            "hud",
        ] {
            assert!(passes.iter().any(|p| p["id"] == id), "missing {id}: {j}");
        }
        assert_eq!(j["readback_errors"], 0);
        for p in passes {
            if let Some(ms) = p["gpu_ms"].as_f64() {
                assert!(ms >= 0.0 && ms.is_finite());
            }
        }
        assert!(j["pass_aggregates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "post"));
    }
    frame.reset_profile();
    frame.profile_sample_every(600);
    let unsampled = pollster::block_on(frame.read_rgba(&v));
    assert_eq!(sampled, unsampled, "profiling must not change pixels");
    frame.profile_sample_every(1);
    frame.set_overlay(None);
    frame.set_post(PostSettings::default());
    pollster::block_on(frame.read_rgba(&v));
    frame.wait_idle();
    if frame.gpu_timer.is_some() {
        let j = frame.profile_json();
        let p = j["passes"].as_array().unwrap();
        assert!(!p
            .iter()
            .any(|p| p["id"] == "hud" || p["id"] == "depth_of_field"));
    }
    assert!(frame.profile_command("perf sample 0").unwrap().is_err());
    assert!(frame.profile_command("perf sample 1").unwrap().is_ok());
    assert_eq!(
        frame
            .profile_command("perf shaders")
            .unwrap()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        14
    );
}

#[test]
fn scene_toggles_skip_shader_work_clear_depth_and_restore_pixels() {
    if crate::try_device().is_none() {
        eprintln!("scene toggle GPU verification skipped: no adapter");
        return;
    }
    eprintln!("scene toggle GPU verification: adapter available");
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let fixture = crate::shadow_gpu_tests::floor();
    let mut frame = pollster::block_on(Frame::new(&instance, None, (64, 64), false, Some(&fixture))).unwrap();
    frame.set_level(&[fixture]).unwrap();
    frame.profile_sample_every(1);
    let cube = crate::env::to_cubemap(&crate::env::procedural(64, 32, [0.4, 0.7, 0.5]), 16, 0.0);
    let sky = frame.build_sky(&cube);
    frame.set_sky(Some(sky), 1.0);
    let v = view(64);
    let original = pollster::block_on(frame.read_rgba(&v));
    assert!(original.chunks_exact(4).any(|p| p[0] > 10));
    // Exercise each scene independently, including the clears that prevent hidden objects occluding others.
    for (level, car) in [(true, false), (false, true), (false, false)] {
        frame.set_visibility(false, level, car);
        let pixels = pollster::block_on(frame.read_rgba(&v));
        frame.wait_idle();
        if !level && !car {
            assert!(pixels.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]), "hidden scenes must leave no stale image");
        } else {
            assert!(pixels.chunks_exact(4).any(|p| p[0] > 10), "remaining scene must not be hidden by stale depth");
        }
        if frame.gpu_timer.is_some() {
            let j = frame.profile_json();
            let passes = j["passes"].as_array().unwrap();
            for (target, enabled) in [("car", car), ("level", level)] {
                assert_eq!(passes.iter().any(|p| p["id"] == format!("{target}_color")), enabled, "{j}");
                if !enabled {
                    assert!(!passes.iter().any(|p| p["id"] == "splat_sort" && p["target"] == target), "hidden scene must skip compute: {j}");
                    assert!(passes.iter().filter(|p| p["id"] == format!("{target}_depth")).all(|p| p["draw_calls"] == 0), "depth must clear without drawing: {j}");
                }
            }
            assert!(passes.iter().filter(|p| p["id"] == "sky").all(|p| p["draw_calls"] == 0));
        }
    }
    frame.set_visibility(true, true, true);
    assert_eq!(original, pollster::block_on(frame.read_rgba(&v)), "restoring toggles preserves GPU resources and pixels");
}
