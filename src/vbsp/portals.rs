//! Tree portals and everything that floods through them: entity flood / leak detection,
//! filling the outside, visible sides, areas, vis clusters and the `.prt` portal file.

use super::load::Level;
use super::tree::Tree;
use crate::flags::contents;
use crate::math::{Aabb, Plane, PlaneSet, Winding};
use glam::DVec3;
use std::collections::VecDeque;

const SIDESPACE: f64 = 8.0;
const BASE_WINDING_EPSILON: f64 = 0.001;
const SPLIT_WINDING_EPSILON: f64 = 0.001;

#[derive(Clone, Debug)]
pub struct Portal {
    pub plane: Plane,
    pub onnode: Option<usize>,
    pub nodes: [usize; 2],
    pub winding: Winding,
    pub sidefound: bool,
    /// (brush, side) of the face this portal shows.
    pub side: Option<(usize, usize)>,
    pub face: [Option<usize>; 2],
}

#[derive(Default)]
pub struct Portals {
    pub list: Vec<Portal>,
}

impl Portals {
    fn add(&mut self, tree: &mut Tree, p: Portal) -> usize {
        let i = self.list.len();
        tree.nodes[p.nodes[0]].portals.push(i);
        tree.nodes[p.nodes[1]].portals.push(i);
        self.list.push(p);
        i
    }

    fn link(&mut self, tree: &mut Tree, i: usize, front: usize, back: usize) {
        self.list[i].nodes = [front, back];
        tree.nodes[front].portals.push(i);
        tree.nodes[back].portals.push(i);
    }

    fn unlink(&mut self, tree: &mut Tree, i: usize) {
        for n in self.list[i].nodes {
            tree.nodes[n].portals.retain(|&p| p != i);
        }
    }
}

/// Clears all portals of the tree.
pub fn free_portals(tree: &mut Tree) {
    for n in &mut tree.nodes {
        n.portals.clear();
    }
}

fn headnode_portals(tree: &mut Tree, ps: &mut Portals) {
    let b = Aabb { min: tree.bounds.min - DVec3::splat(SIDESPACE), max: tree.bounds.max + DVec3::splat(SIDESPACE) };
    let mut planes = Vec::new();
    for i in 0..3 {
        for j in 0..2 {
            let mut n = DVec3::ZERO;
            let pl = if j == 1 {
                n[i] = -1.0;
                Plane::new(n, -b.max[i])
            } else {
                n[i] = 1.0;
                Plane::new(n, b.min[i])
            };
            planes.push(pl);
        }
    }
    let mut windings: Vec<Option<Winding>> = planes.iter().map(|p| Some(Winding::base(p))).collect();
    for i in 0..6 {
        for j in 0..6 {
            if i != j {
                windings[i] = windings[i].take().and_then(|w| w.clip(&planes[j], 0.1, false));
            }
        }
    }
    let (head, outside) = (tree.head, tree.outside);
    tree.nodes[outside].portals.clear();
    tree.nodes[outside].contents = 0;
    for (pl, w) in planes.into_iter().zip(windings) {
        if let Some(w) = w {
            ps.add(tree, Portal { plane: pl, onnode: None, nodes: [head, outside], winding: w, sidefound: false, side: None, face: [None, None] });
        }
    }
}

fn base_winding_for_node(tree: &Tree, planes: &PlaneSet, node: usize) -> Option<Winding> {
    let mut w = Winding::base(&planes[tree.nodes[node].plane?]);
    let mut n = node;
    while let Some(p) = tree.nodes[n].parent {
        let pl = planes[tree.nodes[p].plane.unwrap()];
        let keep = if tree.nodes[p].children[0] == n { pl } else { pl.flipped() };
        w = w.clip(&keep, BASE_WINDING_EPSILON, false)?;
        n = p;
    }
    Some(w)
}

