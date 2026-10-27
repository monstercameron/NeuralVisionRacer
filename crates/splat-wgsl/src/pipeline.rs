//! Composes complete gsplat shader modules for the SOG V2 format: include resolution with the
//! runtime-generated chunks, then the binding layer, then the PlayCanvas-construct fixups.
use crate::binding::{bind_engine_declarations, fix_engine_constructs, fix_fragment_constructs, Binding};
use crate::declarations::{sog_streams, wgsl_input_declarations};
use crate::preprocess::{preprocess_with, Defines, Overrides};

pub const WHEEL_MATH: &str = include_str!("local/wheelMath.wgsl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

/// Defines for the SOG V2 format with spherical harmonics, using numeric fog modes as the engine does.
pub fn sog_v2_sh_defines(stage: Stage) -> Defines {
    let mut d = Defines::new();
    d.insert("SH_BANDS".into(), "3".into());
    d.insert("SOG_V2".into(), String::new());
    // The engine's non-dithered materials set DITHER_NONE (gsplat-instance.js configureMaterial); without it
    // the fragment stage takes the stochastic opacity-dither path, which speckles and darkens the splats.
    d.insert("DITHER_NONE".into(), String::new());
    if stage == Stage::Vertex {
        d.insert("VERTEXSHADER".into(), String::new());
    }
    d.insert("NONE".into(), "0".into());
    d.insert("LINEAR".into(), "1".into());
    d.insert("EXP".into(), "2".into());
    d.insert("EXP2".into(), "3".into());
    d.insert("FOG".into(), "0".into());
    d
}

/// Runtime chunks for SOG V2 with SH, as the engine's `GSplatSogResource` registers them.
pub fn sog_v2_sh_overrides() -> Overrides {
    let mut o = Overrides::new();
    o.insert("gsplatContainerDeclarationsVS".into(), wgsl_input_declarations(&sog_streams(true, true)));
    o.insert("gsplatDeclarationsVS".into(), "#include \"gsplatContainerDeclVS\"".into());
    o.insert("gsplatReadVS".into(), "#include \"gsplatSogVS\"".into());
    const SOG: &str = include_str!("../shaders/gsplat/vert/formats/sog.wgsl");
    let from=SOG.find("fn getCenter() -> vec3f {").expect("SOG center getter");
    let to=from+SOG[from..].find("\n}").expect("SOG center end")+2;
    let mut sog=SOG.to_string();
    sog.replace_range(from..to,"var<storage, read> splatCenters: array<f32>;\nfn getCenter() -> vec3f { let i=splat.index*3u; return vec3f(splatCenters[i],splatCenters[i+1u],splatCenters[i+2u]); }");
    o.insert("gsplatSogVS".into(),sog);

    o.insert("gsplatSogSHVS".into(), sog_sh_24bit_labels());
    o.insert("gsplatModifyVS".into(), include_str!("local/carShadowVS.wgsl").into());
    o.insert("wheelMathVS".into(), WHEEL_MATH.into());
    const VS: &str = include_str!("../shaders/gsplat/vert/gsplat.wgsl");
    const DIR: &str = "let dir = normalize(viewDir * modelView3x3);";
    assert!(VS.contains(DIR), "gsplat SH direction hook changed");
    let vs = VS.replace(DIR, "let dir = wheelShDirection(normalize(viewDir * modelView3x3));");
    // Opacity alone can reject a depth splat before covariance projection. Keep the
    // original post-AA/colour-hook rejection too. This specialization applies only
    // when AA is disabled, so no opacity amplification or changed operation order
    // can move a surviving splat across the cutoff.
    const CENTER: &str = "var modelCenter: vec3f = getCenter();";
    assert!(vs.contains(CENTER), "gsplat early opacity hook changed");
    let vs = vs.replace(CENTER, r#"
#if defined(PREPASS_PASS) && !GSPLAT_AA
    let prepassOpacity = textureLoad(sh0, splat.uv, 0).w;
    if (prepassOpacity * splatFade[splat.index] <= uniform.alphaClip) {
        output.position = discardVec;
        return output;
    }
#endif
    var modelCenter: vec3f = getCenter();
"#);
    const READ_COLOR: &str = "var clr: half4 = half4(getColor());";
    assert!(vs.contains(READ_COLOR), "gsplat color hook changed");
    o.insert("gsplatVS".into(), vs.replace(READ_COLOR, "#ifdef PREPASS_PASS\n var clr: half4 = half4(0.0, 0.0, 0.0, textureLoad(sh0, splat.uv, 0).w);\n#else\n var clr: half4 = half4(getColor());\n#endif"));
    // Add clearcoat after the existing gamma decode, before fog/tonemapping. This avoids encoding and then
    // immediately decoding the same lighting contribution for every vertex.
    const OUTPUT: &str = include_str!("../shaders/gsplat/vert/gsplatOutput.wgsl");
    const FOG: &str = "// apply fog in linear space";
    assert!(OUTPUT.contains(FOG), "gsplat output hook changed");
    o.insert("gsplatOutputVS".into(), OUTPUT.replace(FOG, "color *= shadowTransmission;\n    color += carRadiance;\n    // apply fog in linear space"));
    o.insert("gsplatModifyPS".into(), format!("{}\n{}",
        include_str!("local/carClarityPS.wgsl"), include_str!("local/cutoutWindowPS.wgsl")));
    const PS: &str = include_str!("../shaders/gsplat/frag/gsplat.wgsl");
    const ALPHA: &str = "var alpha: half = normExp(A) * gaussianColor.a;";
    assert!(PS.contains(ALPHA), "gsplat alpha hook changed");
    o.insert("gsplatPS".into(), PS.replace(ALPHA, "var alpha: half = half(carClarityAlpha(f32(normExp(A) * gaussianColor.a))) * half(cutoutWindowAlpha());
    if (alpha <= half(0.0)) { discard; }"));
    o
}

/// The engine's SH reader with a 24-bit centroid label: byte 2 of `sh_labels` (blue) is the high byte. A level merged
/// from several chunks needs more than 65,536 centroids. Plain SOG files leave blue at zero once loaded (the loader
/// clears it), so they read the same as before.
fn sog_sh_24bit_labels() -> String {
    const SOURCE: &str = include_str!("../shaders/gsplat/vert/formats/sogSH.wgsl");
    const LABEL: &str = "let n = i32(labels.x * 255.0 + 0.5) + i32(labels.y * 255.0 + 0.5) * 256;";
    assert!(SOURCE.contains(LABEL), "sogSH.wgsl label line changed");
    SOURCE.replace(LABEL, "let n = i32(labels.x * 255.0 + 0.5) + i32(labels.y * 255.0 + 0.5) * 256 + i32(labels.z * 255.0 + 0.5) * 65536;")
}

/// Composes one stage with its own binding numbering (see `compose_pipeline` for the shared one).
pub fn compose_gsplat(stage: Stage) -> Result<(String, Vec<Binding>), crate::preprocess::Error> {
    compose_gsplat_with(stage, &[])
}

/// Composes one stage with extra defines on top of the SOG V2 set, e.g. `PREPASS_PASS` for the depth prepass.
/// `NAME=VALUE` sets a value and overrides the default, e.g. `SH_BANDS=0`.
pub fn compose_gsplat_with(stage: Stage, extra: &[&str]) -> Result<(String, Vec<Binding>), crate::preprocess::Error> {
    let key = match stage {
        Stage::Vertex => "gsplatVS",
        Stage::Fragment => "gsplatPS",
    };
    let mut defines = sog_v2_sh_defines(stage);
    for define in extra {
        let (name, value) = define.split_once('=').unwrap_or((define, ""));
        defines.insert(name.to_string(), value.to_string());
    }
    let src = preprocess_with(key, &mut defines, &sog_v2_sh_overrides())?;
    let (bound, bindings) = bind_engine_declarations(&src);
    let fixed = match stage {
        Stage::Vertex => fix_engine_constructs(&bound),
        Stage::Fragment => fix_fragment_constructs(&bound),
    };
    Ok((fixed, bindings))
}

/// Composes both stages with one shared binding table, so every resource name has the same
/// (group, binding) in the vertex and fragment modules. Returns (vertex, fragment, union bindings).
pub fn compose_pipeline() -> Result<(String, String, Vec<Binding>), crate::preprocess::Error> {
    compose_pipeline_with(&[])
}

/// `compose_pipeline` with extra defines on both stages. `&["PREPASS_PASS"]` gives the depth prepass, which discards
/// fragments below the `alphaClip` uniform.
pub fn compose_pipeline_with(extra: &[&str]) -> Result<(String, String, Vec<Binding>), crate::preprocess::Error> {
    let (vs_src, vs_b) = compose_gsplat_with(Stage::Vertex, extra)?;
    let (fs_src, fs_b) = compose_gsplat_with(Stage::Fragment, extra)?;
    let (table, unified) = crate::binding::unify(&[&vs_b, &fs_b]);
    Ok((
        crate::binding::rebind(&vs_src, &table),
        crate::binding::rebind(&fs_src, &table),
        unified,
    ))
}
