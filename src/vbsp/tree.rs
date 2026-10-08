//! BSP tree construction (qbsp3 `BrushBSP` / `BuildTree_r` / `BlockTree`).
//!
//! Subtrees are built in parallel (rayon) as owned trees, then flattened into an arena
//! (`Tree`) that the portal and face stages work on.

use super::brush::*;
use crate::flags::{contents, surf};
use crate::math::{Aabb, PlaneSet};
use glam::DVec3;
use std::collections::HashMap;

pub const BLOCK_SIZE: f64 = 1024.0;

pub enum BNode {
    Leaf { brushes: Vec<BspBrush>, contents: i32 },
    Split { plane: usize, detail_sep: bool, kids: Box<(BNode, BNode)> },
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    /// Even plane number for decision nodes, `None` for leaves.
    pub plane: Option<usize>,
    pub children: [usize; 2],
    pub parent: Option<usize>,
    pub detail_sep: bool,
    pub contents: i32,
    /// Leaf brush fragments (structural).
    pub brushes: Vec<BspBrush>,
    /// Original brushes overlapping the leaf (structural and detail), for the brush lists.
    pub leaf_brushes: Vec<usize>,
    pub portals: Vec<usize>,
    /// BFS distance from an entity (0 = unreached).
    pub occupied: u32,
    pub occupant: Option<usize>,
    pub area: i32,
    pub cluster: i32,
    pub faces: Vec<usize>,
    /// Faces seen from this leaf (portal faces and filtered detail faces).
    pub mark_faces: Vec<usize>,
    pub bounds: Aabb,
    /// LEAFWATERDATA index of a water leaf.
    pub water_id: Option<i16>,
}

impl Node {
    pub fn is_leaf(&self) -> bool {
        self.plane.is_none()
    }
}

pub struct Tree {
    pub nodes: Vec<Node>,
    pub head: usize,
    pub outside: usize,
    pub bounds: Aabb,
}

impl Tree {
    pub fn from_bnode(root: BNode, bounds: Aabb) -> Tree {
        let mut nodes = Vec::new();
        let head = flatten(root, None, &mut nodes);
        let outside = nodes.len();
        nodes.push(Node { contents: 0, cluster: -1, ..Default::default() });
        Tree { nodes, head, outside, bounds }
    }

    /// Leaf containing `p`.
    pub fn leaf_for_point(&self, planes: &PlaneSet, p: DVec3) -> usize {
        let mut n = self.head;
        while let Some(pl) = self.nodes[n].plane {
            n = self.nodes[n].children[if planes[pl].distance(p) >= 0.0 { 0 } else { 1 }];
        }
        n
    }

    /// qbsp `PruneNodes`: decision nodes with two solid leaf children become one solid leaf.
    /// Returns the number of pruned nodes.
    pub fn prune(&mut self) -> usize {
        fn rec(t: &mut Tree, n: usize) -> usize {
            if t.nodes[n].is_leaf() {
                return 0;
            }
            let [a, b] = t.nodes[n].children;
            let mut c = rec(t, a) + rec(t, b);
            let solid = |x: &Node| x.is_leaf() && x.contents & contents::SOLID != 0;
            if solid(&t.nodes[a]) && solid(&t.nodes[b]) {
                let mut brushes = std::mem::take(&mut t.nodes[a].brushes);
                brushes.append(&mut t.nodes[b].brushes);
                let mut lb = std::mem::take(&mut t.nodes[a].leaf_brushes);
                for x in std::mem::take(&mut t.nodes[b].leaf_brushes) {
                    if !lb.contains(&x) {
                        lb.push(x);
                    }
                }
                let node = &mut t.nodes[n];
                node.plane = None;
                node.contents = contents::SOLID;
                node.detail_sep = false;
                node.brushes = brushes;
                node.leaf_brushes = lb;
                node.mark_faces.clear();
                c += 1;
            }
            c
        }
        let h = self.head;
        rec(self, h)
    }

    /// Leaves in depth-first order (front child first), the order they are emitted in.
    pub fn leaves(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![self.head];
        while let Some(n) = stack.pop() {
            if self.nodes[n].is_leaf() {
                out.push(n);
            } else {
                stack.push(self.nodes[n].children[1]);
                stack.push(self.nodes[n].children[0]);
            }
        }
        out
    }
}

