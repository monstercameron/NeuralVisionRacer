use super::*;

#[test]
fn car_sharpening_respects_background_and_level_occlusion() {
    let Some((device, queue)) = crate::try_device() else {
        return;
    };
    let size = wgpu::Extent3d {
        width: 16,
        height: 16,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("clarity source"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let pixels: Vec<u8> = (0..256)
        .flat_map(|i| {
            let value = if i % 4 < 2 { 110 } else { 145 };
            [value, value, value, 255]
        })
        .collect();
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
            bytes_per_row: Some(64),
            rows_per_image: Some(16),
        },
        size,
    );
    let source = texture.create_view(&Default::default());
    let (_, _, level, car) = targets(&device, 16, 16);
    let post = PostLayer::new(&device, &source, &car, &level);
    let (output, view) = post_target(&device, 16, 16);
    let protect_region = std::cell::Cell::new(false);
    let render = |car_z: f32, level_z: f32, sharpen: f32, blur: f32| {
        let mut uniforms = [0.0f32; 32];
        uniforms[0] = blur;
        uniforms[11] = 0.35;
        uniforms[15] = 0.5;
        uniforms[5] = 1.0 / 16.0;
        uniforms[6] = 1.0 / 16.0;
        uniforms[20] = sharpen;
        uniforms[21] = sharpen * 0.25;
        if protect_region.get() {uniforms[28..32].copy_from_slice(&[0.5,0.5,1.0,1.0]);}
        queue.write_buffer(&post.uniform, 0, &f32_bytes(&uniforms));
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clarity pixels"),
            size: 4096,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        for (target, depth) in [(&car, car_z), (&level, level_z)] {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clarity test depth"),
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
                label: Some("clarity test"),
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
                    rows_per_image: Some(16),
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
            .flat_map(|row| row[..64].to_vec())
            .collect::<Vec<_>>()
    };
    let base = render(1.0, 1.0, 0.0, 0.0);
    assert_eq!(
        render(1.0, 1.0, 0.8, 0.0),
        base,
        "background must not be sharpened"
    );
    assert_eq!(
        render(0.4, 0.2, 0.8, 0.0),
        base,
        "a level surface in front of the car must not be sharpened"
    );
    assert_eq!(
        render(0.500001, 0.5, 0.8, 0.0),
        base,
        "even a close occluder must be respected"
    );
    assert_ne!(
        render(0.4, 0.8, 0.8, 0.0),
        base,
        "visible car detail must receive sharpening"
    );
    assert_ne!(render(1.0,1.0,0.0,0.8),base,"peripheral background must visibly smear");
    assert_eq!(render(0.4,0.8,0.0,0.8),base,"visible car must stay sharp even at the screen edges");
    assert_ne!(render(0.4,0.2,0.0,0.8),base,"level in front of the car must still receive speed blur");
    protect_region.set(true);
    assert_eq!(render(1.0,1.0,0.0,0.8),base,"projected car region must fill gaps in sparse Gaussian depth");

}
