//! Prints the naga result for a composed gsplat stage: `cargo run --example naga_check -- vertex|fragment`.
use splat_wgsl::pipeline::{compose_gsplat, Stage};

fn main() {
    let stage = match std::env::args().nth(1).as_deref() {
        Some("fragment") => Stage::Fragment,
        _ => Stage::Vertex,
    };
    let (src, bindings) = compose_gsplat(stage).expect("compose");
    println!("bindings: {}", bindings.len());
    std::fs::write("/tmp/gsplat.bound.wgsl", &src).unwrap();
    match naga::front::wgsl::parse_str(&src) {
        Ok(module) => match naga::valid::Validator::new(Default::default(), Default::default()).validate(&module) {
            Ok(_) => println!("VALID"),
            Err(e) => println!("VALIDATION ERROR:\n{e:?}"),
        },
        Err(e) => println!("PARSE ERROR:\n{}", e.emit_to_string(&src)),
    }
}
