//! PHYSCOLLIDE lump: VPhysics (IVP) compact surfaces built from brushes.
//!
//! Layout (matched against Valve-compiled Portal 2 maps):
//! `dphysmodel_t { modelIndex, dataSize, keydataSize, solidCount }`, then per solid
//! `int size; "VPHY" v0x100 type0; compactsurfaceheader_t; IVP_Compact_Surface`, then the
//! keydata text; a `-1` model terminates the lump.
//!
//! An `IVP_Compact_Surface` is `[header 48][ledges][shared points][ledge tree]`. Each brush is
//! one convex ledge of triangles whose edges link to their opposite edges; points are in IVP
//! space: `(x, -z, y)` in metres.

use super::load::Level;
use crate::flags::contents;
use crate::math::{Plane, Winding};
use glam::DVec3;
use std::collections::HashMap;

const METERS_PER_INCH: f64 = 0.0254;
/// `MASK_SOLID` minus grates: the world's main static solid.
const WORLD_SOLID_MASK: i32 = contents::SOLID | contents::WINDOW | contents::MOVEABLE | contents::MONSTER;
/// Brush entities are shrunk by this much so they rest on surfaces without interpenetrating.
const MODEL_SHRINK: f64 = 0.5;
const DENSITY: f64 = 2000.0;

fn to_ivp(p: DVec3) -> DVec3 {
    DVec3::new(p.x, -p.z, p.y) * METERS_PER_INCH
}

/// A convex hull: welded points and outward-facing triangles (counter-clockwise).
struct Convex {
    brush: usize,
    points: Vec<DVec3>,
    tris: Vec<([u32; 3], u8)>,
}

/// Builds a convex from brush side planes (optionally shrunk), in IVP space.
fn brush_convex(lv: &Level, bi: usize, shrink: f64, material: &mut dyn FnMut(&str) -> u8) -> Option<Convex> {
    let b = &lv.brushes[bi];
    let sides: Vec<(Plane, &str)> = b
        .sides
        .iter()
        .filter(|s| !s.bevel)
        .map(|s| {
            let p = lv.planes[s.plane];
            let sp = s.material.as_ref().map(|m| m.surfaceprop.as_str()).unwrap_or("default");
            (Plane::new(p.normal, p.dist - shrink), sp)
        })
        .collect();
    let mut points: Vec<DVec3> = Vec::new();
    let weld = |points: &mut Vec<DVec3>, p: DVec3| -> u32 {
        if let Some(i) = points.iter().position(|q| (*q - p).abs().max_element() < 0.01) {
            return i as u32;
        }
        points.push(p);
        (points.len() - 1) as u32
    };
    let mut tris = Vec::new();
    for (i, (pl, sp)) in sides.iter().enumerate() {
        let mut w = Some(Winding::base(pl));
        for (j, (pj, _)) in sides.iter().enumerate() {
            if i != j {
                w = w.and_then(|w| w.chop(pj, 0.0));
            }
        }
        let Some(w) = w else { continue };
        let mut idx: Vec<u32> = w.points.iter().map(|&p| weld(&mut points, p)).collect();
        idx.dedup();
        while idx.len() > 1 && idx.first() == idx.last() {
            idx.pop();
        }
        if idx.len() < 3 {
            continue;
        }
        let m = material(sp);
        // Windings are clockwise from the front; IVP wants counter-clockwise.
        for k in 1..idx.len() - 1 {
            tris.push(([idx[0], idx[k + 1], idx[k]], m));
        }
    }
    if tris.len() < 4 {
        return None;
    }
    let points = points.into_iter().map(to_ivp).collect();
    Some(Convex { brush: bi, points, tris })
}

/// Volume, centroid and inertia (per unit mass, about the centroid's axes) of a closed mesh.
fn mass_props(points: &[DVec3], tris: &[([u32; 3], u8)]) -> (f64, DVec3) {
    let mut vol = 0.0;
    let mut c = DVec3::ZERO;
    for (t, _) in tris {
        let (a, b, d) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
        let v = a.dot(b.cross(d)) / 6.0;
        vol += v;
        c += (a + b + d) * (v / 4.0);
    }
    if vol.abs() < 1e-12 {
        return (0.0, DVec3::ZERO);
    }
    (vol, c / vol)
}