fn make_node_portal(tree: &mut Tree, ps: &mut Portals, planes: &PlaneSet, node: usize) {
    let Some(mut w) = base_winding_for_node(tree, planes, node) else { return };
    for &pi in &tree.nodes[node].portals {
        let p = &ps.list[pi];
        let keep = if p.nodes[0] == node { p.plane } else { p.plane.flipped() };
        match w.clip(&keep, 0.1, false) {
            Some(nw) => w = nw,
            None => return,
        }
    }
    if w.is_tiny() {
        return;
    }
    let n = &tree.nodes[node];
    let pl = planes[n.plane.unwrap()];
    let (c0, c1) = (n.children[0], n.children[1]);
    ps.add(tree, Portal { plane: pl, onnode: Some(node), nodes: [c0, c1], winding: w, sidefound: false, side: None, face: [None, None] });
}

fn split_node_portals(tree: &mut Tree, ps: &mut Portals, planes: &PlaneSet, node: usize) {
    let plane = planes[tree.nodes[node].plane.unwrap()];
    let [f, b] = tree.nodes[node].children;
    let list = std::mem::take(&mut tree.nodes[node].portals);
    for pi in list {
        let side = if ps.list[pi].nodes[0] == node { 0 } else { 1 };
        let other = ps.list[pi].nodes[1 - side];
        ps.unlink(tree, pi);
        let (mut fw, mut bw) = ps.list[pi].winding.split(&plane, SPLIT_WINDING_EPSILON);
        if fw.as_ref().is_some_and(|w| w.is_tiny()) {
            fw = None;
        }
        if bw.as_ref().is_some_and(|w| w.is_tiny()) {
            bw = None;
        }
        let order = |child: usize| if side == 0 { (child, other) } else { (other, child) };
        match (fw, bw) {
            (None, None) => {}
            (None, Some(_)) => {
                let (a, c) = order(b);
                ps.link(tree, pi, a, c);
            }
            (Some(_), None) => {
                let (a, c) = order(f);
                ps.link(tree, pi, a, c);
            }
            (Some(fw), Some(bw)) => {
                let mut np = ps.list[pi].clone();
                np.winding = bw;
                ps.list[pi].winding = fw;
                let (a, c) = order(f);
                ps.link(tree, pi, a, c);
                let (a, c) = order(b);
                np.nodes = [a, c];
                ps.add(tree, np);
            }
        }
    }
}

/// Bounds of every node from its portals (qbsp `CalcNodeBounds`), children included.
pub fn calc_bounds(tree: &mut Tree, ps: &Portals) {
    for n in 0..tree.nodes.len() {
        let mut b = Aabb::EMPTY;
        for &pi in &tree.nodes[n].portals {
            for &p in &ps.list[pi].winding.points {
                b.add(p);
            }
        }
        tree.nodes[n].bounds = b;
    }
    // Decision nodes cover their children.
    fn up(tree: &mut Tree, n: usize) -> Aabb {
        if tree.nodes[n].is_leaf() {
            return tree.nodes[n].bounds;
        }
        let [a, b] = tree.nodes[n].children;
        let u = up(tree, a).union(&up(tree, b)).union(&tree.nodes[n].bounds);
        tree.nodes[n].bounds = u;
        u
    }
    let h = tree.head;
    up(tree, h);
}

/// qbsp `MakeTreePortals`: portals between all leaves (stopping at detail separators when
/// `vis_only`, which makes cluster portals).
pub fn make_tree_portals(tree: &mut Tree, planes: &PlaneSet, vis_only: bool) -> Portals {
    free_portals(tree);
    let mut ps = Portals::default();
    headnode_portals(tree, &mut ps);
    let mut stack = vec![tree.head];
    while let Some(n) = stack.pop() {
        if tree.nodes[n].is_leaf() || (vis_only && tree.nodes[n].detail_sep) {
            continue;
        }
        make_node_portal(tree, &mut ps, planes, n);
        split_node_portals(tree, &mut ps, planes, n);
        let [c0, c1] = tree.nodes[n].children;
        stack.push(c1);
        stack.push(c0);
    }
    ps
}

