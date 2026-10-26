use super::*;

#[test]
fn cinematic_grade_protects_neutrals_keeps_shadow_detail_and_bypasses_cleanly() {
    let Some((device, queue)) = crate::try_device() else {
        return;
    };
    let size = wgpu::Extent3d {
        width: 16,
        height: 16,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grade source"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let palette = [[255,0,0],[0,255,0],[0,0,255],[255,255,0],[0,255,255],[255,0,255],
        [80,40,20],[20,60,80],[210,170,140],[80,100,25],[235,235,235],[5,5,5],[30,30,30],
        [128,128,128],[255,255,255],[0,0,0]];
    let pixels: Vec<u8> = (0..256).flat_map(|i| {
        let rgb = if i / 16 < 8 { [((i % 16) * 17) as u8;3] } else {palette[i % 16]};
        [rgb[0],rgb[1],rgb[2],255]
    }).collect();
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
    let render = |strength: f32| {
        // Exercise the actual production pipeline and its 128-byte WebGPU binding.
        let mut uniforms = [0.0f32; 28];
        uniforms[3] = strength;
        uniforms[5] = 1.0 / 16.0; uniforms[6] = 1.0 / 16.0;
        uniforms[7] = 0.60;
        uniforms[8] = 0.82; uniforms[9] = 0.96;
        uniforms[12..15].copy_from_slice(&[-0.045,0.014,0.032]);
        uniforms[16..19].copy_from_slice(&[0.055,0.018,-0.035]);
        uniforms[24..28].copy_from_slice(&[0.10,0.16,0.0,0.008]);
        queue.write_buffer(&post.uniform, 0, &f32_bytes(&uniforms));
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grade pixels"),
            size: 4096,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        for (target, depth) in [(&car, 1.0), (&level, 1.0)] {
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
    let base = render(0.0);
    assert!(base.iter().zip(&pixels).all(|(a,b)| a.abs_diff(*b)<=1), "grade off must preserve input colour");
    let film = render(1.0);
    let y = |p: &[u8]| 0.2126*f32::from(p[0])+0.7152*f32::from(p[1])+0.0722*f32::from(p[2]);
    let row:Vec<_> = film[..64].chunks_exact(4).collect();
    for pair in row.windows(2) {assert!(y(pair[1])>=y(pair[0]),"grey ramp must remain monotonic");}
    for index in [0,15] {
        let p=row[index];
        assert!(p[0].abs_diff(p[1])<=1 && p[1].abs_diff(p[2])<=1,"protect neutral black/white: {p:?}");
    }
    assert!(row[0][0]<=4 && row[15][0]>220 && row[15][0]<250,"gentle toe and highlight shoulder");
    assert!(y(row[3])>51.0*0.55,"retain shaded road detail");
    assert!(row[4][2]>row[4][0]+3,"cooler shadows");
    assert!(row[11][0]>row[11][2]+3,"warmer light");
    let half = render(0.5);
    for ((a,b),c) in base.iter().zip(&film).zip(&half) {
        assert!((f32::from(*c)-(f32::from(*a)+f32::from(*b))*0.5).abs()<=1.5,"continuous grade-strength blend");
    }
    assert!(film.chunks_exact(4).all(|p| p[3]==255));
}