fn flatten(b: BNode, parent: Option<usize>, nodes: &mut Vec<Node>) -> usize {
    let i = nodes.len();
    nodes.push(Node { parent, cluster: -1, ..Default::default() });
    match b {
        BNode::Leaf { brushes, contents } => {
            nodes[i].contents = contents;
            nodes[i].brushes = brushes;
        }
        BNode::Split { plane, detail_sep, kids } => {
            nodes[i].plane = Some(plane);
            nodes[i].detail_sep = detail_sep;
            let (f, bk) = *kids;
            let c0 = flatten(f, Some(i), nodes);
            let c1 = flatten(bk, Some(i), nodes);
            nodes[i].children = [c0, c1];
        }
    }
    i
}

/// qbsp3 `LeafNode`: a solid brush whose every side lies on a node fills the leaf.
fn leaf_node(brushes: Vec<BspBrush>) -> BNode {
    let mut c = 0;
    for b in &brushes {
        if b.contents & contents::SOLID != 0 && b.sides.iter().all(|s| s.on_node || s.bevel) {
            c = contents::SOLID;
            break;
        }
        c |= b.contents;
    }
    BNode::Leaf { brushes, contents: c }
}

/// qbsp3 `SelectSplitSide`. Returns the (even) plane and whether the node only separates
/// detail / non-visible geometry (`detail_seperator`). Leaves `side` set on every brush.
fn select_split(planes: &PlaneSet, brushes: &mut [BspBrush], volume: &BspBrush) -> Option<(usize, bool)> {
    let mut best: Option<usize> = None;
    let mut best_value = i64::MIN;
    let mut detail_sep = false;
    for pass in 0..4 {
        for bi in 0..brushes.len() {
            let detail = brushes[bi].contents & contents::DETAIL != 0;
            if (pass & 1 == 1) != detail {
                continue;
            }
            for si in 0..brushes[bi].sides.len() {
                let s = &brushes[bi].sides[si];
                if s.bevel || s.winding.is_none() || s.on_node || s.tested || s.surf & surf::SKIP != 0 {
                    continue;
                }
                if s.visible != (pass < 2) {
                    continue;
                }
                let pnum = s.plane & !1;
                let is_hint = s.surf & surf::HINT != 0;
                let (vf, vb) = volume.split(planes, pnum);
                if vf.is_none() || vb.is_none() {
                    continue;
                }
                let (mut front, mut back, mut facing, mut splits) = (0i64, 0i64, 0i64, 0i64);
                let mut epsilonbrush = 0;
                let mut hint_split_any = false;
                for t in brushes.iter_mut() {
                    let (mut bs, mut hs) = (0, false);
                    let r = t.test_plane(planes, pnum, &mut bs, &mut hs, &mut epsilonbrush);
                    splits += bs as i64;
                    hint_split_any |= hs;
                    t.testside = r;
                    if r & PSIDE_FACING != 0 {
                        facing += 1;
                        for ts in &mut t.sides {
                            if ts.plane & !1 == pnum {
                                ts.tested = true;
                            }
                        }
                    }
                    if r & PSIDE_FRONT != 0 {
                        front += 1;
                    }
                    if r & PSIDE_BACK != 0 {
                        back += 1;
                    }
                }
                let mut value = 5 * facing - 5 * splits - (front - back).abs();
                if planes[pnum].kind() < 3 {
                    value += 5;
                }
                value -= epsilonbrush as i64 * 1000;
                if hint_split_any && !is_hint {
                    value = -9_999_999;
                }
                if value > best_value {
                    best_value = value;
                    best = Some(pnum);
                    for t in brushes.iter_mut() {
                        t.side = t.testside;
                    }
                }
            }
        }
        if best.is_some() {
            detail_sep = pass > 0;
            break;
        }
    }
    for b in brushes.iter_mut() {
        for s in &mut b.sides {
            s.tested = false;
        }
    }
    best.map(|p| (p, detail_sep))
}