/// Entity flood (qbsp `FloodEntities`), breadth-first so the leak path is the shortest one.
/// Returns true if some entity is inside and the outside was not reached.
pub fn flood_entities(tree: &mut Tree, ps: &Portals, planes: &PlaneSet, lv: &Level) -> bool {
    for n in &mut tree.nodes {
        n.occupied = 0;
        n.occupant = None;
    }
    let mut inside = false;
    let mut queue = VecDeque::new();
    for (ei, e) in lv.entities.iter().enumerate().skip(1) {
        // Compile-time entities (overlays, cubemaps, static props) are consumed before the
        // flood and can't hold the map open.
        if super::load::removed_class(e.classname()) {
            continue;
        }
        let Some(mut origin) = e.vec3("origin") else { continue };
        if origin == DVec3::ZERO {
            continue;
        }
        origin.z += 1.0;
        let tries: Vec<DVec3> = if e.is("info_player_start") {
            let mut v = Vec::new();
            for x in [-16.0, 0.0, 16.0] {
                for y in [-16.0, 0.0, 16.0] {
                    v.push(origin + DVec3::new(x, y, 0.0));
                }
            }
            v.sort_by(|a, b| (*a - origin).length().total_cmp(&(*b - origin).length()));
            v
        } else {
            vec![origin]
        };
        for o in tries {
            let leaf = tree.leaf_for_point(planes, o);
            if tree.nodes[leaf].contents & contents::SOLID != 0 {
                continue;
            }
            if tree.nodes[leaf].occupied == 0 {
                tree.nodes[leaf].occupied = 1;
                tree.nodes[leaf].occupant = Some(ei);
                queue.push_back(leaf);
            }
            inside = true;
            break;
        }
    }
    while let Some(n) = queue.pop_front() {
        let d = tree.nodes[n].occupied;
        for &pi in &tree.nodes[n].portals.clone() {
            let p = &ps.list[pi];
            let other = if p.nodes[0] == n { p.nodes[1] } else { p.nodes[0] };
            if tree.nodes[other].occupied != 0 {
                continue;
            }
            if (tree.nodes[p.nodes[0]].contents | tree.nodes[p.nodes[1]].contents) & contents::SOLID != 0 {
                continue;
            }
            tree.nodes[other].occupied = d + 1;
            tree.nodes[other].occupant = tree.nodes[n].occupant;
            queue.push_back(other);
        }
    }
    inside && tree.nodes[tree.outside].occupied == 0
}

/// Leak path from the outside back to the entity that reached it (for the `.lin` file).
pub fn leak_path(tree: &Tree, ps: &Portals, lv: &Level) -> (Vec<DVec3>, Option<usize>) {
    let mut pts = Vec::new();
    let mut node = tree.outside;
    while tree.nodes[node].occupied > 1 {
        let mut best = None;
        let mut next = tree.nodes[node].occupied;
        for &pi in &tree.nodes[node].portals {
            let p = &ps.list[pi];
            let other = if p.nodes[0] == node { p.nodes[1] } else { p.nodes[0] };
            let o = tree.nodes[other].occupied;
            if o != 0 && o < next {
                next = o;
                best = Some((pi, other));
            }
        }
        let Some((pi, other)) = best else { break };
        pts.push(ps.list[pi].winding.center());
        node = other;
    }
    let occ = tree.nodes[node].occupant;
    if let Some(o) = occ.and_then(|e| lv.entities[e].vec3("origin")) {
        pts.push(o);
    }
    (pts, occ)
}

/// Turns every leaf not reached by an entity into solid (qbsp `FillOutside`).
pub fn fill_outside(tree: &mut Tree) -> (usize, usize) {
    let (mut filled, mut inside) = (0, 0);
    for n in 0..tree.nodes.len() {
        if n == tree.outside || !tree.nodes[n].is_leaf() {
            continue;
        }
        if tree.nodes[n].occupied == 0 {
            if tree.nodes[n].contents & contents::SOLID == 0 {
                filled += 1;
            }
            tree.nodes[n].contents = contents::SOLID;
        } else {
            inside += 1;
        }
    }
    (filled, inside)
}

