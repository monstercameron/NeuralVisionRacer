use splat_render::{bind_group_layouts, shader_module, try_device};
use splat_wgsl::pipeline::compose_pipeline;

#[test]
fn gsplat_pipeline_is_created_on_a_device() {
    let Some((device, _queue)) = try_device() else {
        eprintln!("no GPU adapter available; skipping pipeline test");
        return;
    };
    let (vs_src, fs_src, bindings) = compose_pipeline().expect("compose");
    let vs = shader_module(&device, "gsplat vs", &vs_src);
    let fs = shader_module(&device, "gsplat fs", &fs_src);
    let layouts = bind_group_layouts(&device, &bindings);
    assert!(!layouts.is_empty());
    let _pipeline = splat_render::gsplat_pipeline(&device, &vs, &fs, &layouts, splat_render::COLOR_FORMAT);
}
