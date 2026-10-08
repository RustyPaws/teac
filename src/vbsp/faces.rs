//! Faces: creation from portals, merging, lightmap subdivision, vertex welding and
//! t-junction repair.

use super::load::Level;
use super::portals::{visible_contents, Portals};
use super::tree::Tree;
use crate::flags::{contents, surf};
use crate::math::{PlaneSet, Winding};
use glam::DVec3;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Face {
    pub w: Winding,
    /// Plane number; odd = the back side of the even plane.
    pub plane: usize,
    pub texinfo: i32,
    pub contents: i32,
    /// Original (brush, side).
    pub side: Option<(usize, usize)>,
    /// Further original sides merged into this face (overlays reference any of them).
    pub merged_sides: Vec<(usize, usize)>,
    pub merged: Option<usize>,
    pub split: Option<[usize; 2]>,
    /// Output face number (-1 = not emitted).
    pub output: i32,
    pub verts: Vec<u32>,
    pub dispinfo: i32,
    pub smoothing: u32,
}

impl Face {
    pub fn is_final(&self) -> bool {
        self.merged.is_none() && self.split.is_none()
    }
}

#[derive(Default)]
pub struct Faces {
    pub list: Vec<Face>,
}

impl Faces {
    pub fn add(&mut self, f: Face) -> usize {
        self.list.push(f);
        self.list.len() - 1
    }

    /// Final faces a face turned into (following merges and splits).
    pub fn finals(&self, mut f: usize, out: &mut Vec<usize>) {
        while let Some(m) = self.list[f].merged {
            f = m;
        }
        match self.list[f].split {
            Some([a, b]) => {
                self.finals(a, out);
                self.finals(b, out);
            }
            None => out.push(f),
        }
    }
}

/// qbsp `FaceFromPortal`.
fn face_from_portal(tree: &Tree, ps: &Portals, planes: &PlaneSet, lv: &Level, pi: usize, pside: usize) -> Option<Face> {
    let p = &ps.list[pi];
    let (b, s) = p.side?;
    let side = &lv.brushes[b].sides[s];
    let (c0, c1) = (tree.nodes[p.nodes[0]].contents, tree.nodes[p.nodes[1]].contents);
    let mine = tree.nodes[p.nodes[pside]].contents;
    if mine & contents::WINDOW != 0 && visible_contents(c0 ^ c1) == contents::WINDOW {
        return None; // don't show the insides of windows
    }
    let nodeplane = tree.nodes[p.onnode?].plane?;
    let _ = planes;
    let w = if pside == 1 { p.winding.reversed() } else { p.winding.clone() };
    Some(Face {
        w,
        plane: nodeplane | pside,
        texinfo: side.texinfo,
        contents: mine,
        side: Some((b, s)),
        merged_sides: Vec::new(),
        merged: None,
        split: None,
        output: -1,
        verts: Vec::new(),
        dispinfo: -1,
        smoothing: side.smoothing,
    })
}

/// qbsp `MakeFaces`: faces of every portal seen from a non-solid leaf, stored on the node the
/// portal lies on.
pub fn make_faces(tree: &mut Tree, ps: &mut Portals, planes: &PlaneSet, lv: &Level, faces: &mut Faces) {
    for n in 0..tree.nodes.len() {
        if n == tree.outside || !tree.nodes[n].is_leaf() || tree.nodes[n].contents & contents::SOLID != 0 {
            continue;
        }
        for pi in tree.nodes[n].portals.clone() {
            let s = if ps.list[pi].nodes[1] == n { 1 } else { 0 };
            let Some(on) = ps.list[pi].onnode else { continue };
            if let Some(f) = face_from_portal(tree, ps, planes, lv, pi, s) {
                let fi = faces.add(f);
                ps.list[pi].face[s] = Some(fi);
                tree.nodes[on].faces.push(fi);
                tree.nodes[n].mark_faces.push(fi);
            }
        }
    }
}

const CONTINUOUS_EPSILON: f64 = 0.005;
const EQUAL_EPSILON: f64 = 0.001;

