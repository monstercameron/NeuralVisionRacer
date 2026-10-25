//! Prints the binding table for the composed gsplat stages.
use splat_wgsl::binding::BindingKind;
use splat_wgsl::pipeline::{compose_gsplat, Stage};

fn main() {
    for stage in [Stage::Vertex, Stage::Fragment] {
        let (_, bindings) = compose_gsplat(stage).expect("compose");
        println!("== {stage:?}");
        for b in &bindings {
            let desc = match &b.kind {
                BindingKind::Uniform { ty } => format!("uniform {ty}"),
                BindingKind::Texture { ty, unfilterable } => format!("texture {ty}{}", if *unfilterable { " (unfilterable)" } else { "" }),
                BindingKind::Sampler { ty } => format!("sampler {ty}"),
                BindingKind::Storage { ty, access } => format!("storage<{access}> {ty}"),
            };
            println!("  @group({}) @binding({}) {} : {}", b.group, b.binding, b.name, desc);
        }
    }
}
