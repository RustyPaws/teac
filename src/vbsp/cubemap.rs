//! `env_cubemap`: the CUBEMAPS lump and per-side patched materials.
//!
//! Every brush side whose material uses `$envmap env_cubemap` gets a patched material
//! `maps/<map>/<material>_<x>_<y>_<z>` pointing `$envmap` at the cubemap texture
//! `maps/<map>/c<x>_<y>_<z>` of the cubemap listing the side, else the nearest one.

use super::load::Level;
use crate::bspfile::DCubemapSample;
use crate::ctx::Ctx;
use crate::material::Materials;
use glam::DVec3;
use std::collections::HashMap;

pub struct Cubemaps {
    pub samples: Vec<DCubemapSample>,
    /// Patched VMTs for the pakfile.
    pub files: Vec<(String, Vec<u8>)>,
}

fn int_origin(v: DVec3) -> [i32; 3] {
    [v.x as i32, v.y as i32, v.z as i32]
}

pub fn process(lv: &mut Level, mats: &Materials, map_name: &str, ctx: &Ctx) -> Cubemaps {
    let mut samples = Vec::new();
    let mut side_owner: HashMap<i32, usize> = HashMap::new();
    for e in lv.entities.iter().filter(|e| e.is("env_cubemap")) {
        let o = int_origin(e.vec3("origin").unwrap_or(DVec3::ZERO));
        let size = e.get("cubemapsize").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let ci = samples.len();
        samples.push(DCubemapSample { origin: o, size });
        for id in e.get("sides").unwrap_or("").split_whitespace().filter_map(|t| t.parse::<i32>().ok()) {
            side_owner.insert(id, ci);
        }
    }
    let mut files = Vec::new();
    if samples.is_empty() {
        return Cubemaps { samples, files };
    }
    let map_lower = map_name.to_ascii_lowercase();
    let mut made: HashMap<(usize, usize), usize> = HashMap::new(); // (texinfo, cubemap) -> texinfo
    let mut patched: HashMap<(String, usize), ()> = HashMap::new();
    let mut count = 0;
    for bi in 0..lv.brushes.len() {
        for si in 0..lv.brushes[bi].sides.len() {
            let side = &lv.brushes[bi].sides[si];
            if side.bevel || side.texinfo < 0 {
                continue;
            }
            let Some(mat) = side.material.clone() else { continue };
            if !mat.param("$envmap").is_some_and(|v| v.trim().eq_ignore_ascii_case("env_cubemap")) {
                continue;
            }
            let ci = match side_owner.get(&side.id) {
                Some(&c) => c,
                None => {
                    // vbsp `FindClosestCubemap`: nearest cubemap in front of the side
                    // (cubemap 0 if none is).
                    let Some(w) = &side.winding else { continue };
                    let c = w.center() + lv.entities[lv.brushes[bi].entity].origin;
                    let n = lv.planes[side.plane].normal;
                    let mut best = (f64::MAX, 0);
                    for (i, s) in samples.iter().enumerate() {
                        let d = DVec3::from(s.origin.map(|x| x as f64)) - c;
                        if d.dot(n) < 0.0 {
                            continue;
                        }
                        if d.length() < best.0 {
                            best = (d.length(), i);
                        }
                    }
                    best.1
                }
            };
            let ti = side.texinfo as usize;
            let new_ti = match made.get(&(ti, ci)) {
                Some(&t) => t,
                None => {
                    let [x, y, z] = samples[ci].origin;
                    let base = side.material_name.clone();
                    let name = format!("maps/{map_lower}/{}_{x}_{y}_{z}", mat.name);
                    if patched.insert((name.clone(), ci), ()).is_none() {
                        let vmt = format!(
                            "\"patch\"\r\n{{\r\n\t\"include\"\t\t\"materials/{base}.vmt\"\r\n\t\"replace\"\r\n\t{{\r\n\t\t\"$envmap\"\t\t\"maps/{map_lower}/c{x}_{y}_{z}\"\r\n\t}}\r\n}}\r\n"
                        );
                        files.push((format!("materials/{name}.vmt"), vmt.into_bytes()));
                    }
                    let td = lv.texdata.find(&name, &mats.get(&base));
                    let mut t = lv.texinfo.list[ti];
                    t.texdata = td as i32;
                    let nt = lv.texinfo.find(t);
                    made.insert((ti, ci), nt);
                    nt
                }
            };
            lv.brushes[bi].sides[si].texinfo = new_ti as i32;
            count += 1;
        }
    }
    ctx.detail(format!("{} cubemaps, {count} sides patched, {} materials", samples.len(), files.len()));
    Cubemaps { samples, files }
}
