//! Static prop shadows: the props' collision models (`.phy`, IVP compact surfaces) become
//! occluder triangles, placed with the transforms from the `sprp` game lump.

use super::bvh::Tri;
use super::scene::TRI_OPAQUE_NO_FACE;
use crate::bspfile::BspFile;
use crate::fs::GameFs;
use crate::math::angles_matrix;
use glam::{DVec3, Vec3};
use std::collections::HashMap;

pub struct PropInstance {
    pub model: String,
    pub origin: DVec3,
    pub angles: DVec3,
    pub solid: u8,
    pub flags: u8,
}

/// Reads the static props of a v9 `sprp` lump.
pub fn read_props(bsp: &BspFile) -> Vec<PropInstance> {
    let Some(g) = bsp.game_lump("sprp") else { return Vec::new() };
    let d = &g.data;
    let rd_i32 = |o: usize| d.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
    let rd_f32 = |o: usize| f32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as f64;
    let Some(nd) = rd_i32(0) else { return Vec::new() };
    let mut names = Vec::new();
    for i in 0..nd.max(0) as usize {
        let s = &d[4 + i * 128..4 + (i + 1) * 128];
        let end = s.iter().position(|&c| c == 0).unwrap_or(128);
        names.push(String::from_utf8_lossy(&s[..end]).to_string());
    }
    let mut o = 4 + nd as usize * 128;
    let nl = rd_i32(o).unwrap_or(0).max(0) as usize;
    o += 4 + nl * 2;
    let np = rd_i32(o).unwrap_or(0).max(0) as usize;
    o += 4;
    let size = if np > 0 { (d.len() - o) / np } else { 0 };
    let mut out = Vec::with_capacity(np);
    for i in 0..np {
        let b = o + i * size;
        if b + 34 > d.len() {
            break;
        }
        let ty = u16::from_le_bytes([d[b + 24], d[b + 25]]) as usize;
        out.push(PropInstance {
            model: names.get(ty).cloned().unwrap_or_default(),
            origin: DVec3::new(rd_f32(b), rd_f32(b + 4), rd_f32(b + 8)),
            angles: DVec3::new(rd_f32(b + 12), rd_f32(b + 16), rd_f32(b + 20)),
            solid: d[b + 30],
            flags: d[b + 31],
        });
    }
    out
}

/// Triangles of every convex in a `.phy` file, in model space (inches).
pub fn phy_triangles(b: &[u8]) -> Vec<[DVec3; 3]> {
    let mut out = Vec::new();
    let rd = |o: usize| b.get(o..o + 4).map(|x| i32::from_le_bytes(x.try_into().unwrap()));
    let Some(header) = rd(0) else { return out };
    let solids = rd(8).unwrap_or(0).max(0) as usize;
    let mut o = header.max(0) as usize;
    for _ in 0..solids {
        let Some(size) = rd(o) else { break };
        let solid = o + 4;
        o = solid + size.max(0) as usize;
        // "VPHY" solids have a 28-byte header before the compact surface.
        let surf = if b.get(solid..solid + 4) == Some(b"VPHY") { solid + 28 } else { solid };
        let Some(root) = rd(surf + 32) else { continue };
        // Walk the ledge tree.
        let mut stack = vec![surf + root as usize];
        let mut guard = 0;
        while let Some(node) = stack.pop() {
            guard += 1;
            if guard > 100_000 || node + 28 > b.len() {
                break;
            }
            let right = rd(node).unwrap_or(0);
            let ledge_off = rd(node + 4).unwrap_or(0);
            if right == 0 {
                if ledge_off != 0 {
                    ledge_triangles(b, (node as i64 + ledge_off as i64) as usize, &mut out);
                }
            } else {
                stack.push(node + right as usize);
                stack.push(node + 28);
            }
        }
    }
    out
}

fn ledge_triangles(b: &[u8], ledge: usize, out: &mut Vec<[DVec3; 3]>) {
    let rd = |o: usize| b.get(o..o + 4).map(|x| i32::from_le_bytes(x.try_into().unwrap()));
    let (Some(point_off), Some(_)) = (rd(ledge), rd(ledge + 4)) else { return };
    let Some(ntri) = b.get(ledge + 12..ledge + 14).map(|x| i16::from_le_bytes([x[0], x[1]])) else { return };
    let points = (ledge as i64 + point_off as i64) as usize;
    let point = |i: usize| -> Option<DVec3> {
        let o = points + i * 16;
        let f = |k: usize| b.get(o + k..o + k + 4).map(|x| f32::from_le_bytes(x.try_into().unwrap()) as f64);
        let (x, y, z) = (f(0)?, f(4)?, f(8)?);
        // IVP (x, -z, y) metres -> Hammer inches.
        Some(DVec3::new(x, z, -y) / 0.0254)
    };
    for t in 0..ntri.max(0) as usize {
        let tri = ledge + 16 + t * 16;
        let mut v = [DVec3::ZERO; 3];
        let mut ok = true;
        for k in 0..3 {
            let Some(e) = b.get(tri + 4 + k * 4..tri + 8 + k * 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())) else { return };
            match point((e & 0xffff) as usize) {
                Some(p) => v[k] = p,
                None => ok = false,
            }
        }
        if ok {
            out.push(v);
        }
    }
}

/// Occluder triangles of all shadow-casting static props.
pub fn prop_occluders(bsp: &BspFile, fs: &GameFs) -> (Vec<Tri>, usize) {
    let mut cache: HashMap<String, Vec<[DVec3; 3]>> = HashMap::new();
    let mut tris = Vec::new();
    let mut count = 0;
    for p in read_props(bsp) {
        // Non-solid props and props with shadows disabled don't block light.
        if p.solid == 0 || p.flags & 0x10 != 0 {
            continue;
        }
        let mesh = cache.entry(p.model.clone()).or_insert_with(|| {
            let phy = p.model.trim_end_matches(".mdl").to_string() + ".phy";
            fs.read(&phy).map(|b| phy_triangles(&b)).unwrap_or_default()
        });
        if mesh.is_empty() {
            continue;
        }
        count += 1;
        let m = angles_matrix(p.angles);
        let f = |v: DVec3| {
            let w = m * v + p.origin;
            Vec3::new(w.x as f32, w.y as f32, w.z as f32)
        };
        for t in mesh.iter() {
            tris.push(Tri::new(f(t[0]), f(t[1]), f(t[2]), TRI_OPAQUE_NO_FACE));
        }
    }
    (tris, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_portal2_phy() {
        let Some(p2) = crate::testutil::portal2_dir() else { return };
        let fs = GameFs::mount(&p2.join("portal2")).unwrap();
        let tris = phy_triangles(&fs.read("models/props_bts/handrail_64.phy").unwrap());
        assert!(!tris.is_empty());
        // A 64-unit handrail: extent along its length about 64 inches.
        let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
        for t in &tris {
            for v in t {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
        }
        let ext = hi - lo;
        assert!(ext.max_element() > 40.0 && ext.max_element() < 80.0, "{ext}");
    }
}