pub fn visible_contents(c: i32) -> i32 {
    let mut i = 1;
    while i <= contents::LAST_VISIBLE {
        if c & i != 0 {
            return i;
        }
        i <<= 1;
    }
    0
}

/// qbsp `FindPortalSide`: the brush side that a portal between different contents shows.
fn find_portal_side(tree: &Tree, ps: &mut Portals, planes: &PlaneSet, lv: &Level, pi: usize) {
    let (pn0, pn1, onnode) = (ps.list[pi].nodes[0], ps.list[pi].nodes[1], ps.list[pi].onnode);
    let vis = visible_contents(tree.nodes[pn0].contents ^ tree.nodes[pn1].contents);
    ps.list[pi].sidefound = true;
    if vis == 0 {
        return;
    }
    let Some(on) = onnode else { return };
    let planenum = tree.nodes[on].plane.unwrap();
    let pn = planes[planenum].normal;
    let mut best: Option<(usize, usize)> = None;
    let mut bestdot = 0.0;
    'outer: for j in 0..2 {
        let n = &tree.nodes[ps.list[pi].nodes[j]];
        for bb in &n.brushes {
            let brush = &lv.brushes[bb.original];
            if brush.contents & vis == 0 {
                continue;
            }
            for (si, s) in brush.sides.iter().enumerate() {
                if s.bevel {
                    continue;
                }
                if s.plane & !1 == planenum {
                    best = Some((bb.original, si));
                    break 'outer;
                }
                let dot = pn.dot(planes[s.plane & !1].normal);
                if dot > bestdot {
                    bestdot = dot;
                    best = Some((bb.original, si));
                }
            }
        }
    }
    ps.list[pi].side = best;
}

/// qbsp `MarkVisibleSides`: sides used by a portal between different contents are visible.
pub fn mark_visible_sides(tree: &Tree, ps: &mut Portals, lv: &mut Level, brushes: &[usize]) {
    let mut marks = Vec::new();
    {
        let lvr: &Level = lv;
        let planes = &lvr.planes;
        for n in 0..tree.nodes.len() {
            if n == tree.outside || !tree.nodes[n].is_leaf() || tree.nodes[n].contents == 0 {
                continue;
            }
            for &pi in &tree.nodes[n].portals {
                if ps.list[pi].onnode.is_none() {
                    continue;
                }
                if !ps.list[pi].sidefound {
                    find_portal_side(tree, ps, planes, lvr, pi);
                }
                if let Some(bs) = ps.list[pi].side {
                    marks.push(bs);
                }
            }
        }
    }
    for &b in brushes {
        for s in &mut lv.brushes[b].sides {
            s.visible = false;
        }
    }
    for (b, s) in marks {
        lv.brushes[b].sides[s].visible = true;
    }
}

/// Finds the portal sides for face generation without touching visibility flags.
pub fn find_all_portal_sides(tree: &Tree, ps: &mut Portals, planes: &PlaneSet, lv: &Level) {
    for pi in 0..ps.list.len() {
        if !ps.list[pi].sidefound {
            find_portal_side(tree, ps, planes, lv, pi);
        }
    }
}

/// What the area flood learned about one area portal entity.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortalAreas {
    pub areas: [i32; 2],
    /// A leaf of each area next to the portal.
    pub sample: [usize; 2],
    /// Touches more than two areas.
    pub extra: bool,
}

