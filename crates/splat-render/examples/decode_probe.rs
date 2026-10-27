//! CPU-only paired SOG WebP decode probe; does not initialize a GPU.
//! Usage: decode_probe OUTPUT_DIR [ROUNDS] stream=path ...
//! Each reference output is saved as raw RGBA for an external SHA-256 digest.
use splat_render::sog::{self, Texture};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

struct Counter;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout)
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        System.realloc(pointer, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;

fn reference(stream: &str, bytes: &[u8]) -> sog::Result<Texture> {
    if bytes.is_empty() && sog::is_sh_stream(stream) {
        return Ok(Texture { width: 0, height: 0, rgba: Vec::new() });
    }
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .map_err(|e| format!("decoding {stream}: {e}"))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let mut rgba = decoded.into_raw();
    if stream == "sh_labels" {
        rgba.chunks_exact_mut(4).for_each(|p| p[2] = 0);
    }
    Ok(Texture { width, height, rgba })
}

fn main() -> sog::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().ok_or("expected OUTPUT_DIR")?);
    let rounds: usize = args.next().ok_or("expected ROUNDS")?.parse()?;
    let fixtures: Vec<(String, Vec<u8>)> = args
        .map(|arg| {
            let (stream, path) = arg.split_once('=').ok_or("expected stream=path")?;
            if !stream.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err("invalid stream name".into());
            }
            Ok((stream.to_owned(), std::fs::read(path)?))
        })
        .collect::<sog::Result<_>>()?;
    if fixtures.is_empty() || rounds == 0 {
        return Err("at least one fixture and one round are required".into());
    }
    std::fs::create_dir_all(&out)?;
    for (stream, bytes) in &fixtures {
        let expected = reference(stream, bytes)?;
        let warm = sog::decode_stream(stream, bytes)?;
        assert_eq!((warm.width, warm.height, &warm.rgba), (expected.width, expected.height, &expected.rgba));
        drop(warm);
        std::fs::write(out.join(format!("decode-{stream}.rgba")), &expected.rgba)?;
        for round in 0..rounds {
            let order = if round % 2 == 0 { ["reference", "candidate"] } else { ["candidate", "reference"] };
            for variant in order {
                let allocs = ALLOCATIONS.load(Ordering::Relaxed);
                let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
                let start = Instant::now();
                let decoded = if variant == "reference" {
                    reference(black_box(stream), black_box(bytes))?
                } else {
                    sog::decode_stream(black_box(stream), black_box(bytes))?
                };
                let ns = start.elapsed().as_nanos();
                let allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocs;
                let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed) - allocated_bytes;
                assert_eq!((decoded.width, decoded.height, &decoded.rgba), (expected.width, expected.height, &expected.rgba));
                println!("{}", serde_json::json!({"stream":stream,"round":round,"variant":variant,"input_bytes":bytes.len(),
                    "width":decoded.width,"height":decoded.height,"output_bytes":decoded.rgba.len(),"ns":ns,
                    "allocations":allocations,"allocated_bytes":allocated_bytes}));
            }
        }
    }
    Ok(())
}
