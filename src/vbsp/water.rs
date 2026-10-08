//! Water: `$waterdepth` material patches, LEAFWATERDATA, face fog volumes and
//! LEAFMINDISTTOWATER.

use super::load::Level;
use super::tree::Tree;
use crate::bspfile::DLeafWaterData;
use crate::flags::contents;
use crate::material::Materials;
use crate::math::Aabb;
use std::collections::HashMap;

const WATER: i32 = contents::WATER | contents::SLIME;

/// Connected water brush groups of the world (bounds touching).
fn groups(lv: &Level) -> Vec<Vec<usize>> {
    let water: Vec<usize> = lv.entities[0].brushes.iter().copied().filter(|&b| lv.brushes[b].contents & WATER != 0).collect();
    let mut gid: Vec<usize> = (0..water.len()).collect();
    fn root(g: &mut [usize], mut i: usize) -> usize {
        while g[i] != i {
            g[i] = g[g[i]];
            i = g[i];
        }
        i
    }
    for i in 0..water.len() {
        for j in i + 1..water.len() {
            let a = lv.brushes[water[i]].bounds;
            let grown = Aabb { min: a.min - glam::DVec3::splat(0.1), max: a.max + glam::DVec3::splat(0.1) };
            if grown.intersects(&lv.brushes[water[j]].bounds) {
                let (ri, rj) = (root(&mut gid, i), root(&mut gid, j));
                gid[ri] = rj;
            }
        }
    }
    let mut m: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..water.len() {
        let r = root(&mut gid, i);
        m.entry(r).or_default().push(water[i]);
    }
    let mut v: Vec<Vec<usize>> = m.into_values().collect();
    v.sort();
    v
}

/// Gives the water surface materials a `$waterdepth` patch (vbsp does this so the shader knows
/// how deep the volume is). Returns the patch VMTs for the pakfile.
pub fn patch_depth(lv: &mut Level, mats: &Materials, map_name: &str) -> Vec<(String, Vec<u8>)> {
    let map_lower = map_name.to_ascii_lowercase();
    let mut files = Vec::new();
    let mut made: HashMap<(usize, i32), usize> = HashMap::new();
    for g in groups(lv) {
        let mut b = Aabb::EMPTY;
        for &i in &g {
            b = b.union(&lv.brushes[i].bounds);
        }
        let depth = (b.max.z - b.min.z).round() as i32;
        for &bi in &g {
            for si in 0..lv.brushes[bi].sides.len() {
                let s = &lv.brushes[bi].sides[si];
                let Some(m) = s.material.clone() else { continue };
                if m.shader != "water" || s.texinfo < 0 {
                    continue;
                }
                let ti = s.texinfo as usize;
                let nt = *made.entry((ti, depth)).or_insert_with(|| {
                    let name = format!("maps/{map_lower}/{}_depth_{depth}", m.name);
                    let vmt = format!(
                        "\"patch\"\r\n{{\r\n\t\"include\"\t\t\"materials/{}.vmt\"\r\n\t\"insert\"\r\n\t{{\r\n\t\t\"$waterdepth\"\t\t\"{depth}\"\r\n\t}}\r\n}}\r\n",
                        s.material_name
                    );
                    let path = format!("materials/{name}.vmt");
                    if !files.iter().any(|(p, _)| *p == path) {
                        files.push((path, vmt.into_bytes()));
                    }
                    let td = lv.texdata.find(&name, &mats.get(&s.material_name));
                    let mut t = lv.texinfo.list[ti];
                    t.texdata = td as i32;
                    lv.texinfo.find(t)
                });
                lv.brushes[bi].sides[si].texinfo = nt as i32;
            }
        }
    }
    files
}

/// Water data per water leaf: (data list, tree leaf -> data index).
pub fn leaf_data(lv: &Level, tree: &Tree) -> (Vec<DLeafWaterData>, HashMap<usize, i16>) {
    let mut data: Vec<DLeafWaterData> = Vec::new();
    let mut index: HashMap<(i64, i64, i32), i16> = HashMap::new();
    let mut map = HashMap::new();
    for (n, node) in tree.nodes.iter().enumerate() {
        if !node.is_leaf() || node.contents & WATER == 0 {
            continue;
        }
        let Some(bi) = node.brushes.iter().map(|b| b.original).find(|&b| lv.brushes[b].contents & WATER != 0) else { continue };
        let brush = &lv.brushes[bi];
        // Surface: the upward-facing side.
        let top = brush.sides.iter().filter(|s| !s.bevel).max_by(|a, b| lv.planes[a.plane].normal.z.total_cmp(&lv.planes[b.plane].normal.z));
        let texinfo = top.map_or(-1, |s| s.texinfo);
        let (surface, min) = (brush.bounds.max.z, brush.bounds.min.z);
        let key = ((surface * 16.0) as i64, (min * 16.0) as i64, texinfo);
        let id = *index.entry(key).or_insert_with(|| {
            data.push(DLeafWaterData { surface_z: surface as f32, min_z: min as f32, surface_texinfo_id: texinfo as i16, padding: 0 });
            (data.len() - 1) as i16
        });
        map.insert(n, id);
    }
    (data, map)
}

/// Distance from each leaf box to the nearest water surface (0 inside water).
pub fn min_dist_to_water(leaf_bounds: &[(Aabb, bool)], data: &[DLeafWaterData], water_boxes: &[Aabb]) -> Vec<u16> {
    leaf_bounds
        .iter()
        .map(|(b, in_water)| {
            if *in_water {
                return 0;
            }
            if data.is_empty() || b.is_empty() {
                return 65535;
            }
            let mut best = f64::MAX;
            for w in water_boxes {
                let dx = (w.min.x - b.max.x).max(b.min.x - w.max.x).max(0.0);
                let dy = (w.min.y - b.max.y).max(b.min.y - w.max.y).max(0.0);
                let dz = (w.min.z - b.max.z).max(b.min.z - w.max.z).max(0.0);
                best = best.min((dx * dx + dy * dy + dz * dz).sqrt());
            }
            best.min(65535.0) as u16
        })
        .collect()
}

pub fn water_boxes(lv: &Level) -> Vec<Aabb> {
    groups(lv)
        .into_iter()
        .map(|g| g.iter().fold(Aabb::EMPTY, |a, &b| a.union(&lv.brushes[b].bounds)))
        .collect()
}
