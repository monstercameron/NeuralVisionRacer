# Research verification — October 24, 2026

## Passed

- `python3 scripts/fetch_upstream.py`: all **196** source files downloaded and checked against their pinned Git blob hashes. A second invocation verified **196 cached files**, with no downloads or changes.
- Confirmed that each of the three upstream source subsets includes its original `LICENSE` file.
- `python3 scripts/download_supersplat.py --help`: CLI loads successfully with Python 3.9.6.
- Downloader fixture checks: plain PLY, gzip-wrapped PLY, and real converter-produced SOG preserve payload bytes and produce the correct SHA-256/provenance; signed URL queries are omitted from saved URLs. HTML masquerading as PLY, oversized payloads, invalid SOG metadata and overwriting existing output are rejected. **Seven checks passed.** HTTP responses were mocked; no live community download was tested.
- Actual SplatTransform **3.10.3** CPU conversion of a synthetic **128-Gaussian, zero-SH-band PLY** to SOG succeeded. The result was **7,159 bytes**, had metadata **version 2**, and declared **128 Gaussians**.
- Actual SOG → PLY decoding succeeded and preserved the **128-vertex** count. This establishes format plumbing only; lossy numerical fidelity and rendering quality were not evaluated.
- Initialized a local Git repository on branch `main`. No remote or commit was created.

Conversion commands used in a scratch directory:

```sh
npx --yes --package @playcanvas/splat-transform@3.10.3 splat-transform \
  --gpu cpu synthetic.ply synthetic.sog
npx --yes --package @playcanvas/splat-transform@3.10.3 splat-transform \
  synthetic.sog roundtrip.ply
```

## Not yet validated

- No Rust compiler/Cargo is currently available on PATH; native/WASM builds were not attempted.
- No shader compilation, rendering comparison, physics simulation, GPU collider generation, training or large-scene performance test was performed.
- No authenticated OpenAI/ElevenLabs request or paid audio generation was made.
- The supplied downloader requires an actual asset URL; share-page resolution and streamed SOG downloading are not implemented.
- Collected PlayCanvas sources are unchanged reference subsets, not Rust ports or standalone buildable libraries.