/// Diagonal inertia of the solid per unit mass about `center` (tetrahedron decomposition).
fn inertia(points: &[DVec3], tris: &[([u32; 3], u8)], center: DVec3) -> DVec3 {
    let mut vol = 0.0;
    let mut sq = DVec3::ZERO; // ∫x², ∫y², ∫z²
    for (t, _) in tris {
        let (a, b, d) = (points[t[0] as usize] - center, points[t[1] as usize] - center, points[t[2] as usize] - center);
        let v = a.dot(b.cross(d)) / 6.0;
        vol += v;
        // ∫ over tetra (0,a,b,d) of x² = v/10 * (ax²+bx²+dx² + ax bx + ax dx + bx dx)
        let f = |i: usize| a[i] * a[i] + b[i] * b[i] + d[i] * d[i] + a[i] * b[i] + a[i] * d[i] + b[i] * d[i];
        sq += DVec3::new(f(0), f(1), f(2)) * (v / 10.0);
    }
    if vol.abs() < 1e-12 {
        return DVec3::ZERO;
    }
    let s = sq / vol;
    DVec3::new(s.y + s.z, s.x + s.z, s.x + s.y)
}

fn put_i32(o: &mut Vec<u8>, v: i32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_f32(o: &mut Vec<u8>, v: f64) {
    o.extend_from_slice(&(v as f32).to_le_bytes());
}

/// Index of the triangle a ray from `tri`'s centre along its inward normal leaves through.
fn pierce_index(points: &[DVec3], tris: &[([u32; 3], u8)], ti: usize) -> usize {
    let tri = |t: usize| {
        let [a, b, c] = tris[t].0.map(|i| points[i as usize]);
        (a, b, c)
    };
    let (a, b, c) = tri(ti);
    let n = (b - a).cross(c - a).normalize_or_zero();
    let origin = (a + b + c) / 3.0;
    let dir = -n;
    let mut best = (f64::MAX, (ti + tris.len() / 2) % tris.len());
    for j in 0..tris.len() {
        if j == ti {
            continue;
        }
        let (p, q, r) = tri(j);
        let nj = (q - p).cross(r - p);
        let denom = nj.dot(dir);
        if denom <= 1e-12 {
            continue;
        }
        let t = nj.dot(p - origin) / denom;
        if t > 1e-9 && t < best.0 {
            best = (t, j);
        }
    }
    best.1
}

/// Serializes one ledge (header + triangles); points are written separately.
fn write_ledge(out: &mut Vec<u8>, cv: &Convex, point_base: &[u32], points_offset_from_ledge: i32) -> Option<()> {
    let n = cv.tris.len();
    // Directed edge (start, end) -> edge unit index (tri*4 + 1 + k).
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    for (t, (tri, _)) in cv.tris.iter().enumerate() {
        for k in 0..3 {
            edges.insert((tri[k], tri[(k + 1) % 3]), t * 4 + 1 + k);
        }
    }
    let size = 16 + n * 16 + cv.points.len() * 16;
    put_i32(out, points_offset_from_ledge);
    put_i32(out, cv.brush as i32); // client data: brush index
    let flags: u32 = 1 << 2 | ((size as u32 / 16) << 8);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(n as i16).to_le_bytes());
    out.extend_from_slice(&0i16.to_le_bytes());
    for (t, (tri, mat)) in cv.tris.iter().enumerate() {
        let pierce = pierce_index(&cv.points, &cv.tris, t);
        let head: u32 = (t as u32 & 0xfff) | ((pierce as u32 & 0xfff) << 12) | ((*mat as u32 & 0x7f) << 24);
        out.extend_from_slice(&head.to_le_bytes());
        for k in 0..3 {
            let (s, e) = (tri[k], tri[(k + 1) % 3]);
            let this = t * 4 + 1 + k;
            let opp = *edges.get(&(e, s))?;
            let rel = opp as i32 - this as i32;
            let start = point_base[s as usize];
            let v: u32 = (start & 0xffff) | (((rel as u32) & 0x7fff) << 16);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    Some(())
}

struct TreeNode {
    center: DVec3,
    radius: f64,
    half: DVec3,
    ledge: Option<usize>,
    kids: Option<Box<(TreeNode, TreeNode)>>,
}

fn build_tree(items: &mut [(usize, DVec3, DVec3, DVec3)]) -> TreeNode {
    // items: (ledge, bbox min, bbox max, centre)
    let (mut min, mut max) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for it in items.iter() {
        min = min.min(it.1);
        max = max.max(it.2);
    }
    let center = (min + max) * 0.5;
    if items.len() == 1 {
        let radius = ((items[0].2 - items[0].1) * 0.5).length();
        return TreeNode { center, radius, half: (max - min) * 0.5, ledge: Some(items[0].0), kids: None };
    }
    let ext = max - min;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    items.sort_by(|a, b| a.3[axis].total_cmp(&b.3[axis]));
    let mid = items.len() / 2;
    let (l, r) = items.split_at_mut(mid);
    let a = build_tree(l);
    let b = build_tree(r);
    let radius = ((a.center - center).length() + a.radius).max((b.center - center).length() + b.radius);
    TreeNode { center, radius, half: (max - min) * 0.5, ledge: None, kids: Some(Box::new((a, b))) }
}

fn count_nodes(n: &TreeNode) -> usize {
    1 + n.kids.as_ref().map_or(0, |k| count_nodes(&k.0) + count_nodes(&k.1))
}

/// Writes nodes in preorder; `ledge_pos` holds each ledge's byte offset in the surface.
fn write_tree(out: &mut Vec<u8>, n: &TreeNode, surface_start: usize, ledge_pos: &[usize]) {
    let me = out.len();
    let right = n.kids.as_ref().map_or(0, |k| (1 + count_nodes(&k.0)) * 28);
    put_i32(out, right as i32);
    let ledge_off = n.ledge.map_or(0, |l| (surface_start + ledge_pos[l]) as i64 - me as i64);
    put_i32(out, ledge_off as i32);
    put_f32(out, n.center.x);
    put_f32(out, n.center.y);
    put_f32(out, n.center.z);
    put_f32(out, n.radius);
    for i in 0..3 {
        let v = if n.radius > 0.0 { (n.half[i] / n.radius * 250.0).ceil() } else { 0.0 };
        out.push(v.clamp(0.0, 255.0) as u8);
    }
    out.push(0);
    if let Some(k) = &n.kids {
        write_tree(out, &k.0, surface_start, ledge_pos);
        write_tree(out, &k.1, surface_start, ledge_pos);
    }
}

/// One VPHY solid (with its `int size` prefix). Returns (bytes, volume in in³).
fn compact_surface(convexes: &[Convex]) -> Option<(Vec<u8>, f64)> {
    if convexes.is_empty() {
        return None;
    }
    // Shared, deduplicated point array.
    let mut all_points: Vec<DVec3> = Vec::new();
    let mut lookup: HashMap<[i64; 3], u32> = HashMap::new();
    let mut bases: Vec<Vec<u32>> = Vec::new();
    for cv in convexes {
        let mut base = Vec::with_capacity(cv.points.len());
        for &p in &cv.points {
            let key = [(p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64, (p.z * 1e5).round() as i64];
            let i = *lookup.entry(key).or_insert_with(|| {
                all_points.push(p);
                (all_points.len() - 1) as u32
            });
            base.push(i);
        }
        bases.push(base);
    }
    // Mass properties of the union of ledges.
    let mut vol = 0.0;
    let mut center = DVec3::ZERO;
    for cv in convexes {
        let (v, c) = mass_props(&cv.points, &cv.tris);
        vol += v;
        center += c * v;
    }
    if vol.abs() > 1e-12 {
        center /= vol;
    }
    let mut inert = DVec3::ZERO;
    for cv in convexes {
        let (v, _) = mass_props(&cv.points, &cv.tris);
        inert += inertia(&cv.points, &cv.tris, center) * v;
    }
    if vol.abs() > 1e-12 {
        // IVP stores the per-mass inertia scaled by 1/sqrt(2) (matches Valve's compiler).
        inert /= vol * std::f64::consts::SQRT_2;
    }
    let radius = all_points.iter().map(|p| (*p - center).length()).fold(0.0, f64::max);

    let mut surf = vec![0u8; 48];
    let mut ledge_pos = Vec::new();
    let ledges_len: usize = convexes.iter().map(|c| 16 + c.tris.len() * 16).sum();
    let points_at = 48 + ledges_len;
    for (cv, base) in convexes.iter().zip(&bases) {
        let at = surf.len();
        ledge_pos.push(at);
        write_ledge(&mut surf, cv, base, (points_at - at) as i32)?;
    }
    for p in &all_points {
        put_f32(&mut surf, p.x);
        put_f32(&mut surf, p.y);
        put_f32(&mut surf, p.z);
        put_f32(&mut surf, 0.0);
    }
    let tree_at = surf.len();
    let mut items: Vec<(usize, DVec3, DVec3, DVec3)> = convexes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (mut mn, mut mx) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
            for &p in &c.points {
                mn = mn.min(p);
                mx = mx.max(p);
            }
            (i, mn, mx, (mn + mx) * 0.5)
        })
        .collect();
    let tree = build_tree(&mut items);
    write_tree(&mut surf, &tree, 0, &ledge_pos);
    let size = surf.len();
    // Surface header.
    let mut h = Vec::with_capacity(48);
    for v in [center.x, center.y, center.z, inert.x, inert.y, inert.z, radius] {
        put_f32(&mut h, v);
    }
    let bits: u32 = 205 | ((size as u32) << 8);
    h.extend_from_slice(&bits.to_le_bytes());
    put_i32(&mut h, tree_at as i32);
    put_i32(&mut h, 0);
    put_i32(&mut h, 0);
    h.extend_from_slice(b"IVPS");
    surf[..48].copy_from_slice(&h);

    let mut solid = Vec::with_capacity(size + 32);
    put_i32(&mut solid, (size + 28) as i32);
    solid.extend_from_slice(b"VPHY");
    solid.extend_from_slice(&0x100i16.to_le_bytes());
    solid.extend_from_slice(&0i16.to_le_bytes());
    put_i32(&mut solid, size as i32);
    for _ in 0..3 {
        put_f32(&mut solid, 1.0);
    }
    put_i32(&mut solid, 0);
    solid.extend_from_slice(&surf);
    let vol_in3 = vol / METERS_PER_INCH.powi(3);
    Some((solid, vol_in3))
}

/// Builds the PHYSCOLLIDE lump for the given models (`(model index, entity)`).
pub fn build(lv: &Level, models: &[(usize, usize)], ctx: &crate::ctx::Ctx) -> Vec<u8> {
    let mut out = Vec::new();
    for &(mi, ent) in models {
        let brushes = &lv.entities[ent].brushes;
        let mut solids: Vec<Vec<u8>> = Vec::new();
        let mut keydata = String::new();
        if mi == 0 {
            let mut table: Vec<String> = Vec::new();
            let mut mat = |name: &str| -> u8 {
                let n = name.to_ascii_lowercase();
                let i = match table.iter().position(|t| *t == n) {
                    Some(i) => i,
                    None => {
                        table.push(n);
                        table.len() - 1
                    }
                };
                (i + 1).min(127) as u8
            };
            let groups: [(i32, &dyn Fn(i32) -> bool); 4] = [
                (WORLD_SOLID_MASK, &|c| c & (contents::SOLID | contents::WINDOW) != 0),
                (contents::GRATE, &|c| c & contents::GRATE != 0 && c & (contents::SOLID | contents::WINDOW) == 0),
                (contents::PLAYERCLIP, &|c| c & contents::PLAYERCLIP != 0),
                (contents::MONSTERCLIP, &|c| c & contents::MONSTERCLIP != 0),
            ];
            for (mask, pick) in groups {
                let cvs: Vec<Convex> = brushes
                    .iter()
                    .filter(|&&b| pick(lv.brushes[b].contents) && lv.brushes[b].contents & (contents::WATER | contents::SLIME) == 0)
                    .filter_map(|&b| brush_convex(lv, b, 0.0, &mut mat))
                    .collect();
                if let Some((s, _)) = compact_surface(&cvs) {
                    keydata.push_str(&format!("staticsolid {{\n\"index\" \"{}\"\n\"contents\" \"{mask}\"\n}}\n", solids.len()));
                    solids.push(s);
                }
            }
            // Fluids: connected groups of water brushes.
            let water: Vec<usize> = brushes.iter().copied().filter(|&b| lv.brushes[b].contents & (contents::WATER | contents::SLIME) != 0).collect();
            for group in connected(lv, &water) {
                let cvs: Vec<Convex> = group.iter().filter_map(|&b| brush_convex(lv, b, 0.0, &mut mat)).collect();
                let top = group.iter().map(|&b| lv.brushes[b].bounds.max.z).fold(f64::MIN, f64::max);
                let c = lv.brushes[group[0]].contents;
                if let Some((s, _)) = compact_surface(&cvs) {
                    keydata.push_str(&format!(
                        "fluid {{\n\"index\" \"{}\"\n\"surfaceprop\" \"water\"\n\"damping\" \"0.010000\"\n\"contents\" \"{c}\"\n\"surfaceplane\" \"0.000000 0.000000 1.000000 {:.6} \"\n\"currentvelocity\" \"0.000000 0.000000 0.000000 \"\n}}\n",
                        solids.len(),
                        top
                    ));
                    solids.push(s);
                }
            }
            keydata.push_str("virtualterrain {}\nmaterialtable {\n");
            for (i, t) in table.iter().enumerate() {
                keydata.push_str(&format!("\"{t}\" \"{}\"\n", i + 1));
            }
            keydata.push_str("}\n");
        } else {
            let solid_mask = contents::SOLID | contents::WINDOW | contents::GRATE;
            let mut surfaceprop = String::from("default");
            let cvs: Vec<Convex> = brushes
                .iter()
                .filter(|&&b| lv.brushes[b].contents & solid_mask != 0)
                .filter_map(|&b| brush_convex(lv, b, MODEL_SHRINK, &mut |_| 0))
                .collect();
            if let Some(sp) = brushes.first().and_then(|&b| lv.brushes[b].sides.first()).and_then(|s| s.material.as_ref()) {
                if sp.surfaceprop != "default" && !sp.surfaceprop.is_empty() {
                    surfaceprop = sp.surfaceprop.clone();
                }
            }
            let Some((s, vol)) = compact_surface(&cvs) else { continue };
            let mass = vol * METERS_PER_INCH.powi(3) * DENSITY;
            keydata.push_str(&format!(
                "solid {{\n\"index\" \"0\"\n\"mass\" \"{mass:.6}\"\n\"surfaceprop\" \"{surfaceprop}\"\n\"volume\" \"{vol:.6}\"\n}}\n"
            ));
            solids.push(s);
        }
        if solids.is_empty() {
            continue;
        }
        let data_size: usize = solids.iter().map(Vec::len).sum();
        let mut kd = keydata.into_bytes();
        kd.push(0);
        put_i32(&mut out, mi as i32);
        put_i32(&mut out, data_size as i32);
        put_i32(&mut out, kd.len() as i32);
        put_i32(&mut out, solids.len() as i32);
        for s in &solids {
            out.extend_from_slice(s);
        }
        out.extend_from_slice(&kd);
    }
    let _ = ctx;
    for v in [-1, -1, 0, 0] {
        put_i32(&mut out, v);
    }
    out
}

/// Groups brushes whose bounds touch.
fn connected(lv: &Level, list: &[usize]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..list.len()).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    for i in 0..list.len() {
        for j in i + 1..list.len() {
            let (a, b) = (&lv.brushes[list[i]].bounds, &lv.brushes[list[j]].bounds);
            let grow = crate::math::Aabb { min: a.min - DVec3::splat(0.1), max: a.max + DVec3::splat(0.1) };
            if grow.intersects(b) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..list.len() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(list[i]);
    }
    let mut g: Vec<Vec<usize>> = groups.into_values().collect();
    g.sort();
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(h: f64) -> Convex {
        let mut points = Vec::new();
        for i in 0..8 {
            points.push(DVec3::new(if i & 1 != 0 { h } else { -h }, if i & 2 != 0 { h } else { -h }, if i & 4 != 0 { h } else { -h }));
        }
        // Outward CCW quads.
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut tris = Vec::new();
        for q in quads {
            tris.push(([q[0], q[1], q[2]], 0));
            tris.push(([q[0], q[2], q[3]], 0));
        }
        Convex { brush: 0, points, tris }
    }

    #[test]
    fn cube_mass_properties() {
        let c = cube(0.2413);
        let (v, ctr) = mass_props(&c.points, &c.tris);
        assert!((v - 0.4826f64.powi(3)).abs() < 1e-9 && ctr.length() < 1e-12);
        let i = inertia(&c.points, &c.tris, ctr);
        assert!((i.x - 2.0 * 0.4826f64.powi(2) / 12.0).abs() < 1e-9);
        let (bytes, _) = compact_surface(&[c]).unwrap();
        // size prefix + VPHY header + surface (48 + 16 + 12*16 + 8*16 + 28).
        assert_eq!(bytes.len(), 4 + 28 + 412);
    }
}
