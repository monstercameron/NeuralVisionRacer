use super::*;

#[test]
fn portal_rim_requires_foreground_preserves_car_and_bypasses_cleanly() {
    let Some((device, queue)) = crate::try_device() else {
        return;
    };
    let size = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("portal source"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let pixels = [128u8, 128, 128, 255].repeat(64 * 64);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(256),
            rows_per_image: Some(64),
        },
        size,
    );
    let source = texture.create_view(&Default::default());
    let (_, _, level, car) = targets(&device, 64, 64);
    let post = PostLayer::new(&device, &source, &car, &level);
    let (output, view) = post_target(&device, 64, 64);
    let projection = camera::perspective(90f32.to_radians(), 1.0, 0.1, 100.0);
    let render = |strength: f32, level_z: f32, car_z: f32, floor: f32, enabled: bool| {
        let mut uniforms = [0.0f32; 48];
        uniforms[5] = 1.0 / 64.0;
        uniforms[6] = 1.0 / 64.0;
        uniforms[22] = strength;
        uniforms[23] = 18.0;
        uniforms[32..36].copy_from_slice(&[0.0, 0.0, 0.5, if enabled { 10.0 } else { 0.0 }]);
        uniforms[36..40].copy_from_slice(&[1.12, 0.25, 0.5, floor]);
        uniforms[40..44].copy_from_slice(&[1.0, 1.0, projection[10], projection[14]]);
        uniforms[44..48].copy_from_slice(&[0.0, 1.0, 0.0, 0.0]);
        queue.write_buffer(&post.uniform, 0, &f32_bytes(&uniforms));
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grade pixels"),
            size: 16384,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        for (target, depth) in [(&car, car_z), (&level, level_z)] {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("grade test depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: target,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(depth),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("grade test"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&post.pipeline);
            pass.set_bind_group(0, &post.group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range().unwrap();
        mapped
            .chunks_exact(256)
            .flat_map(|row| row[..256].to_vec())
            .collect::<Vec<_>>()
    };
    let depth = |z: f32| -projection[10] + projection[14] / z;
    let base = render(0.0, depth(5.0), 1.0, -100.0, true);
    assert_eq!(base, pixels, "off must preserve input pixels");
    let rim = render(1.0, depth(5.0), 1.0, -100.0, true);
    let changed = base
        .chunks_exact(4)
        .zip(rim.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 80, "foreground must get a visible rim: {changed}");
    let centre = (32 * 64 + 32) * 4;
    assert_eq!(
        &base[centre..centre + 4],
        &rim[centre..centre + 4],
        "clear aperture centre"
    );
    assert_eq!(
        &base[..4],
        &rim[..4],
        "unrelated background stays untouched"
    );
    for (scene, car, floor, enabled) in [
        (1.0, 1.0, -100.0, true),
        (depth(20.0), 1.0, -100.0, true),
        (depth(5.0), depth(2.0), -100.0, true),
        (depth(5.0), 1.0, 100.0, true),
        (depth(5.0), 1.0, -100.0, false),
    ] {
        assert_eq!(
            base,
            render(1.0, scene, car, floor, enabled),
            "sky, distant scene, visible car, road and disabled cutouts stay unframed"
        );
    }
}

#[test]
fn pixel_aperture_clips_foreground_but_preserves_floor_and_distant_scene() {
    let Some((device, queue)) = crate::try_device() else {
        return;
    };
    let pipelines = scene::Pipelines::new(&device, COLOR_FORMAT).unwrap();
    let level = scene::build(
        &device,
        &queue,
        &pipelines,
        &crate::shadow_gpu_tests::floor(),
        0.5,
    )
    .unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 5.0, 0.01], [0.0; 3], [0.0, 0.0, -1.0]),
        projection: camera::perspective(65f32.to_radians(), 1.0, 0.01, 100.0),
        width: 128,
        height: 128,
        near: 0.01,
        far: 100.0,
    };
    let inverse = camera::inverse(&view.view).unwrap();
    let base = level.render_rgba(&device, &queue, &pipelines, &view, None);
    let render = |depth: f32, floor: f32| {
        let values = [
            0.0,
            0.0,
            0.15,
            depth,
            1.12,
            0.0,
            0.5,
            floor,
            1.0 / view.projection[0],
            1.0 / view.projection[5],
            1.0 / 128.0,
            1.0 / 128.0,
            inverse[1],
            inverse[5],
            inverse[9],
            inverse[13],
        ];
        level.set_uniform(&queue, "cutout_window", &f32_bytes(&values));
        level.render_rgba(&device, &queue, &pipelines, &view, None)
    };
    let window = render(10.0, -1.0);
    let centre = (64 * 128 + 64) * 4;
    assert!(base[centre] > 80, "fixture has foreground coverage");
    assert_eq!(
        &window[centre..centre + 3],
        &[0, 0, 0],
        "foreground is removed in aperture"
    );
    let outside = (64 * 128 + 100) * 4;
    assert_eq!(
        &base[outside..outside + 4],
        &window[outside..outside + 4],
        "outside is preserved"
    );
    assert_eq!(
        base,
        render(2.0, -1.0),
        "geometry behind target stays visible"
    );
    assert_eq!(base, render(10.0, 1.0), "preserved road stays visible");
    assert_eq!(
        base,
        render(0.0, -1.0),
        "disabled aperture restores image exactly"
    );
}
