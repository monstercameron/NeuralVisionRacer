use super::*;
use crate::tire::TireMark;
use std::collections::VecDeque;

#[test]
fn tire_projection_darkens_only_road_fades_restores_and_is_profiled() {
    if crate::try_device().is_none() {
        return;
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame =
        pollster::block_on(Frame::new(&instance, None, (128, 128), false, None)).unwrap();
    frame
        .set_level(&[crate::shadow_gpu_tests::floor()])
        .unwrap();
    frame.profile_sample_every(1);
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 5.0, 0.01], [0.0; 3], [0.0, 0.0, -1.0]),
        projection: camera::perspective(65f32.to_radians(), 1.0, 0.01, 100.0),
        width: 128,
        height: 128,
        near: 0.01,
        far: 100.0,
    };
    let base = pollster::block_on(frame.read_rgba(&view));
    let marks = VecDeque::from([TireMark {
        start: [-0.5, 0.0, -1.5],
        end: [-0.5, 0.0, 1.5],
        normal: [0.0, 1.0, 0.0],
        forward: [0.0, 0.0, 1.0],
        width: 0.335,
        phase_start: 0.0,
        phase_end: 3.0,
        strength: 0.7,
        smear: 0.3,
        born: 0.0,
        last_deposit: 0.0,
    }]);
    frame.set_tire_marks(1, &marks, 0.0, 1.0);
    let ink = pollster::block_on(frame.read_rgba(&view));
    frame.wait_idle();
    let changed = base
        .chunks_exact(4)
        .zip(ink.chunks_exact(4))
        .filter(|(a, b)| a[0] > b[0] + 2)
        .count();
    assert!(
        changed > 100,
        "marks must visibly darken the Gaussian road, changed={changed}"
    );
    for (a, b) in base.chunks_exact(4).zip(ink.chunks_exact(4)) {
        assert_eq!(a[3], b[3], "deposition must preserve scene alpha");
        assert!(b[0] <= a[0], "rubber must not brighten the road");
    }
    if frame.gpu_timer.is_some() {
        assert!(frame.profile_json()["passes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "tire_marks"));
    }
    frame.set_tire_marks(1, &marks, 125.0, 1.0);
    assert_eq!(
        base,
        pollster::block_on(frame.read_rgba(&view)),
        "old marks fade completely"
    );
    // Continuing stationary transfer refreshes fading without changing creation order.
    let refreshed = VecDeque::from([TireMark { last_deposit: 125.0, ..marks[0] }]);
    frame.set_tire_marks(2, &refreshed, 125.0, 1.0);
    assert_eq!(ink, pollster::block_on(frame.read_rgba(&view)),
        "a freshly deposited burnout patch stays dark even with an old creation time");
    frame.set_tire_marks(1, &marks, 0.0, 0.0);
    assert_eq!(
        base,
        pollster::block_on(frame.read_rgba(&view)),
        "disable restores the base image"
    );
    // Decals above or far below the finite road projection slab must not paint it.
    let high = VecDeque::from([TireMark {
        start: [-0.5, 2.0, -1.5],
        end: [-0.5, 2.0, 1.5],
        ..marks[0]
    }]);
    frame.set_tire_marks(3, &high, 0.0, 1.0);
    assert_eq!(
        base,
        pollster::block_on(frame.read_rgba(&view)),
        "projection must not reach another elevation"
    );
    // Resizing rebuilds the depth bindings and cached screen bounds.
    frame.resize(96, 96);
    let smaller = View {
        width: 96,
        height: 96,
        ..view
    };
    frame.set_tire_marks(1, &marks, 0.0, 1.0);
    assert_eq!(
        pollster::block_on(frame.read_rgba(&smaller)).len(),
        96 * 96 * 4
    );
}

#[test]
fn tire_projection_rejects_car_and_missing_depth() {
    let Some((device, queue)) = crate::try_device() else {
        return;
    };
    let (color, color_view, level, car) = targets(&device, 64, 64);
    let mut tires = crate::tire::TireLayer::new(&device, &level, &car);
    let marks = VecDeque::from([TireMark {
        start: [0.0, 0.0, -1.0],
        end: [0.0, 0.0, 1.0],
        normal: [0.0, 1.0, 0.0],
        forward: [0.0, 0.0, 1.0],
        width: 0.6,
        phase_start: 0.0,
        phase_end: 2.0,
        strength: 0.8,
        smear: 1.0,
        born: 0.0,
        last_deposit: 0.0,
    }]);
    tires.set(1, &marks, 0.0, 1.0);
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 3.0, 0.0], [0.0; 3], [0.0, 0.0, -1.0]),
        projection: [
            0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        width: 64,
        height: 64,
        near: 0.0,
        far: 10.0,
    };
    let render = |road: f32, occluder: f32| {
        let ready = tires.prepare(&queue, &view, None);
        let mut encoder = device.create_command_encoder(&Default::default());
        for (depth, z) in [(&level, road), (&car, occluder)] {
            let _p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tire test depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(z),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
        }
        {
            let _p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        tires.draw(&mut encoder, &color_view, ready.count, None);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16384,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let result = readback.slice(..).get_mapped_range().unwrap().to_vec();
        result
    };
    let visible = render(0.3, 1.0);
    assert_eq!(
        visible,
        render(0.28, 1.0),
        "core depth noise within the contact slab must not distort tread coordinates"
    );
    assert!(
        visible.chunks_exact(4).any(|p| p[0] < 180),
        "fixture needs visible ink"
    );
    assert!(
        render(0.3, 0.2).iter().all(|p| *p == 255),
        "car must obscure every mark"
    );
    assert!(
        render(1.0, 1.0).iter().all(|p| *p == 255),
        "missing level depth must not mark the sky"
    );
}
