//! Ground-material-aware wheel dust. Emit on fixed physics substeps; animate billboards on the GPU.
use crate::tire_marks::TireContact;
use rapier3d::prelude::Vector;
use splat_render::dust::{DustParticle,CAPACITY};
use std::{collections::{BTreeMap,VecDeque},sync::OnceLock};
#[derive(Clone,Copy,Debug)]
pub struct Surface { pub kind:u8, pub dustiness:f32, pub colour:[f32;3] }
impl Surface {
    pub fn named(kind:u8,sand:bool)->Self {
        let (dustiness,colour)=match kind {
            b'd'=>(1.0,if sand {[0.80,0.71,0.58]} else {[0.60,0.50,0.37]}),
            b'g'=>(0.18,[0.60,0.57,0.43]), b's'=>(0.065,[0.69,0.66,0.59]),
            b'p'=>(0.025,[0.60,0.59,0.56]), _=>(0.025,[0.52,0.50,0.45])
        };Self{kind,dustiness,colour}
    }
    fn name(self)->&'static str {match self.kind {b'd'=>"dry soil/sand",b'g'=>"grass",b's'=>"stone/cobble",b'p'=>"pavement",_=>"unknown"}}
}
struct Grid {origin:[f32;2],dims:[usize;2],ids:Vec<u8>,heights:Vec<i16>,fallback:u8}
fn maps()->&'static BTreeMap<String,Grid> {
    static MAPS:OnceLock<BTreeMap<String,Grid>>=OnceLock::new();
    MAPS.get_or_init(|| {
        let value:serde_json::Value=serde_json::from_str(include_str!("../../../specs/surface-materials.json")).expect("embedded surface maps");
        value["levels"].as_object().expect("levels").iter().map(|(name,v)| {
            let ids=v["ids"].as_str().unwrap().as_bytes().to_vec();
            let dims=[0,1].map(|i|v["dims"][i].as_u64().unwrap() as usize);
            let heights=v["ground_y_dm"].as_array().unwrap().iter().map(|y|y.as_i64().unwrap() as i16).collect::<Vec<_>>();
            assert_eq!(ids.len(),dims[0]*dims[1]);assert_eq!(heights.len(),ids.len());
            (name.clone(),Grid{origin:[0,1].map(|i|v["origin"][i].as_f64().unwrap() as f32),dims,ids,heights,
                fallback:v["fallback"].as_str().unwrap().as_bytes()[0]})
        }).collect()
    })
}
#[derive(Default)]
pub struct Dust {
    pub particles:VecDeque<DustParticle>, pub time:f64, pub revision:u64, pub strength:f32,
    level:String, credit:[f32;4], previous:[Option<Vector>;4], seed:u32,
    clock:Option<fn()->f64>,metrics:splat_render::metrics::Metrics,
    emitted:u64,evicted:u64, latest:[Option<(u8,f32,f32)>;4],
}
impl Dust {
    pub fn set_level(&mut self,path:&str) {
        let name=path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        if self.level!=name {self.level=name.to_owned();self.clear();}
    }
    pub fn set_clock(&mut self,clock:fn()->f64) {self.clock=Some(clock);}
    pub fn clear(&mut self) {self.particles.clear();self.credit=[0.0;4];self.previous=[None;4];self.latest=[None;4];self.revision+=1;}
    pub fn surface_at(&self,p:Vector)->Surface {
        let sand=self.level=="saburi-alley-yazd";
        let Some(grid)=maps().get(&self.level) else {return Surface::named(b'?',false)};
        let (x,z)=((p.x-grid.origin[0]).floor() as isize,(p.z-grid.origin[1]).floor() as isize);
        if x<0||z<0||x>=grid.dims[0] as isize||z>=grid.dims[1] as isize {return Surface::named(b'?',false);}
        let i=x as usize*grid.dims[1]+z as usize;
        let id=if grid.ids[i]==b'.' {grid.fallback} else if (p.y-grid.heights[i] as f32*0.1).abs()>0.65 {b'?'} else {grid.ids[i]};
        Surface::named(id,sand)
    }
    fn random(&mut self)->f32 {self.seed=self.seed.wrapping_mul(1664525).wrapping_add(1013904223);(self.seed>>8) as f32/16777216.0}
    pub fn step(&mut self,contacts:[Option<TireContact>;4],dt:f32) {
        let started=self.clock.map(|c|c());
        let surfaces=contacts.map(|c|c.map(|c|self.surface_at(c.point)));
        self.step_surfaces(contacts,surfaces,dt);
        if let (Some(clock),Some(start))=(self.clock,started) {self.metrics.record("generate",clock()-start);}
    }
    fn step_surfaces(&mut self,contacts:[Option<TireContact>;4],surfaces:[Option<Surface>;4],dt:f32) {
        if !dt.is_finite()||dt<=0.0 {return;}
        self.time+=f64::from(dt);
        // Lifetimes vary: expiry is not ordered by birth time.
        let old=self.particles.len();self.particles.retain(|p|self.time-f64::from(p.born)<f64::from(p.life));
        if old!=self.particles.len(){self.revision+=1;}
        self.latest=[None;4];
        for i in 0..4 {
            let Some(c)=contacts[i].filter(|c|c.load>0.025&&c.normal.y>0.35&&c.point.is_finite()
                &&c.angular_speed.is_finite()&&c.longitudinal_speed.is_finite()&&c.lateral_speed.is_finite()) else {
                self.credit[i]=0.0;self.previous[i]=None;continue;
            };
            let surface=surfaces[i].unwrap();
            let speed=c.longitudinal_speed.hypot(c.lateral_speed);
            let slip=(c.angular_speed*c.radius-c.longitudinal_speed).abs();
            let activity=(speed-0.8).max(0.0)*2.8+(slip-0.75).max(0.0)*5.0+c.lateral_speed.abs()*2.5;
            let rate=(activity*surface.dustiness*c.load.clamp(0.0,1.5).sqrt()).min(110.0);
            self.latest[i]=Some((surface.kind,speed,rate));
            let jumped=self.previous[i].is_some_and(|p|(p-c.point).length()>3.0);
            if jumped {self.credit[i]=0.0;}
            let start=self.previous[i].unwrap_or(c.point);self.previous[i]=Some(c.point);
            if self.strength<=0.0||rate<=0.0 {self.credit[i]=0.0;continue;}
            self.credit[i]+=rate*dt;
            let count=(self.credit[i].floor() as usize).min(32);self.credit[i]-=count as f32;
            for n in 0..count {
                let seed=self.random();let a=self.random()-0.5;let b=self.random()-0.5;
                let normal=c.normal.normalize_or_zero();let side=c.forward.cross(normal).normalize_or_zero();
                let origin=if jumped {c.point} else {start.lerp(c.point,(n as f32+0.5)/count as f32)}
                    +normal*0.07+side*(a*c.width);
                let velocity=normal*(0.25+seed*0.35)-c.forward*(c.longitudinal_speed.signum()*(speed*0.055+slip.min(25.0)*0.06))
                    +side*(b*(0.5+speed*0.025))+Vector::new(0.16,0.0,0.07);
                let particle=DustParticle{origin:origin.to_array(),velocity:velocity.to_array(),normal:normal.to_array(),
                    born:self.time as f32,life:1.7+seed*0.7,size:0.11+(speed.min(30.0)/30.0)*0.11+seed*0.035,
                    opacity:0.12+seed*0.07,colour:surface.colour,seed};
                if self.particles.len()==CAPACITY {self.particles.pop_front();self.evicted+=1;}
                self.particles.push_back(particle);self.emitted+=1;self.revision+=1;
            }
        }
    }
    pub fn profile_json(&self)->serde_json::Value {
        serde_json::json!({"active":self.particles.len(),"capacity":CAPACITY,"emitted":self.emitted,"evicted":self.evicted,
            "simulation_seconds":self.time,"cpu":self.metrics.json(),"material_source":"height-filtered source colour inference plus authored level fallback; 1 m grid",
            "level":self.level,"wheels":self.latest.map(|m|m.map(|(kind,speed,rate)|serde_json::json!({"material":Surface::named(kind,false).name(),"road_mps":speed,"particles_per_s":rate})))})
    }
}
#[cfg(test)] mod tests {
    use super::*;
    fn contact(speed:f32,spin:f32)->TireContact {TireContact{point:Vector::ZERO,normal:Vector::Y,forward:Vector::Z,
        radius:0.35,width:0.3,load:1.0,longitudinal_speed:speed,angular_speed:spin/0.35,..Default::default()}}
    fn run(hz:usize,speed:f32,spin:f32,kind:u8)->Dust {
        let mut d=Dust{strength:1.0,..Default::default()};
        for _ in 0..hz {d.step_surfaces([Some(contact(speed,spin)),None,None,None],[Some(Surface::named(kind,true)),None,None,None],1.0/hz as f32);} d
    }
    #[test] fn emission_tracks_speed_slip_material_and_substeps() {
        assert!(run(180,0.0,0.0,b'd').particles.is_empty());
        let soil=run(180,20.0,20.0,b'd');let paved=run(180,20.0,20.0,b'p');
        assert!(soil.emitted>paved.emitted*15);assert!(soil.emitted>run(180,5.0,5.0,b'd').emitted*3);
        assert!(run(180,0.0,20.0,b'd').emitted>50,"blocked-wheel burnout on dirt");
        assert!(soil.emitted.abs_diff(run(60,20.0,20.0,b'd').emitted)<=1);
    }
    #[test] fn air_pause_expiry_reset_and_capacity_are_bounded() {
        let mut d=run(180,20.0,20.0,b'd');let before=d.time;d.step([None;4],0.0);assert_eq!(d.time,before);
        let emitted=d.emitted;for _ in 0..600 {d.step([None;4],1.0/180.0);}assert_eq!(emitted,d.emitted);assert!(d.particles.is_empty());
        for _ in 0..1000 {d.step_surfaces([Some(contact(100.0,150.0));4],[Some(Surface::named(b'd',true));4],0.1);}
        assert!(d.particles.len()<=CAPACITY);
        let p=*d.particles.back().unwrap();d.particles=std::iter::repeat_n(p,CAPACITY).collect();
        let evicted=d.evicted;d.step_surfaces([Some(contact(100.0,150.0));4],[Some(Surface::named(b'd',true));4],0.1);
        assert_eq!(d.particles.len(),CAPACITY);assert!(d.evicted>evicted);
        d.clear();assert!(d.particles.is_empty());
    }
    #[test] fn baked_material_maps_have_consistent_dimensions_and_separate_grass() {
        assert_eq!(maps().len(),4);let park=&maps()["nmbu-park-03"];
        assert!(park.ids.contains(&b'g')&&park.ids.contains(&b'p'));
        let mut d=Dust::default();d.set_level("levels/saburi-alley-yazd");
        assert_eq!(d.surface_at(Vector::new(-0.725,0.05,-5.775)).kind,b'd');
        assert_eq!(d.surface_at(Vector::new(-0.725,5.05,-5.775)).kind,b'?',"a ground map must not label a roof as soil");
    }
}
