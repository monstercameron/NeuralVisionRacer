//! Source-only experiment for sorted SOG codebook remapping. No GPU is used.
//! Usage: decode_remap_probe OUTPUT_FILE ROUNDS ITERATIONS meta.json ...
//! Includes the actual implementation so the reference is never a rewritten copy.
#[allow(dead_code)]
#[path = "../src/sog.rs"]
mod sog;
#[path = "../../../scripts/decode_remap_candidate.rs"]
mod remap_candidate;

use std::{hint::black_box, time::Instant};
use remap_candidate::candidate;

fn validate_edges() -> Vec<u8> {
    let nan = f32::from_bits(0x7fc0_1234);
    let ordered = vec![f32::NEG_INFINITY, -4.0, -0.0, 0.0, 0.5, 1.0, 1.0, f32::INFINITY];
    let mut descending = ordered.clone();
    descending.reverse();
    let mut large: Vec<f32> = (0..301).map(|i| i as f32 - 150.0).collect();
    large[300] = nan; // ignored beyond the first 256 entries of `from`
    let sources = [vec![], vec![nan], vec![-0.0, 0.0], vec![-0.5, 0.5], ordered.clone(), descending.clone(), large];
    let targets = [vec![0.0], vec![-0.0, 0.0, 1.0, 1.0], ordered, descending, vec![nan], vec![0.0, nan, 1.0],
        (0..301).map(|i| i as f32 - 150.0).collect()];
    let mut outputs = Vec::new();
    for from in &sources {
        for to in &targets {
            let old = sog::remap(from, to);
            assert_eq!(candidate(from, to), old);
            outputs.extend_from_slice(&old);
        }
    }
    // Random bit patterns exercise NaNs, infinities, subnormals, large values,
    // sorting and deliberately unordered inputs, in addition to the named cases.
    let mut state = 0x9e37_79b9u32;
    for round in 0..128 {
        let mut make = |n| (0..n).map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            f32::from_bits(state)
        }).collect::<Vec<_>>();
        let mut from = make(round % 300);
        let mut to = make(1 + (round * 17 % 300));
        from.sort_by(f32::total_cmp);
        to.sort_by(f32::total_cmp);
        if round % 2 == 0 { from.reverse(); }
        if round % 3 == 0 { to.reverse(); }
        let old = sog::remap(&from, &to);
        assert_eq!(candidate(&from, &to), old);
        outputs.extend_from_slice(&old);
    }
    // Match the existing behavior for an empty target, including debug-mode
    // overflow checks versus release-mode wrapping.
    assert_eq!(candidate(&[], &[]), sog::remap(&[], &[]));
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let candidate_result = std::panic::catch_unwind(|| candidate(&[0.0], &[]));
    let reference_result = std::panic::catch_unwind(|| sog::remap(&[0.0], &[]));
    std::panic::set_hook(hook);
    match (candidate_result, reference_result) {
        (Ok(candidate), Ok(reference)) => assert_eq!(candidate, reference),
        (Err(_), Err(_)) => {},
        _ => panic!("empty target behaves differently"),
    }
    outputs
}

fn main() -> sog::Result<()> {
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("expected OUTPUT_FILE")?;
    let rounds: usize = args.next().ok_or("expected ROUNDS")?.parse()?;
    let iterations: usize = args.next().ok_or("expected ITERATIONS")?.parse()?;
    let metas: Vec<sog::Meta> = args.map(|p| sog::parse_meta_json(&std::fs::read(p)?)).collect::<sog::Result<_>>()?;
    if metas.is_empty() || rounds == 0 || iterations == 0 { return Err("fixtures, rounds and iterations are required".into()); }
    let edge_outputs = validate_edges();
    let scales = sog::merged_codebook(metas.iter().map(|m| m.scales_codebook.as_slice()));
    let sh0 = sog::merged_codebook(metas.iter().map(|m| m.sh0_codebook.as_slice()));
    let mut sh_n = sog::merged_codebook(metas.iter().map(|m| m.sh_n_codebook.as_slice()));
    let zero = (0..256).min_by(|&a, &b| sh_n[a].abs().total_cmp(&sh_n[b].abs())).unwrap_or(0);
    sh_n[zero] = 0.0;
    let books: Vec<(&[f32], &[f32])> = metas.iter().flat_map(|m| [
        (m.scales_codebook.as_slice(), scales.as_slice()),
        (m.sh0_codebook.as_slice(), sh0.as_slice()),
        (m.sh_n_codebook.as_slice(), sh_n.as_slice()),
    ]).collect();
    let mut outputs = edge_outputs;
    outputs.reserve(books.len() * 256);
    for &(from, to) in &books {
        let old = sog::remap(from, to);
        let new = candidate(from, to);
        assert_eq!(old, new);
        outputs.extend_from_slice(&old);
    }
    std::fs::write(output, &outputs)?;
    for round in 0..rounds {
        let order = if round % 2 == 0 { ["reference", "candidate"] } else { ["candidate", "reference"] };
        for variant in order {
            let start = Instant::now();
            for _ in 0..iterations {
                for &(from, to) in &books {
                    let from = black_box(from);
                    let to = black_box(to);
                    let map = if variant == "reference" { sog::remap(from, to) } else { candidate(from, to) };
                    black_box(map);
                }
            }
            let ns = start.elapsed().as_nanos();
            println!("{}", serde_json::json!({"round":round,"variant":variant,"books":books.len(),"iterations":iterations,"ns":ns}));
        }
    }
    Ok(())
}
