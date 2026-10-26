//! Review imported collision against the visible scan, with repeatable before/after overlays.
//! Run from the repo root: cargo run --release --manifest-path crates/splat-app/Cargo.toml --example collision_review -- <level-dir> <output-dir>
use splat_engine::{collision::LevelCollision, EngineConfig, LevelFloor};
use splat_render::{camera, frame::Frame, lod::LodMeta, scene::View, sog};

use std::{path::Path, fs};
fn save(path: &Path, pixels: &[u8], w: u32, h: u32) {
    image::RgbaImage::from_raw(w,h,pixels.to_vec()).unwrap().save(path).unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let dir=Path::new(&args[0]); let output=Path::new(&args[1]); fs::create_dir_all(output).unwrap();
    let name=dir.file_name().unwrap().to_str().unwrap();
    let config=EngineConfig::parse(&fs::read_to_string("engine.conf").unwrap()).unwrap();
    let level=LevelFloor::load(dir,config.level_floor_clearance_m).unwrap();
    let spawn=level.road_spawn().unwrap_or(config.vehicle_spawn);
    let forward=level.spawn_forward.unwrap_or([0.0,1.0]);
    let target=[spawn[0]+forward[0]*3.0,spawn[1]+0.3,spawn[2]+forward[1]*3.0];
    let eye=if name=="trompetstraat-delft" {
        [spawn[0]-forward[0]*3.0,spawn[1]+1.6,spawn[2]-forward[1]*3.0]
    } else { [spawn[0]-forward[0]*5.0-forward[1]*3.0,spawn[1]+5.5,spawn[2]-forward[1]*5.0+forward[0]*3.0] };
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame=pollster::block_on(Frame::new(&instance,None,(900,600),false,None)).unwrap();
    let meta=LodMeta::parse(&fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    let lod=meta.pick(-1,1_500_000);
    let chunks:Vec<_>=meta.chunk_dirs(lod).iter().map(|c|sog::load_dir(&dir.join(c)).unwrap()).collect();
    frame.set_level(&chunks).unwrap();
    frame.set_level_model([-1.0,0.0,0.0,0.0,0.0,-1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0,0.0,0.0,1.0]);
    let view=View{model:camera::IDENTITY,view:camera::look_at(eye,target,[0.0,1.0,0.0]),projection:camera::perspective(65f32.to_radians(),1.5,0.05,1000.0),width:900,height:600,near:0.05,far:1000.0};
    save(&output.join(format!("{name}-scan.png")),&pollster::block_on(frame.read_rgba(&view)),900,600);
    let mut result=serde_json::json!({"level":name,"spawn":spawn,"eye":eye,"target":target,"voxel_m":level.resolution,
        "coarse_wall_columns":level.walls.iter().flatten().count(),"fitted_boxes":level.wall_boxes.as_ref().unwrap().len(),
        "legacy_volume_m3":level.walls.iter().flatten().count() as f32*level.block_m.powi(2)*(config.level_wall_height_m+1.0),
        "fitted_volume_m3":level.wall_boxes.as_ref().unwrap().iter().map(|b|(0..3).map(|a|b.max[a]-b.min[a]).product::<f32>()).sum::<f32>()});
    for (index,before) in [true,false].into_iter().enumerate() {
        let mut input=level.clone(); let mut config=config.clone();
        if before {input.wall_boxes=None;config.level_floor_max_slope=0.0;}
        let mut collision=LevelCollision::new(&input,&config);
        let mut physics=splat_engine::Physics::new(&config); let world=&mut physics.world; collision.update(world,spawn);world.step();
        let lines=collision.debug_segments(world,4).into_iter().filter(|(a,b,_)|{
            let near=|p:[f32;3]| (p[0]-spawn[0]).hypot(p[2]-spawn[2])<14.0;
            near(*a)&&near(*b)
        }).map(|(a,b,wall)|(a,b,if wall {if before {[1.0,0.05,0.5,0.75]} else {[0.1,1.0,0.3,0.75]}} else {[0.0,0.7,1.0,0.3]})).collect();
        frame.set_world_debug(Some(index as u64),||lines);
        let label=if before {"before"} else {"after"};
        save(&output.join(format!("{name}-{label}.png")),&pollster::block_on(frame.read_rgba(&view)),900,600);
        result[label]=serde_json::json!({"tiles":collision.tiles(),"colliders":world.colliders.len()});
    }
    fs::write(output.join(format!("{name}.json")),serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("{result}");
}