/// Merges two coplanar convex windings sharing an edge, if the result is convex.
pub fn try_merge_winding(a: &Winding, b: &Winding, normal: DVec3) -> Option<Winding> {
    let (na, nb) = (a.points.len(), b.points.len());
    let eq = |p: DVec3, q: DVec3| (p - q).abs().max_element() <= EQUAL_EPSILON;
    let mut found = None;
    'f: for i in 0..na {
        let (p1, p2) = (a.points[i], a.points[(i + 1) % na]);
        for j in 0..nb {
            let (p3, p4) = (b.points[j], b.points[(j + 1) % nb]);
            if eq(p1, p4) && eq(p2, p3) {
                found = Some((i, j));
                break 'f;
            }
        }
    }
    let (i, j) = found?;
    let mut pts = Vec::with_capacity(na + nb);
    for k in 0..na {
        pts.push(a.points[(i + 1 + k) % na]);
    }
    for k in 0..nb.saturating_sub(2) {
        pts.push(b.points[(j + 2 + k) % nb]);
    }
    // Convexity (clockwise from the front: every turn has cross·normal <= 0) and removal of
    // the colinear points at the joins.
    let mut changed = true;
    while changed && pts.len() >= 3 {
        changed = false;
        let n = pts.len();
        for k in 0..n {
            let prev = pts[(k + n - 1) % n];
            let cur = pts[k];
            let next = pts[(k + 1) % n];
            let e1 = cur - prev;
            let e2 = next - cur;
            if e1.length() < 1e-6 || e2.length() < 1e-6 {
                pts.remove(k);
                changed = true;
                break;
            }
            let turn = e1.normalize().cross(e2.normalize()).dot(normal);
            if turn > CONTINUOUS_EPSILON {
                return None;
            }
            if turn.abs() <= CONTINUOUS_EPSILON && e1.normalize().dot(e2.normalize()) > 0.0 {
                pts.remove(k);
                changed = true;
                break;
            }
        }
    }
    (pts.len() >= 3).then(|| Winding::new(pts))
}

fn try_merge(faces: &mut Faces, a: usize, b: usize, normal: DVec3) -> Option<usize> {
    let (fa, fb) = (&faces.list[a], &faces.list[b]);
    if fa.texinfo != fb.texinfo || fa.plane != fb.plane || fa.contents != fb.contents || fa.smoothing != fb.smoothing || fa.dispinfo != -1 || fb.dispinfo != -1 {
        return None;
    }
    let w = try_merge_winding(&fa.w, &fb.w, normal)?;
    let mut nf = fa.clone();
    nf.w = w;
    let mut extra = fb.merged_sides.clone();
    extra.extend(fb.side);
    for x in extra {
        if Some(x) != nf.side && !nf.merged_sides.contains(&x) {
            nf.merged_sides.push(x);
        }
    }
    nf.merged = None;
    nf.split = None;
    let ni = faces.add(nf);
    faces.list[a].merged = Some(ni);
    faces.list[b].merged = Some(ni);
    Some(ni)
}

/// qbsp `MergeNodeFaces` on a list: merged faces are appended and retried.
pub fn merge_list(faces: &mut Faces, list: &mut Vec<usize>, planes: &PlaneSet) {
    let mut i = 0;
    while i < list.len() {
        let f1 = list[i];
        if !faces.list[f1].is_final() {
            i += 1;
            continue;
        }
        for j in 0..i {
            let f2 = list[j];
            if !faces.list[f2].is_final() {
                continue;
            }
            let pl = faces.list[f1].plane;
            let normal = if pl & 1 == 1 { -planes[pl & !1].normal } else { planes[pl].normal };
            if let Some(m) = try_merge(faces, f1, f2, normal) {
                list.push(m);
                break;
            }
        }
        i += 1;
    }
    list.retain(|&f| faces.list[f].is_final());
}

/// Splits faces whose lightmap would exceed `max_dim` luxels (vbsp `SubdivideFace`).
pub fn subdivide(faces: &mut Faces, f: usize, lv: &Level, max_dim: f64) {
    let ti = faces.list[f].texinfo;
    if ti < 0 {
        return;
    }
    let tex = &lv.texinfo.list[ti as usize];
    if tex.flags & (surf::NOCHOP | surf::NODRAW | surf::SKY | surf::SKY2D) != 0 || faces.list[f].dispinfo >= 0 {
        return;
    }
    for axis in 0..2 {
        let lv4 = tex.lightmap_vecs[axis];
        let temp = DVec3::new(lv4[0] as f64, lv4[1] as f64, lv4[2] as f64);
        let (mut mins, mut maxs) = (f64::MAX, f64::MIN);
        for &p in &faces.list[f].w.points {
            let v = p.dot(temp);
            mins = mins.min(v);
            maxs = maxs.max(v);
        }
        if maxs.ceil() - mins.floor() <= max_dim {
            continue;
        }
        let len = temp.length();
        if len < 1e-9 {
            return;
        }
        let dist = (mins + max_dim - 1.0) / len;
        let plane = crate::math::Plane::new(temp / len, dist);
        let (fw, bw) = faces.list[f].w.split(&plane, 0.1);
        let (Some(fw), Some(bw)) = (fw, bw) else { return };
        let mut a = faces.list[f].clone();
        a.w = fw;
        let mut b = faces.list[f].clone();
        b.w = bw;
        let ia = faces.add(a);
        let ib = faces.add(b);
        faces.list[f].split = Some([ia, ib]);
        subdivide(faces, ia, lv, max_dim);
        subdivide(faces, ib, lv, max_dim);
        return;
    }
}