/// Assigns areas (qbsp `FloodAreas`). Area portal leaves bound areas and record the two areas
/// they separate in `portal_areas[entity]`. Returns the number of areas (excluding area 0).
pub fn flood_areas(tree: &mut Tree, ps: &Portals, lv: &Level, portal_areas: &mut [PortalAreas]) -> i32 {
    let mut c_areas = 0;
    let leaves: Vec<usize> = (0..tree.nodes.len()).filter(|&n| n != tree.outside && tree.nodes[n].is_leaf()).collect();
    for &start in &leaves {
        let n = &tree.nodes[start];
        if n.area != 0 || n.contents & contents::SOLID != 0 || n.occupied == 0 || n.contents & contents::AREAPORTAL != 0 {
            continue;
        }
        c_areas += 1;
        let mut stack = vec![start];
        tree.nodes[start].area = c_areas;
        while let Some(n) = stack.pop() {
            for &pi in &tree.nodes[n].portals.clone() {
                let p = &ps.list[pi];
                let other = if p.nodes[0] == n { p.nodes[1] } else { p.nodes[0] };
                if (tree.nodes[p.nodes[0]].contents | tree.nodes[p.nodes[1]].contents) & contents::SOLID != 0 {
                    continue;
                }
                if tree.nodes[other].contents & contents::AREAPORTAL != 0 {
                    // Record which areas this area portal entity touches, and a leaf on each
                    // side (to orient the portal plane).
                    if let Some(ent) = areaportal_entity(tree, lv, other) {
                        let pa = &mut portal_areas[ent];
                        if pa.areas[0] != c_areas && pa.areas[1] != c_areas {
                            if pa.areas[0] == 0 {
                                pa.areas[0] = c_areas;
                                pa.sample[0] = n;
                            } else if pa.areas[1] == 0 {
                                pa.areas[1] = c_areas;
                                pa.sample[1] = n;
                            } else {
                                pa.extra = true;
                            }
                        }
                    }
                    continue;
                }
                if tree.nodes[other].area != 0 || other == tree.outside {
                    continue;
                }
                tree.nodes[other].area = c_areas;
                stack.push(other);
            }
        }
    }
    // Area portal leaves take the first area they touch.
    for &l in &leaves {
        if tree.nodes[l].contents & contents::AREAPORTAL != 0 {
            if let Some(ent) = areaportal_entity(tree, lv, l) {
                tree.nodes[l].area = portal_areas[ent].areas[0];
            }
        }
    }
    c_areas
}

pub fn areaportal_entity(tree: &Tree, lv: &Level, leaf: usize) -> Option<usize> {
    tree.nodes[leaf].brushes.iter().find(|b| lv.brushes[b.original].contents & contents::AREAPORTAL != 0).map(|b| lv.brushes[b.original].entity)
}

/// Contents of a cluster (qbsp `ClusterContents`).
fn cluster_contents(tree: &Tree, n: usize) -> i32 {
    let node = &tree.nodes[n];
    if node.is_leaf() {
        return node.contents;
    }
    let c1 = cluster_contents(tree, node.children[0]);
    let c2 = cluster_contents(tree, node.children[1]);
    let mut c = c1 | c2;
    if c1 & contents::SOLID == 0 || c2 & contents::SOLID == 0 {
        c &= !contents::SOLID;
    }
    c
}

fn portal_vis_flood(tree: &Tree, p: &Portal) -> bool {
    if p.onnode.is_none() {
        return false;
    }
    let mut c1 = cluster_contents(tree, p.nodes[0]);
    let mut c2 = cluster_contents(tree, p.nodes[1]);
    if visible_contents(c1 ^ c2) == 0 {
        return true;
    }
    if c1 & (contents::TRANSLUCENT | contents::DETAIL) != 0 {
        c1 = 0;
    }
    if c2 & (contents::TRANSLUCENT | contents::DETAIL) != 0 {
        c2 = 0;
    }
    if (c1 | c2) & contents::SOLID != 0 {
        return false;
    }
    if c1 ^ c2 == 0 {
        return true;
    }
    visible_contents(c1 ^ c2) == 0
}

/// A vis portal between two clusters (for vvis).
#[derive(Clone, Debug)]
pub struct VisPortal {
    pub clusters: [i32; 2],
    pub winding: Winding,
}

#[derive(Clone, Debug, Default)]
pub struct PortalFile {
    pub num_clusters: usize,
    pub portals: Vec<VisPortal>,
}