fn build_r(planes: &PlaneSet, mut brushes: Vec<BspBrush>, volume: BspBrush) -> BNode {
    let Some((pnum, detail_sep)) = select_split(planes, &mut brushes, &volume) else {
        return leaf_node(brushes);
    };
    let (front, back) = split_list(planes, brushes, pnum);
    let (vf, vb) = volume.split(planes, pnum);
    let (Some(vf), Some(vb)) = (vf, vb) else {
        // Cannot happen: the plane was checked against the volume.
        let mut all = front;
        all.extend(back);
        return leaf_node(all);
    };
    let big = front.len() + back.len() > 8;
    let (f, b) = if big {
        rayon::join(|| build_r(planes, front, vf), || build_r(planes, back, vb))
    } else {
        (build_r(planes, front, vf), build_r(planes, back, vb))
    };
    BNode::Split { plane: pnum, detail_sep, kids: Box::new((f, b)) }
}

/// Builds a tree from brushes inside the box `min..max` (whose six planes must exist).
pub fn brush_bsp(planes: &PlaneSet, brushes: Vec<BspBrush>, min: DVec3, max: DVec3) -> BNode {
    if brushes.is_empty() {
        return BNode::Leaf { brushes, contents: 0 };
    }
    let volume = BspBrush::from_bounds(planes, min, max);
    build_r(planes, brushes, volume)
}

/// Planes needed by `brush_bsp` volumes and block separators, created up front so tree
/// building can share the plane set read-only.
pub fn ensure_box_planes(planes: &mut PlaneSet, min: DVec3, max: DVec3) {
    for i in 0..3 {
        let mut n = DVec3::ZERO;
        n[i] = 1.0;
        planes.find(crate::math::Plane::new(n, max[i]));
        planes.find(crate::math::Plane::new(n, min[i]));
    }
}

/// Block range covering the bounds.
pub fn block_range(b: &Aabb) -> (i32, i32, i32, i32) {
    (
        (b.min.x / BLOCK_SIZE).floor() as i32,
        (b.min.y / BLOCK_SIZE).floor() as i32,
        (b.max.x / BLOCK_SIZE).floor() as i32,
        (b.max.y / BLOCK_SIZE).floor() as i32,
    )
}

pub fn block_bounds(x: i32, y: i32) -> (DVec3, DVec3) {
    let z = crate::math::MAX_COORD;
    (
        DVec3::new(x as f64 * BLOCK_SIZE, y as f64 * BLOCK_SIZE, -z),
        DVec3::new((x + 1) as f64 * BLOCK_SIZE, (y + 1) as f64 * BLOCK_SIZE, z),
    )
}

/// qbsp3 `BlockTree`: joins per-block trees with axial separator nodes.
pub fn block_tree(planes: &PlaneSet, blocks: &mut HashMap<(i32, i32), BNode>, xl: i32, yl: i32, xh: i32, yh: i32) -> BNode {
    if xl == xh && yl == yh {
        return blocks.remove(&(xl, yl)).unwrap_or(BNode::Leaf { brushes: Vec::new(), contents: contents::SOLID });
    }
    let plane_at = |axis: usize, d: f64| {
        let mut n = DVec3::ZERO;
        n[axis] = 1.0;
        planes.lookup(crate::math::Plane::new(n, d)).expect("block plane")
    };
    if xh - xl > yh - yl {
        let mid = xl + (xh - xl) / 2 + 1;
        let p = plane_at(0, mid as f64 * BLOCK_SIZE);
        let f = block_tree(planes, blocks, mid, yl, xh, yh);
        let b = block_tree(planes, blocks, xl, yl, mid - 1, yh);
        BNode::Split { plane: p, detail_sep: false, kids: Box::new((f, b)) }
    } else {
        let mid = yl + (yh - yl) / 2 + 1;
        let p = plane_at(1, mid as f64 * BLOCK_SIZE);
        let f = block_tree(planes, blocks, xl, mid, xh, yh);
        let b = block_tree(planes, blocks, xl, yl, xh, mid - 1);
        BNode::Split { plane: p, detail_sep: false, kids: Box::new((f, b)) }
    }
}
