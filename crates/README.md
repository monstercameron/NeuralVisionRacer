# Crates

Splat Racer is split into crates. Each one is its own Cargo workspace; they depend on each other by relative path.

| Crate | What it does |
| --- | --- |
| [`splat-app`](splat-app/README.md) | Native window and the `splat-racer` binary. |
| [`splat-web`](splat-web/README.md) | Browser build (WebAssembly). |
| [`splat-engine`](splat-engine/README.md) | Engine services: config, console, physics, wheels, pacing. |
| [`splat-render`](splat-render/README.md) | wgpu renderer: pools, sorting, LOD streaming, passes. |
| [`splat-wgsl`](splat-wgsl/README.md) | Gaussian-splat shaders, ported from PlayCanvas. |
| [`splat-game`](splat-game/README.md) | State machines for the game flow. |
| [`splat-car`](splat-car/README.md) | Car mechanics and the car specification. |
| [`splat-camera`](splat-camera/README.md) | Chase and orbit cameras. |
| [`splat-hud`](splat-hud/README.md) | Dashboard gauges and `.hud` designs. |
| [`splat-ecs`](splat-ecs/README.md) | Small entity-component-system. |

Run a crate's tests with `cargo test` inside its folder, or all of them with the loop in the [README](../README.md#tests).