/// Numbers clusters and collects the vis portals between them (qbsp `WritePortalFile`).
/// The tree's portals are replaced by cluster portals.
pub fn make_vis_portals(tree: &mut Tree, planes: &PlaneSet) -> PortalFile {
    let ps = make_tree_portals(tree, planes, true);
    // Number clusters.
    let mut num = 0;
    let mut cluster_roots = Vec::new();
    let mut stack = vec![tree.head];
    while let Some(n) = stack.pop() {
        let node = &tree.nodes[n];
        if !node.is_leaf() && !node.detail_sep {
            stack.push(node.children[1]);
            stack.push(node.children[0]);
            continue;
        }
        if node.is_leaf() && node.contents & contents::SOLID != 0 {
            tree.nodes[n].cluster = -1;
            continue;
        }
        set_cluster(tree, n, num);
        // Portals of a detail cluster attach to its root node.
        tree.nodes[n].cluster = num;
        cluster_roots.push(n);
        num += 1;
    }
    let mut out = PortalFile { num_clusters: num as usize, portals: Vec::new() };
    for &n in &cluster_roots {
        for &pi in &tree.nodes[n].portals {
            let p = &ps.list[pi];
            if p.nodes[0] != n || !portal_vis_flood(tree, p) {
                continue;
            }
            let w = &p.winding;
            let wn = w.plane().normal;
            let (a, b) = (tree.nodes[p.nodes[0]].cluster, tree.nodes[p.nodes[1]].cluster);
            if a < 0 || b < 0 {
                continue;
            }
            let clusters = if p.plane.normal.dot(wn) < 0.99 { [b, a] } else { [a, b] };
            out.portals.push(VisPortal { clusters, winding: w.clone() });
        }
    }
    free_portals(tree);
    out
}

/// Sets `c` on every leaf below `n` (solid leaves inside a detail cluster get -1).
fn set_cluster(tree: &mut Tree, n: usize, c: i32) {
    let mut stack = vec![n];
    while let Some(i) = stack.pop() {
        if tree.nodes[i].is_leaf() {
            tree.nodes[i].cluster = if tree.nodes[i].contents & contents::SOLID != 0 { -1 } else { c };
        } else {
            stack.extend(tree.nodes[i].children);
        }
    }
}

impl PortalFile {
    /// Text `.prt` file (PRT1).
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = format!("PRT1\n{}\n{}\n", self.num_clusters, self.portals.len());
        for p in &self.portals {
            let _ = write!(s, "{} {} {} ", p.winding.points.len(), p.clusters[0], p.clusters[1]);
            for v in &p.winding.points {
                let _ = write!(s, "({} {} {} ) ", fmt_float(v.x), fmt_float(v.y), fmt_float(v.z));
            }
            s.push('\n');
        }
        s
    }

    pub fn parse(text: &str) -> anyhow::Result<PortalFile> {
        use anyhow::{bail, Context};
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some("PRT1") {
            bail!("not a PRT1 file");
        }
        let num_clusters: usize = lines.next().context("missing cluster count")?.trim().parse()?;
        let num_portals: usize = lines.next().context("missing portal count")?.trim().parse()?;
        let mut portals = Vec::with_capacity(num_portals);
        for _ in 0..num_portals {
            let l = lines.next().context("truncated portal file")?;
            let clean = l.replace(['(', ')'], " ");
            let mut it = clean.split_whitespace();
            let n: usize = it.next().context("portal")?.parse()?;
            let a: i32 = it.next().context("portal")?.parse()?;
            let b: i32 = it.next().context("portal")?.parse()?;
            let mut pts = Vec::with_capacity(n);
            for _ in 0..n {
                let x: f64 = it.next().context("point")?.parse()?;
                let y: f64 = it.next().context("point")?.parse()?;
                let z: f64 = it.next().context("point")?.parse()?;
                pts.push(DVec3::new(x, y, z));
            }
            portals.push(VisPortal { clusters: [a, b], winding: Winding::new(pts) });
        }
        Ok(PortalFile { num_clusters, portals })
    }
}

fn fmt_float(v: f64) -> String {
    let r = v.round();
    if (v - r).abs() < 0.001 { format!("{}", r as i64) } else { format!("{v:.6}") }
}
