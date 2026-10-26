/* tslint:disable */
/* eslint-disable */

/**
 * The running game: the engine, its frame, and the clock that turns animation frames into ticks.
 */
export class Viewer {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Adds one level chunk from its files, in `sog::BUNDLE_FILES` order: meta.json, means_l, means_u, quats,
     * scales, sh0, shN_labels, shN_centroids, and redraws the level from every chunk so far, merged so one sort
     * orders them all. Returns the chunk's splat count.
     */
    add_level_chunk(meta: Uint8Array, means_l: Uint8Array, means_u: Uint8Array, quats: Uint8Array, scales: Uint8Array, sh0: Uint8Array, sh_labels: Uint8Array, sh_centroids: Uint8Array): number;
    /**
     * A requested chunk's files, in `sog::BUNDLE_FILES` order, for streaming.
     */
    add_stream_chunk(file: number, meta: Uint8Array, means_l: Uint8Array, means_u: Uint8Array, quats: Uint8Array, scales: Uint8Array, sh0: Uint8Array, sh_labels: Uint8Array, sh_centroids: Uint8Array): void;
    /**
     * `follow` for the chase camera, `free` for the orbit camera.
     */
    camera_mode(): string;
    /**
     * Runs one console line and returns its reply as one JSON line, the same format as the desktop console.
     */
    command(line: string): string;
    /**
     * Builds the viewer on the canvas with id `canvas_id`.
     *
     * - `config_text`: the contents of `engine.conf`.
     * - `sog`: the car's `.sog` bytes.
     * - `level_json`, `level_bin`: the level's `scene.voxel.json` and `scene.voxel.bin`. Pass empty arrays for no
     *   level, which keeps the flat ground plane.
     * - `viewer_settings`: the level's `viewer-settings.json` (its starting camera hints the spawn), or empty.
     * - `level_path`: the selected level folder, including any URL override of engine.conf.
     */
    static create(canvas_id: string, config_text: string, sog: Uint8Array, level_json: Uint8Array, level_bin: Uint8Array, viewer_settings: Uint8Array, level_path: string): Promise<Viewer>;
    /**
     * One animation frame. `now_ms` is `requestAnimationFrame`'s timestamp. Runs whole engine ticks for the elapsed
     * time, then draws the current state.
     */
    frame(now_ms: number): void;
    /**
     * Milliseconds the last frame spent on: engine ticks, chunk decoding, stream selection and uploads, debug views
     * and the HUD, encoding and submitting the render, and in all (CPU time; the GPU runs after).
     */
    frame_timings(): Float64Array;
    /**
     * The level's chunk folders to fetch, as a JSON array (e.g. `["2_0","2_1"]`), from its `lod-meta.json`, at the
     * level of detail `level.lod` selects.
     */
    level_chunk_dirs(lod_meta: Uint8Array): string;
    /**
     * A requested chunk could not be fetched; the stream may ask for it again.
     */
    level_chunk_failed(file: number): void;
    /**
     * Level chunks received so far.
     */
    level_chunks(): number;
    /**
     * Chunk folders the stream wants now, as JSON `[[file, "dir"], ...]`; each is then marked in flight.
     */
    level_requests(): string;
    /**
     * Mouse orbit, in CSS pixels.
     */
    orbit(dx: number, dy: number): void;
    /**
     * The pacing state: display refresh (ms), refreshes per frame, median GPU time (ms).
     */
    pacing(): Float64Array;
    /**
     * Mouse pan, in CSS pixels.
     */
    pan(dx: number, dy: number): void;
    /**
     * Middle-drag height, in CSS pixels: dragging up (negative) raises the camera.
     */
    raise(dy: number): void;
    /**
     * The stream's state as JSON (resident splats, leaves per LOD, chunks in memory), or `null`.
     * The camera eye as drawn this frame (between ticks), for checking motion is smooth.
     */
    render_eye(): Float32Array;
    /**
     * Resizes the canvas drawing buffer to match its displayed size.
     */
    resize(width: number, height: number): void;
    /**
     * Replaces the car with one built from a `.car` spec's text (the page passes `vehicle.spec`'s file).
     */
    set_car_spec(text: string): void;
    /**
     * Direction requests W/S and optional rear handbrake 0..1, steer -1 (left) .. 1 (right).
     * Three-argument callers retain a released handbrake. Controls stay until changed. A pending
     * paddle shift is kept until the engine has stepped with it.
     */
    set_controls(throttle: number, brake: number, steer: number, handbrake?: number | null): void;
    /**
     * Switches the dashboard design: a built-in name (`porsche-992-gt3rs`, `turbo-sport`) or a `.hud` file's text.
     */
    set_hud(design: string): void;
    set_io_metrics(text: string): void;
    /**
     * The actual selected level (?level= can override the config). Also selects the embedded sunlight profile.
     */
    set_level_name(name: string): void;
    /**
     * Paddle shift: +1 up, -1 down.
     */
    shift(direction: number): void;
    /**
     * Frame pacing: call on every animation frame with its timestamp, and run `frame` only when this returns true,
     * so frames are shown for a steady number of display refreshes (see `splat_engine::pacing`).
     */
    should_render(now_ms: number): boolean;
    /**
     * Starts streaming the level: `lod_meta` is its `lod-meta.json`, `chunk_metas` a JSON array of every chunk's
     * `meta.json` in file order (`null` for one that failed).
     */
    start_stream(lod_meta: Uint8Array, chunk_metas: Uint8Array): void;
    /**
     * The engine's status as JSON.
     */
    status(): string;
    /**
     * The leaves nearest the LOD centre with their wanted and resident LOD, for debugging pop-in.
     */
    stream_near(n: number): string;
    stream_stats(): string;
    /**
     * The latest LOD changes, for debugging churn.
     */
    stream_switches(): string;
    /**
     * Whether `level.stream` is on, so the page streams the level instead of loading one level of detail.
     */
    streaming(): boolean;
    /**
     * Switches between the chase camera and the free camera; returns the console reply.
     */
    toggle_follow(): string;
    /**
     * Scroll zoom; positive steps zoom in.
     */
    zoom(steps: number): void;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_viewer_free: (a: number, b: number) => void;
    readonly viewer_add_level_chunk: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number, q: number) => [number, number, number];
    readonly viewer_add_stream_chunk: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number, q: number, r: number) => [number, number];
    readonly viewer_camera_mode: (a: number) => [number, number];
    readonly viewer_command: (a: number, b: number, c: number) => [number, number];
    readonly viewer_create: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number) => any;
    readonly viewer_frame: (a: number, b: number) => void;
    readonly viewer_frame_timings: (a: number) => [number, number];
    readonly viewer_level_chunk_dirs: (a: number, b: number, c: number) => [number, number, number, number];
    readonly viewer_level_chunk_failed: (a: number, b: number) => void;
    readonly viewer_level_chunks: (a: number) => number;
    readonly viewer_level_requests: (a: number) => [number, number];
    readonly viewer_orbit: (a: number, b: number, c: number) => void;
    readonly viewer_pacing: (a: number) => [number, number];
    readonly viewer_pan: (a: number, b: number, c: number) => void;
    readonly viewer_raise: (a: number, b: number) => void;
    readonly viewer_render_eye: (a: number) => [number, number];
    readonly viewer_resize: (a: number, b: number, c: number) => void;
    readonly viewer_set_car_spec: (a: number, b: number, c: number) => [number, number];
    readonly viewer_set_controls: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly viewer_set_hud: (a: number, b: number, c: number) => [number, number];
    readonly viewer_set_io_metrics: (a: number, b: number, c: number) => void;
    readonly viewer_set_level_name: (a: number, b: number, c: number) => void;
    readonly viewer_shift: (a: number, b: number) => void;
    readonly viewer_should_render: (a: number, b: number) => number;
    readonly viewer_start_stream: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly viewer_status: (a: number) => [number, number];
    readonly viewer_stream_near: (a: number, b: number) => [number, number];
    readonly viewer_stream_stats: (a: number) => [number, number];
    readonly viewer_stream_switches: (a: number) => [number, number];
    readonly viewer_streaming: (a: number) => number;
    readonly viewer_toggle_follow: (a: number) => [number, number];
    readonly viewer_zoom: (a: number, b: number) => void;
    readonly wasm_bindgen_7b0a4a0adf17f3da___convert__closures_____invoke___js_sys_76b3957f13640b70___Function_fn_wasm_bindgen_7b0a4a0adf17f3da___JsValue_____wasm_bindgen_7b0a4a0adf17f3da___sys__Undefined___js_sys_76b3957f13640b70___Function_fn_wasm_bindgen_7b0a4a0adf17f3da___JsValue_____wasm_bindgen_7b0a4a0adf17f3da___sys__Undefined_______true_: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen_7b0a4a0adf17f3da___convert__closures_____invoke___wasm_bindgen_7b0a4a0adf17f3da___JsValue__core_37ee0ce0c4cd57d5___result__Result_____wasm_bindgen_7b0a4a0adf17f3da___JsError___true_: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_7b0a4a0adf17f3da___convert__closures_____invoke___wasm_bindgen_7b0a4a0adf17f3da___sys__JsNullable_wgpu_e02f8868a75c5f0f___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_37ee0ce0c4cd57d5___result__Result_____wasm_bindgen_7b0a4a0adf17f3da___JsError___true_: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_7b0a4a0adf17f3da___convert__closures_____invoke___wasm_bindgen_7b0a4a0adf17f3da___sys__JsNullable_wgpu_e02f8868a75c5f0f___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_37ee0ce0c4cd57d5___result__Result_____wasm_bindgen_7b0a4a0adf17f3da___JsError___true__35: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_7b0a4a0adf17f3da___convert__closures_____invoke___wasm_bindgen_7b0a4a0adf17f3da___sys__JsNullable_wgpu_e02f8868a75c5f0f___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_37ee0ce0c4cd57d5___result__Result_____wasm_bindgen_7b0a4a0adf17f3da___JsError___true__36: (a: number, b: number, c: any) => [number, number];
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
