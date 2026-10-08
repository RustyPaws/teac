//! Detail geometry (vbsp `MergeDetailTree`): detail brushes don't take part in the world
//! tree. Their visible sides (minus parts covered by other detail brushes) become faces that are
//! filtered into the world leaves, and all brushes are filtered into the leaves they touch for
//! the collision brush lists.

use super::brush::BspBrush;
use super::faces::{merge_list, subdivide, Face, Faces};
use super::load::Level;
use super::tree::Tree;
use crate::flags::contents;
use crate::math::{Plane, PlaneSet, Winding};

const ON_EPSILON: f64 = 0.1;

/// Parts of `w` (on plane `wplane`) outside the convex brush `b`.
fn clip_outside_brush(w: Winding, wplane: &Plane, b: &BspBrush, planes: &PlaneSet) -> Vec<Winding> {
    let mut outside = Vec::new();
    let mut rest = w;
    for s in &b.sides {
        if s.bevel {
            continue;
        }
        let p = &planes[s.plane];
        // Coplanar: touching opposite faces hide each other; a face on the same plane facing
        // the same way is outside.
        if rest.points.iter().all(|&q| p.distance(q).abs() <= ON_EPSILON) {
            if p.normal.dot(wplane.normal) > 0.0 {
                outside.push(rest);
                return outside;
            }
            continue;
        }
        let (f, bk) = rest.split(p, ON_EPSILON);
        if let Some(f) = f {
            outside.push(f);
        }
        match bk {
            Some(bk) => rest = bk,
            None => return outside,
        }
    }
    // What remains is inside the brush.
    outside
}

/// Visible faces of detail brushes (vbsp `ComputeVisibleBrushSides`).
pub fn detail_faces(lv: &Level, planes: &PlaneSet, brushes: &[usize], faces: &mut Faces, merge: bool, subdiv: bool, max_dim: f64) -> Vec<usize> {
    let frags: Vec<BspBrush> = brushes.iter().map(|&b| BspBrush::from_map(lv, b)).collect();
    let mut out = Vec::new();
    for (i, bb) in frags.iter().enumerate() {
        let mb = &lv.brushes[bb.original];
        for s in &bb.sides {
            let Some(oi) = s.orig else { continue };
            let side = &mb.sides[oi];
            let Some(w) = &s.winding else { continue };
            if side.bevel || side.texinfo < 0 {
                continue;
            }
            let wplane = planes[s.plane];
            let mut pieces = vec![w.clone()];
            for (j, other) in frags.iter().enumerate() {
                if i == j || pieces.is_empty() {
                    continue;
                }
                // Only opaque solids hide faces.
                if other.contents & contents::SOLID == 0 {
                    continue;
                }
                if !other.bounds.intersects(&bb.bounds) {
                    continue;
                }
                pieces = pieces.into_iter().flat_map(|p| clip_outside_brush(p, &wplane, other, planes)).collect();
            }
            for p in pieces {
                if p.is_tiny() {
                    continue;
                }
                out.push(faces.add(Face {
                    w: p,
                    plane: s.plane,
                    texinfo: side.texinfo,
                    contents: mb.contents,
                    side: Some((bb.original, oi)),
                    merged_sides: Vec::new(),
                    merged: None,
                    split: None,
                    output: -1,
                    verts: Vec::new(),
                    dispinfo: -1,
                    smoothing: side.smoothing,
                }));
            }
        }
    }
    if merge {
        merge_list(faces, &mut out, planes);
    }
    let mut finals = Vec::new();
    for f in out {
        if subdiv {
            subdivide(faces, f, lv, max_dim);
        }
        faces.finals(f, &mut finals);
    }
    finals
}

/// Pushes faces down the world tree; fragments in solid leaves are dropped, the rest become
/// leaf faces (`on_node = false`).
pub fn filter_faces(tree: &mut Tree, planes: &PlaneSet, faces: &mut Faces, list: Vec<usize>) -> Vec<usize> {
    let mut emitted = Vec::new();
    for f in list {
        let w = faces.list[f].w.clone();
        let mut stack = vec![(tree.head, w)];
        let mut first = true;
        while let Some((n, w)) = stack.pop() {
            let node = &tree.nodes[n];
            let Some(pl) = node.plane else {
                if node.contents & contents::SOLID != 0 {
                    continue;
                }
                let fi = if first {
                    first = false;
                    faces.list[f].w = w;
                    f
                } else {
                    let mut nf = faces.list[f].clone();
                    nf.w = w;
                    faces.add(nf)
                };
                tree.nodes[n].mark_faces.push(fi);
                emitted.push(fi);
                continue;
            };
            let p = planes[pl];
            let [c0, c1] = node.children;
            if w.points.iter().all(|&q| p.distance(q).abs() <= ON_EPSILON) {
                let same = planes[faces.list[f].plane].normal.dot(p.normal) > 0.0;
                stack.push((if same { c0 } else { c1 }, w));
                continue;
            }
            let (fw, bw) = w.split(&p, ON_EPSILON);
            if let Some(b) = bw {
                stack.push((c1, b));
            }
            if let Some(fr) = fw {
                stack.push((c0, fr));
            }
        }
        if first {
            // Entirely inside solid: never emitted.
            faces.list[f].w = Winding::default();
        }
    }
    emitted
}

/// Adds every brush to the brush list of each leaf it overlaps.
pub fn filter_brushes(tree: &mut Tree, planes: &PlaneSet, lv: &Level, brushes: &[usize]) {
    for &bi in brushes {
        let b = BspBrush::from_map(lv, bi);
        let mut stack = vec![(tree.head, b)];
        while let Some((n, b)) = stack.pop() {
            let node = &tree.nodes[n];
            let Some(pl) = node.plane else {
                if !tree.nodes[n].leaf_brushes.contains(&bi) {
                    tree.nodes[n].leaf_brushes.push(bi);
                }
                continue;
            };
            let [c0, c1] = node.children;
            let (f, bk) = b.split(planes, pl);
            if let Some(f) = f {
                stack.push((c0, f));
            }
            if let Some(bk) = bk {
                stack.push((c1, bk));
            }
        }
    }
}
