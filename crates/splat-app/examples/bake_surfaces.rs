//! Bake inferred ground material IDs from height-filtered source splats. Run from the repo root.
use splat_engine::{LevelFloor,EngineConfig};
use splat_render::{sog,lod::LodMeta};
use std::{path::Path,collections::BTreeMap};
fn main() {
    let mut levels=BTreeMap::new();
    for (name,base) in [("saburi-alley-yazd",b'd'),("nmbu-park-03",b'p'),("trompetstraat-delft",b's'),("basilica-assumption",b's')] {
        let dir=Path::new("levels").join(name);
        let floor=LevelFloor::load(&dir,EngineConfig::default().level_floor_clearance_m).unwrap();
        let nz=floor.dims[2] as usize;
        let mut heights=vec![0u16;floor.dims[0] as usize*nz];
        for c in &floor.cells {heights[c[0] as usize*nz+c[2] as usize]=(c[1]+1) as u16;}
        let nx=(floor.dims[0] as f32*floor.resolution).ceil() as usize;
        let mz=(floor.dims[2] as f32*floor.resolution).ceil() as usize;
        let mut sums=vec![[0.0f64;6];nx*mz];
        let meta=LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
        let lod=meta.pick(-1,3_000_000);
        for chunk in meta.chunk_dirs(lod) {
            let bundle=sog::load_dir(&dir.join(chunk)).unwrap();
            let rgba=&bundle.textures["sh0"].rgba;
            let scales=&bundle.textures["scales"].rgba;
            for (i,p) in sog::centres(&bundle).iter().enumerate() {
                let world=[-p[0],-p[1],p[2]]; // source y-down; game 180 degrees about Z
                let x=((world[0]-floor.origin[0])/floor.resolution).floor() as isize;
                let z=((world[2]-floor.origin[2])/floor.resolution).floor() as isize;
                if x<0 || z<0 || x>=floor.dims[0] as isize || z>=floor.dims[2] as isize {continue;}
                let h=heights[x as usize*nz+z as usize];if h==0 {continue;}
                let y=floor.origin[1]+h as f32*floor.resolution;
                if (world[1]-y).abs()>0.20 || rgba[i*4+3]<100 {continue;}
                if (0..3).any(|a|bundle.meta.scales_codebook[scales[i*4+a] as usize].exp()>0.40) {continue;}
                let rgb=[0,1,2].map(|a|(0.5+0.2820948*bundle.meta.sh0_codebook[rgba[i*4+a] as usize]).max(0.0));
                let [r,g,b]=rgb;
                let max=r.max(g).max(b);if max<0.025 {continue;}
                let key=(x as f32*floor.resolution).floor() as usize*mz+(z as f32*floor.resolution).floor() as usize;
                if key>=sums.len(){continue;}
                let v=&mut sums[key];v[0]+=1.0;v[1]+=y as f64;
                if g>r*1.10 && g>b*1.13 {v[2]+=1.0;}
                if r>g*1.06 && g>b*1.13 {v[3]+=1.0;}
                if (max-r.min(g).min(b))/max<0.18 {v[4]+=1.0;}
            }
        }
        let mut ids=vec![b'.';nx*mz];let mut ys=vec![i16::MIN;ids.len()];let mut counts=BTreeMap::new();
        for (i,v) in sums.iter().enumerate() {
            if v[0]<4.0 {continue;}
            ids[i]=if v[2]/v[0]>0.32 {b'g'} else if base==b'd' && v[4]/v[0]>0.70 {b's'} else {base};
            ys[i]=(v[1]/v[0]*10.0).round() as i16;
            *counts.entry((ids[i] as char).to_string()).or_insert(0)+=1;
        }
        levels.insert(name,serde_json::json!({"origin":[floor.origin[0],floor.origin[2]],"cell_m":1.0,"dims":[nx,mz],
            "ids":String::from_utf8(ids).unwrap(),"ground_y_dm":ys,"fallback":(base as char).to_string(),"source_lod":lod,"counts":counts}));
        println!("{name}: {counts:?}");
    }
    let doc=serde_json::json!({"version":1,"method":"Inferred from opaque ground splats within 20 cm of the collision floor; green votes identify grass, neutral paving in Saburi; authored level base elsewhere. Not capture material metadata.","ids":{"p":"pavement","s":"stone/cobble","d":"dry soil/sand","g":"grass",".":"no reliable sample"},"levels":levels});
    std::fs::write("specs/surface-materials.json",serde_json::to_vec(&doc).unwrap()).unwrap();
}
