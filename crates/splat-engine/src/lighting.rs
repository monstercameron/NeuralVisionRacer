//! Per-level sunlight fitted to the baked scan shadows. Shared by native and WASM; catalog is embedded,
//! so it ships even when large level folders are omitted. Profiles describe the default upright orientation.
use crate::EngineConfig;
use splat_render::{camera, surface::CarLight};
use std::sync::OnceLock;

fn catalog() -> &'static serde_json::Value {
    static DATA: OnceLock<serde_json::Value> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../../specs/level-lighting.json"))
            .expect("valid lighting catalog")
    })
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.map(|x| x / l)
}

/// Resolve the selected folder, not a substring (custom levels use the explicit config fallback).
pub fn for_level(config: &EngineConfig) -> CarLight {
    let mut light = CarLight {
        sun: normalize(config.effects_sun),
        color: [1.0; 3],
        intensity: 1.0,
        roughness: 0.32,
    };
    if !config.effects_level_sun {
        return light;
    }
    let name = config
        .level_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let data = catalog();
    let p = &data["levels"][name];
    let vec3 = |key: &str| {
        p[key]
            .as_array()
            .filter(|a| a.len() == 3)
            .map(|a| [0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0) as f32))
    };
    if let Some(sun) = vec3("sun") {
        let delta = (config.level_rotate_z_degrees
            - data["reference_rotate_z_degrees"].as_f64().unwrap_or(180.0) as f32)
            .to_radians();
        let (s, c) = delta.sin_cos();
        light.sun = normalize(camera::transform_point(
            &[
                c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            sun,
        ));
        light.color = vec3("color").unwrap_or([1.0; 3]);
        light.intensity = p["intensity"].as_f64().unwrap_or(1.0) as f32;
        light.roughness = p["roughness"].as_f64().unwrap_or(0.32) as f32;
    }
    light
}

/// Shadow contrast/penumbra are artistic fits to the same capture evidence, independent of material roughness.
/// Low-confidence profiles deliberately use a weaker, softer approximation.
pub fn shadow_profile(config:&EngineConfig)->(f32,f32) {
    if !config.effects_level_sun{return (1.0,0.12);}
    let name=config.level_path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let data=&catalog()["levels"][name];
    (data["shadow_strength_scale"].as_f64().unwrap_or(1.0).clamp(0.0,2.0) as f32,data["shadow_softness"].as_f64().unwrap_or(0.12).clamp(0.03,0.35) as f32)
}

/// Console diagnostics distinguish an estimated profile from the explicit fallback direction.
pub fn status(config: &EngineConfig) -> serde_json::Value {
    let light = for_level(config);
    let name = config
        .level_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let profile = &catalog()["levels"][name];
    let estimated = config.effects_level_sun && !profile.is_null();
    serde_json::json!({
        "level": name,
        "source": if estimated { "baked-shadow-estimate" } else { "effects.sun" },
        "confidence": if estimated { profile["confidence"].as_str() } else { None },
        "sun_world": light.sun,
        "color_linear": light.color,
        "shadow_strength":if config.effects_enabled {(config.effects_shadow.clamp(0.0,1.0)*shadow_profile(config).0).clamp(0.0,1.0)}else{0.0},
        "shadow_softness":shadow_profile(config).1,
        "specular_intensity": if config.effects_enabled { light.intensity * config.effects_car_specular.clamp(0.0,4.0) } else {0.0},
        "roughness": (light.roughness * config.effects_car_roughness.clamp(0.18,0.8) / 0.32).clamp(0.18,0.8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delft_shadow_matches_dark_baked_street_shade() {
        let c=EngineConfig {level_path:"levels/trompetstraat-delft".into(),effects_shadow:0.55,..Default::default()};
        let (scale,softness)=shadow_profile(&c);
        assert!((0.55*scale-0.7425).abs()<0.001);
        assert!((softness-0.14).abs()<0.001);
    }
    #[test]
    fn every_local_level_has_a_distinct_above_horizon_profile() {
        let names = [
            "nmbu-park-03",
            "madras-square",
            "saburi-alley-yazd",
            "basilica-assumption",
            "trompetstraat-delft",
        ];
        let mut directions = Vec::new();
        for name in names {
            let c = EngineConfig {
                level_path: format!("levels/{name}"),
                ..Default::default()
            };
            let l = for_level(&c);
            assert!(
                l.sun[1] > 0.0 && (l.sun.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5
            );
            assert!(l.intensity > 0.0 && l.roughness >= 0.18);
            assert!(!catalog()["levels"][name]["evidence"]
                .as_str()
                .unwrap()
                .is_empty());
            assert!(!directions.contains(&l.sun));
            directions.push(l.sun);
        }
    }
    #[test]
    fn shadows_share_sun_but_do_not_depend_on_specular_enable(){
        let mut c=EngineConfig{level_path:"levels/saburi-alley-yazd".into(),..Default::default()};
        let profile=shadow_profile(&c);let light=for_level(&c);c.effects_car_specular=0.0;
        assert_eq!(for_level(&c).sun,light.sun);assert_eq!(shadow_profile(&c),profile);
        c.level_path="levels/basilica-assumption".into();assert!(shadow_profile(&c).0<profile.0);assert!(shadow_profile(&c).1>profile.1);
        c.effects_level_sun=false;assert_eq!(shadow_profile(&c),(1.0,0.12));
    }
    #[test]
    fn orientation_fallback_and_explicit_override_are_consistent() {
        let mut c = EngineConfig {
            level_path: "levels/nmbu-park-03/".into(),
            ..Default::default()
        };
        let original = for_level(&c);
        c.level_rotate_z_degrees += 90.0;
        let rotated = for_level(&c);
        assert!((rotated.sun[0] + original.sun[1]).abs() < 1e-5);
        assert!((rotated.sun[1] - original.sun[0]).abs() < 1e-5);
        c.effects_level_sun = false;
        assert_eq!(for_level(&c).sun, normalize(c.effects_sun));
        c.effects_level_sun = true;
        c.level_path = "levels/not-nmbu-park-03".into();
        assert_eq!(for_level(&c).sun, normalize(c.effects_sun));
    }
}
