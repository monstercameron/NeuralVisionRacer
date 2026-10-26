// Live visual controls use the same typed engine settings as the console.
// Strength toggles remember the last nonzero value instead of resetting the tuned grade.
export const SHADER_GROUPS = [
  ["Scene rendering", [
    ["render.sky", "Sky", true, "Environment sky shader"],
    ["render.level", "Level splats", true, "Level colour, depth, culling and sorting"],
    ["render.car", "Car splats", true, "Car colour, depth, culling and sorting"],
    ["render.car_cutout", "Car visibility cutout", true, "Clear nearby splats that obscure the car"],
  ]],
  ["Car & road", [
    ["effects.wheels", "Wheel animation", true, "Wheel deformation, steering and suspension shaders"],
    ["effects.shadow", "Car shadows", 0.55, "Surface-fitted shadows using the level sun"],
    ["effects.car_specular", "Specular highlights", 1, "Sunlight clearcoat and occlusion"],
    ["effects.car_sharpen", "Car sharpening", 0.85, "Depth-masked sharpening"],
    ["effects.car_clarity", "Car clarity", 0.18, "Local car contrast"],
    ["effects.car_haze_cutoff", "Haze removal", 0.02, "Reject faint car splat tails"],
    ["effects.tire_marks", "Tire marks", 1, "Road tread and skid decal shader"],
    ["effects.dust", "Tire smoke & dust", 1, "Floor-sampled tire smoke on streets and dust on loose ground"],
  ]],
  ["Post-processing", [
    ["effects.dof", "Depth of field", 1, "Depth-sensitive focus blur; skips the pass when off"],
    ["effects.blur", "Speed blur", 0.85, "Peripheral streaks that increase with speed"],
    ["effects.fringe", "Chromatic aberration", 0.006, "Speed-sensitive colour fringing"],
    ["effects.grain", "Film grain", 0.018, "Animated grain"],
    ["effects.grade", "Cinematic colour grade", 1, "Contrast, colour balance, highlight rolloff and vignette"],
  ]],
  ["Camera & overlays", [
    ["effects.shake_m", "Camera shake", 0.035, "Speed-sensitive camera movement"],
    ["effects.dezoom_fov_degrees", "Speed field of view", 12, "Widen the lens with speed"],
    ["effects.dezoom_m", "Speed camera pullback", 1.2, "Move the chase camera back with speed"],
    ["hud.enabled", "Dashboard", true, "Dashboard overlay shader"],
    ["render.debug_colliders", "Collision grid", true, "Floor and wall debug lines"],
    ["render.debug_lod_boxes", "LOD boxes", true, "Streaming detail debug lines"],
    ["render.debug_wheels", "Wheel classification", true, "Colour car splats by wheel part"],
  ]],
];

export function setupShaderMenu(viewer, container, restore, notice) {
  const read = (path) => {
    const reply = JSON.parse(viewer.command(`get ${path}`));
    if (!reply.ok) throw new Error(reply.error);
    return reply.data.value;
  };
  const write = (path, value) => {
    const reply = JSON.parse(viewer.command(`set ${path} ${value}`));
    if (!reply.ok) throw new Error(reply.error);
  };
  const initial = new Map();
  const remembered = new Map();
  const controls = [];
  function add(parent, [path, label, fallback, description]) {
    const value = read(path);
    initial.set(path, value);
    const row = document.createElement("label");
    row.className = "shader-toggle";
    row.title = description;
    const input = document.createElement("input");
    input.type = "checkbox";
    input.dataset.setting = path;
    const text = document.createElement("span");
    text.textContent = label;
    row.append(input, text);
    parent.append(row);
    const enabled = value => typeof fallback === "boolean" ? value === "true" : Number(value) > 0;
    controls.push({path, input, fallback, enabled});
    input.addEventListener("change", () => {
      try {
        const current = read(path);
        if (enabled(current)) remembered.set(path, current);
        write(path, input.checked ? remembered.get(path) ?? fallback : typeof fallback === "boolean" ? false : 0);
        notice.textContent = `${label} ${input.checked ? "on" : "off"}. Applied live.`;
      } catch (error) { notice.textContent = error.message; }
      sync();
    });
  }
  add(container, ["effects.enabled", "Visual effects enabled", true, "Master switch for car effects, post-processing and speed camera effects"]);
  for (const [name, entries] of SHADER_GROUPS) {
    const group = document.createElement("fieldset");
    const legend = document.createElement("legend");
    legend.textContent = name;
    group.append(legend);
    for (const entry of entries) add(group, entry);
    container.append(group);
  }
  function sync() {
    const master = read("effects.enabled") === "true";
    for (const control of controls) {
      const value = read(control.path);
      control.input.checked = control.enabled(value);
      if (control.input.checked) remembered.set(control.path, value);
      // Preserve each choice while the master temporarily bypasses its effects.
      control.input.disabled = !master && control.path.startsWith("effects.") && control.path !== "effects.enabled";
    }
  }
  restore.addEventListener("click", () => {
    try {
      for (const [path, value] of initial) write(path, value);
      remembered.clear();
      notice.textContent = "Restored the settings from when this level loaded.";
    } catch (error) { notice.textContent = error.message; }
    sync();
  });
  sync();
  return { sync };
}