/// Merges and subdivides the faces on every node.
pub fn merge_and_subdivide(tree: &mut Tree, faces: &mut Faces, planes: &PlaneSet, lv: &Level, merge: bool, subdiv: bool, max_dim: f64) {
    for n in 0..tree.nodes.len() {
        if tree.nodes[n].faces.is_empty() {
            continue;
        }
        let mut list = std::mem::take(&mut tree.nodes[n].faces);
        if merge {
            merge_list(faces, &mut list, planes);
        }
        if subdiv {
            for &f in &list.clone() {
                subdivide(faces, f, lv, max_dim);
            }
        }
        tree.nodes[n].faces = list;
    }
}

const INTEGRAL_EPSILON: f64 = 0.01;
const POINT_EPSILON: f64 = 0.01;
const VERT_CELL: f64 = 64.0;

/// Unique, welded vertices with a spatial hash for t-junction queries.
#[derive(Default)]
pub struct Vertices {
    pub list: Vec<DVec3>,
    grid: HashMap<(i64, i64, i64), Vec<u32>>,
}

fn cell(p: DVec3) -> (i64, i64, i64) {
    ((p.x / VERT_CELL).floor() as i64, (p.y / VERT_CELL).floor() as i64, (p.z / VERT_CELL).floor() as i64)
}

impl Vertices {
    pub fn get(&mut self, mut p: DVec3, weld: bool) -> u32 {
        for i in 0..3 {
            let r = p[i].round();
            if (p[i] - r).abs() < INTEGRAL_EPSILON {
                p[i] = r;
            }
        }
        let c = cell(p);
        if weld {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(list) = self.grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                            for &v in list {
                                if (self.list[v as usize] - p).abs().max_element() < POINT_EPSILON {
                                    return v;
                                }
                            }
                        }
                    }
                }
            }
        }
        let i = self.list.len() as u32;
        self.list.push(p);
        self.grid.entry(c).or_default().push(i);
        i
    }

    /// Vertices strictly inside the segment a-b (within `eps` of it), sorted along it.
    pub fn on_edge(&self, a: u32, b: u32, eps: f64) -> Vec<u32> {
        let (pa, pb) = (self.list[a as usize], self.list[b as usize]);
        let d = pb - pa;
        let len = d.length();
        if len < 0.1 {
            return Vec::new();
        }
        let dir = d / len;
        let lo = cell(pa.min(pb) - DVec3::splat(eps));
        let hi = cell(pa.max(pb) + DVec3::splat(eps));
        let mut hits: Vec<(f64, u32)> = Vec::new();
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for z in lo.2..=hi.2 {
                    let Some(list) = self.grid.get(&(x, y, z)) else { continue };
                    for &v in list {
                        if v == a || v == b {
                            continue;
                        }
                        let p = self.list[v as usize];
                        let t = (p - pa).dot(dir);
                        if t <= 0.1 || t >= len - 0.1 {
                            continue;
                        }
                        if (pa + dir * t - p).length() < eps {
                            hits.push((t, v));
                        }
                    }
                }
            }
        }
        hits.sort_by(|x, y| x.0.total_cmp(&y.0));
        hits.dedup_by_key(|h| h.1);
        hits.into_iter().map(|h| h.1).collect()
    }
}

/// Assigns welded vertex numbers to a final face, dropping repeated points.
pub fn emit_face_vertices(faces: &mut Faces, f: usize, verts: &mut Vertices, weld: bool) {
    let pts = faces.list[f].w.points.clone();
    let mut v: Vec<u32> = pts.into_iter().map(|p| verts.get(p, weld)).collect();
    v.dedup();
    while v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    faces.list[f].verts = v;
}

/// Inserts vertices of neighbouring faces lying on this face's edges (t-junctions).
pub fn fix_face_tjuncs(faces: &mut Faces, f: usize, verts: &Vertices) -> usize {
    let v = &faces.list[f].verts;
    if v.len() < 3 {
        return 0;
    }
    let mut out = Vec::with_capacity(v.len() + 4);
    let mut added = 0;
    for i in 0..v.len() {
        let (a, b) = (v[i], v[(i + 1) % v.len()]);
        out.push(a);
        let mids = verts.on_edge(a, b, 0.1);
        added += mids.len();
        out.extend(mids);
    }
    faces.list[f].verts = out;
    added
}

/// Edge sharing: an edge used once in the opposite direction by a face with the same
/// contents is shared (negative surfedge).
#[derive(Default)]
pub struct Edges {
    pub list: Vec<[u16; 2]>,
    open: HashMap<(u32, u32, i32), usize>,
}

impl Edges {
    pub fn new() -> Edges {
        // Edge 0 is unused: surfedge 0 could not be negated.
        Edges { list: vec![[0, 0]], open: HashMap::new() }
    }

    pub fn start_model(&mut self) {
        self.open.clear();
    }

    pub fn get(&mut self, a: u32, b: u32, contents: i32) -> i32 {
        if let Some(e) = self.open.remove(&(b, a, contents)) {
            return -(e as i32);
        }
        let e = self.list.len();
        self.list.push([a as u16, b as u16]);
        self.open.insert((a, b, contents), e);
        e as i32
    }
}
