//! Rigid wheel animation. Part 0 is the body, 1..4 rotating wheels, 5..8 their non-spinning calipers.
//! Poses are in the original SOG coordinate system; the ordinary car model transform follows them.

pub const PARTS: usize = 9;
pub const RIG_BYTES: usize = (1 + PARTS * 2) * 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartPose {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

impl Default for PartPose {
    fn default() -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0; 3],
        }
    }
}

impl PartPose {
    /// A rigid transform about a pivot, plus suspension displacement, in one coordinate system.
    pub fn around(pivot: [f32; 3], rotation: [f32; 4], displacement: [f32; 3]) -> Self {
        let moved = rotate(rotation, pivot);
        Self {
            rotation,
            translation: [0, 1, 2].map(|i| pivot[i] - moved[i] + displacement[i]),
        }
    }

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let r = rotate(self.rotation, p);
        [0, 1, 2].map(|i| r[i] + self.translation[i])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WheelAnimation {
    pub enabled: bool,
    pub debug: bool,
    pub parts: [PartPose; PARTS],
}

impl WheelAnimation {
    pub fn bytes(&self) -> Vec<u8> {
        let mut values = vec![
            u8::from(self.enabled) as f32,
            u8::from(self.debug) as f32,
            0.0,
            0.0,
        ];
        for pose in self.parts {
            values.extend(pose.rotation);
            values.extend(pose.translation);
            values.push(0.0);
        }
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

pub fn multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + b[3] * a[0] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] + b[3] * a[1] + a[2] * b[0] - a[0] * b[2],
        a[3] * b[2] + b[3] * a[2] + a[0] * b[1] - a[1] * b[0],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

pub fn axis_angle(axis: [f32; 3], radians: f32) -> [f32; 4] {
    let (s, c) = (radians * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

pub fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = multiply(
        multiply(q, [v[0], v[1], v[2], 0.0]),
        [-q[0], -q[1], -q[2], q[3]],
    );
    [p[0], p[1], p[2]]
}

/// Compute deformation is recorded before depth sorting, using the same poses as the render shaders.
pub(crate) struct GpuWheels {
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    count: u32,
}

impl GpuWheels {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        centers: &wgpu::Buffer,
        labels: &wgpu::Buffer,
        rig: &wgpu::Buffer,
        parts: &[u32],
    ) -> Self {
        let original = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("original wheel centres"),
            size: parts.len().max(1) as u64 * 12,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(centers, 0, &original, 0, parts.len() as u64 * 12);
        queue.submit([encoder.finish()]);
        let indices: Vec<u32> = parts
            .iter()
            .enumerate()
            .filter(|(_, part)| **part > 0)
            .map(|(i, _)| i as u32)
            .collect();
        let selected = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wheel splat indices"),
            size: indices.len().max(1) as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let data: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        if !data.is_empty() {
            queue.write_buffer(&selected, 0, &data);
        }
        let source = format!(
            "{}\n{}",
            splat_wgsl::pipeline::WHEEL_MATH,
            include_str!("shaders/wheel_centers.wgsl")
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wheel centres"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("wheel centres"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wheel deformation"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[&original, centers, labels, rig, &selected]
                .iter()
                .enumerate()
                .map(|(i, buffer)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        });
        Self {
            pipeline,
            group,
            count: indices.len() as u32,
        }
    }

    #[cfg(test)]
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_timed(encoder,None);
    }
    pub(crate) fn encode_timed(&self, encoder: &mut wgpu::CommandEncoder, timer: Option<&crate::timing::GpuTimer>) {
        if self.count == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("animate wheel centres"),
            timestamp_writes: timer.and_then(|t|t.compute("wheel_deformation","car",1,u64::from(self.count.div_ceil(256)),u64::from(self.count))),
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.dispatch_workgroups(self.count.div_ceil(256), 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rigid_wheel_keeps_hub_and_rotates_about_axle() {
        let p = PartPose::around(
            [2.0, 3.0, 4.0],
            axis_angle([1.0, 0.0, 0.0], std::f32::consts::FRAC_PI_2),
            [0.0; 3],
        );
        for (a, b) in p.point([2.0, 3.0, 4.0]).into_iter().zip([2.0, 3.0, 4.0]) {
            assert!((a - b).abs() < 1e-5);
        }
        for (a, b) in p.point([2.0, 4.0, 4.0]).into_iter().zip([2.0, 3.0, 5.0]) {
            assert!((a - b).abs() < 1e-5);
        }
        assert_eq!(WheelAnimation::default().bytes().len(), RIG_BYTES);
        assert_eq!(
            crate::camera::quat_to_mat4(PartPose::default().rotation),
            crate::camera::IDENTITY
        );
    }

    #[test]
    fn gpu_deformation_preserves_body_and_calipers_and_resorts_a_stationary_camera() {
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let buffer = |name: &str, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(name),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let points = [0.0f32, 0.0, -1.0, 0.0, 1.0, -2.0, 1.0, 0.0, -3.0];
        let centers = buffer(
            "centres",
            36,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        queue.write_buffer(
            &centers,
            0,
            &points
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let labels = buffer(
            "parts",
            12,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        queue.write_buffer(
            &labels,
            0,
            &[0u32, 16, 80]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let rig = buffer(
            "rig",
            RIG_BYTES as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        queue.write_buffer(&rig, 0, &WheelAnimation::default().bytes());
        let wheels = GpuWheels::new(&device, &queue, &centers, &labels, &rig, &[0, 1, 5]);
        let fade = buffer("fade", 12, wgpu::BufferUsages::STORAGE);
        let sorter = crate::sort::DepthSorter::new(&device, &queue, 3, &centers, &fade);
        let order = buffer(
            "order",
            12,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let readback = buffer(
            "read centres",
            36,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let read = |b: &wgpu::Buffer| {
            let (tx, rx) = std::sync::mpsc::channel();
            b.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let data = b.slice(..).get_mapped_range().unwrap().to_vec();
            b.unmap();
            data
        };
        let mut first = device.create_command_encoder(&Default::default());
        sorter.encode(
            &queue,
            &mut first,
            &crate::camera::IDENTITY,
            None,
            None,
            &order,
        );
        queue.submit([first.finish()]);
        let initial = read(&order);
        let mut animation = WheelAnimation {
            enabled: true,
            ..Default::default()
        };
        animation.parts[1] = PartPose {
            rotation: axis_angle([1.0, 0.0, 0.0], 0.5),
            translation: [0.0, 0.0, -8.0],
        };
        queue.write_buffer(&rig, 0, &animation.bytes());
        sorter.invalidate();
        let mut encoder = device.create_command_encoder(&Default::default());
        wheels.encode(&mut encoder);
        sorter.encode(
            &queue,
            &mut encoder,
            &crate::camera::IDENTITY,
            None,
            None,
            &order,
        );
        encoder.copy_buffer_to_buffer(&centers, 0, &readback, 0, 36);
        queue.submit([encoder.finish()]);
        assert_ne!(
            initial,
            read(&order),
            "wheel motion must change depth order without camera movement"
        );
        let data = read(&readback);
        let actual: Vec<_> = data
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        assert_eq!(&actual[..3], &points[..3], "chassis remains untouched");
        assert_eq!(&actual[6..], &points[6..], "caliper does not spin");
        for (a, b) in actual[3..6]
            .iter()
            .zip(animation.parts[1].point([0.0, 1.0, -2.0]))
        {
            assert!((*a - b).abs() < 1e-5);
        }
        let mut again = device.create_command_encoder(&Default::default());
        wheels.encode(&mut again);
        again.copy_buffer_to_buffer(&centers, 0, &readback, 0, 36);
        queue.submit([again.finish()]);
        assert_eq!(
            data,
            read(&readback),
            "poses apply to immutable originals, never accumulate GPU drift"
        );
        animation.enabled = false;
        queue.write_buffer(&rig, 0, &animation.bytes());
        let mut disabled = device.create_command_encoder(&Default::default());
        wheels.encode(&mut disabled);
        disabled.copy_buffer_to_buffer(&centers, 0, &readback, 0, 36);
        queue.submit([disabled.finish()]);
        assert_eq!(
            read(&readback),
            points
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
            "disabling the rig restores original centres for sorting too"
        );
    }
}
