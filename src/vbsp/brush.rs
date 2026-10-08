//! Brush fragments used while building trees (qbsp3 `bspbrush_t`): splitting, plane tests,
//! volumes, clipping to boxes.

use super::load::Level;
use crate::flags::surf;
use crate::math::{Aabb, Plane, PlaneSet, Winding};
use glam::DVec3;

pub const PSIDE_FRONT: u8 = 1;
pub const PSIDE_BACK: u8 = 2;
pub const PSIDE_BOTH: u8 = PSIDE_FRONT | PSIDE_BACK;
pub const PSIDE_FACING: u8 = 4;

const PLANESIDE_EPSILON: f64 = 0.001;
const MIN_FRAGMENT_VOLUME: f64 = 0.01;

#[derive(Clone, Debug)]
pub struct BSide {
    pub plane: usize,
    pub winding: Option<Winding>,
    /// Index of the original map side; `None` for split ("mid") sides.
    pub orig: Option<usize>,
    /// The side lies on a node plane already used as a splitter (qbsp `TEXINFO_NODE`).
    pub on_node: bool,
    pub visible: bool,
    pub tested: bool,
    pub bevel: bool,
    pub surf: i32,
}

#[derive(Clone, Debug)]
pub struct BspBrush {
    /// Index into `Level::brushes` (`usize::MAX` for volumes).
    pub original: usize,
    pub contents: i32,
    pub sides: Vec<BSide>,
    pub bounds: Aabb,
    pub side: u8,
    pub testside: u8,
}

impl BspBrush {
    pub fn from_map(lv: &Level, bi: usize) -> BspBrush {
        let mb = &lv.brushes[bi];
        let sides = mb
            .sides
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.bevel)
            .map(|(i, s)| BSide {
                plane: s.plane,
                winding: s.winding.clone(),
                orig: Some(i),
                on_node: false,
                visible: s.visible || s.surf & surf::HINT != 0,
                tested: false,
                bevel: s.bevel,
                surf: s.surf,
            })
            .collect();
        BspBrush { original: bi, contents: mb.contents, sides, bounds: mb.bounds, side: 0, testside: 0 }
    }

    /// A box brush (node volumes).
    pub fn from_bounds(planes: &PlaneSet, min: DVec3, max: DVec3) -> BspBrush {
        let mut sides = Vec::with_capacity(6);
        for i in 0..3 {
            let mut n = DVec3::ZERO;
            n[i] = 1.0;
            sides.push(planes.lookup(Plane::new(n, max[i])).expect("box plane"));
            n[i] = -1.0;
            sides.push(planes.lookup(Plane::new(n, -min[i])).expect("box plane"));
        }
        let mut b = BspBrush {
            original: usize::MAX,
            contents: 0,
            sides: sides
                .into_iter()
                .map(|p| BSide { plane: p, winding: None, orig: None, on_node: false, visible: false, tested: false, bevel: false, surf: 0 })
                .collect(),
            bounds: Aabb::EMPTY,
            side: 0,
            testside: 0,
        };
        b.create_windings(planes);
        b
    }

    pub fn create_windings(&mut self, planes: &PlaneSet) {
        let ps: Vec<(usize, bool)> = self.sides.iter().map(|s| (s.plane, s.bevel)).collect();
        for (i, s) in self.sides.iter_mut().enumerate() {
            let mut w = Some(Winding::base(&planes[s.plane]));
            for (j, &(pj, bevel)) in ps.iter().enumerate() {
                if i == j || bevel {
                    continue;
                }
                w = w.and_then(|w| w.clip(&planes[pj ^ 1], 0.0, false));
            }
            s.winding = w;
        }
        self.bound();
    }

    pub fn bound(&mut self) {
        let mut b = Aabb::EMPTY;
        for s in &self.sides {
            if let Some(w) = &s.winding {
                for &p in &w.points {
                    b.add(p);
                }
            }
        }
        self.bounds = b;
    }

    pub fn volume(&self, planes: &PlaneSet) -> f64 {
        let Some(corner) = self.sides.iter().find_map(|s| s.winding.as_ref().map(|w| w.points[0])) else { return 0.0 };
        let mut v = 0.0;
        for s in &self.sides {
            let Some(w) = &s.winding else { continue };
            let p = &planes[s.plane];
            let d = -p.distance(corner);
            v += d * w.area();
        }
        v / 3.0
    }

    /// Which side of `plane` the brush is mostly on (by its farthest point).
    pub fn mostly_on_side(&self, plane: &Plane) -> u8 {
        let mut max = 0.0;
        let mut side = PSIDE_FRONT;
        for s in &self.sides {
            let Some(w) = &s.winding else { continue };
            for &p in &w.points {
                let d = plane.distance(p);
                if d > max {
                    max = d;
                    side = PSIDE_FRONT;
                }
                if -d > max {
                    max = -d;
                    side = PSIDE_BACK;
                }
            }
        }
        side
    }

    /// qbsp3 `SplitBrush`.
    pub fn split(&self, planes: &PlaneSet, planenum: usize) -> (Option<BspBrush>, Option<BspBrush>) {
        let plane = &planes[planenum];
        let (mut d_front, mut d_back) = (0.0f64, 0.0f64);
        for s in &self.sides {
            if let Some(w) = &s.winding {
                for &p in &w.points {
                    let d = plane.distance(p);
                    if d > 0.0 && d > d_front {
                        d_front = d;
                    }
                    if d < 0.0 && d < d_back {
                        d_back = d;
                    }
                }
            }
        }
        if d_front < 0.1 {
            return (None, Some(self.clone()));
        }
        if d_back > -0.1 {
            return (Some(self.clone()), None);
        }
        // Winding on the split plane, inside the brush.
        let mut mid = Some(Winding::base(plane));
        for s in &self.sides {
            if s.bevel {
                continue;
            }
            mid = mid.and_then(|w| w.clip(&planes[s.plane ^ 1], 0.0, false));
        }
        let mid = match mid {
            Some(w) if !w.is_tiny() => w,
            _ => {
                return match self.mostly_on_side(plane) {
                    PSIDE_FRONT => (Some(self.clone()), None),
                    _ => (None, Some(self.clone())),
                };
            }
        };
        let mut b = [self.empty_copy(), self.empty_copy()];
        for s in &self.sides {
            let Some(w) = &s.winding else { continue };
            let (f, bk) = w.split(plane, 0.0);
            for (j, cw) in [f, bk].into_iter().enumerate() {
                if let Some(cw) = cw {
                    let mut ns = s.clone();
                    ns.winding = Some(cw);
                    ns.tested = false;
                    b[j].sides.push(ns);
                }
            }
        }
        let mut ok = [true, true];
        for i in 0..2 {
            b[i].bound();
            if b[i].sides.len() < 3 || b[i].bounds.min.min_element() < -crate::math::MAX_COORD * 2.0 || b[i].bounds.max.max_element() > crate::math::MAX_COORD * 2.0 {
                ok[i] = false;
            }
        }
        if !(ok[0] && ok[1]) {
            return match (ok[0], ok[1]) {
                (true, false) => (Some(self.clone()), None),
                (false, true) => (None, Some(self.clone())),
                _ => (None, None),
            };
        }
        for i in 0..2 {
            b[i].sides.push(BSide {
                plane: planenum ^ i ^ 1,
                winding: Some(if i == 0 { mid.clone() } else { mid.reversed() }),
                orig: None,
                on_node: true,
                visible: false,
                tested: false,
                bevel: false,
                surf: 0,
            });
        }
        let [b0, b1] = b;
        // qbsp drops fragments under 1 unit^3, which opens leaks along slivers between
        // overlapping angled brushes; only truly degenerate pieces are dropped here.
        let keep = |x: BspBrush| (x.volume(planes) >= MIN_FRAGMENT_VOLUME).then_some(x);
        (keep(b0), keep(b1))
    }

    fn empty_copy(&self) -> BspBrush {
        BspBrush { original: self.original, contents: self.contents, sides: Vec::with_capacity(self.sides.len() + 1), bounds: Aabb::EMPTY, side: 0, testside: 0 }
    }

    /// qbsp3 `TestBrushToPlanenum`: returns PSIDE flags, counting visible faces split.
    pub fn test_plane(&self, planes: &PlaneSet, planenum: usize, splits: &mut i32, hintsplit: &mut bool, epsilonbrush: &mut i32) -> u8 {
        *splits = 0;
        *hintsplit = false;
        for s in &self.sides {
            if s.plane == planenum {
                return PSIDE_BACK | PSIDE_FACING;
            }
            if s.plane == planenum ^ 1 {
                return PSIDE_FRONT | PSIDE_FACING;
            }
        }
        let plane = &planes[planenum];
        let s = box_on_plane_side(&self.bounds, plane);
        if s != PSIDE_BOTH {
            return s;
        }
        let (mut d_front, mut d_back) = (0.0f64, 0.0f64);
        for side in &self.sides {
            if side.on_node || !side.visible {
                continue;
            }
            let Some(w) = &side.winding else { continue };
            let (mut front, mut back) = (false, false);
            for &p in &w.points {
                let d = plane.distance(p);
                d_front = d_front.max(d);
                d_back = d_back.min(d);
                if d > 0.1 {
                    front = true;
                }
                if d < -0.1 {
                    back = true;
                }
            }
            if front && back && side.surf & surf::SKIP == 0 {
                *splits += 1;
                if side.surf & surf::HINT != 0 {
                    *hintsplit = true;
                }
            }
        }
        if (d_front > 0.0 && d_front < 1.0) || (d_back < 0.0 && d_back > -1.0) {
            *epsilonbrush += 1;
        }
        s
    }
}

pub fn box_on_plane_side(b: &Aabb, p: &Plane) -> u8 {
    let mut side = 0;
    let k = p.kind();
    if k < 3 {
        let k = k as usize;
        // Axial planes have a positive or negative unit normal.
        let (lo, hi) = if p.normal[k] > 0.0 { (b.min[k], b.max[k]) } else { (-b.max[k], -b.min[k]) };
        if hi > p.dist + PLANESIDE_EPSILON {
            side |= PSIDE_FRONT;
        }
        if lo < p.dist - PLANESIDE_EPSILON {
            side |= PSIDE_BACK;
        }
        return side;
    }
    let mut corners = [DVec3::ZERO; 2];
    for i in 0..3 {
        if p.normal[i] < 0.0 {
            corners[0][i] = b.min[i];
            corners[1][i] = b.max[i];
        } else {
            corners[1][i] = b.min[i];
            corners[0][i] = b.max[i];
        }
    }
    let d1 = p.distance(corners[0]);
    let d2 = p.distance(corners[1]);
    if d1 >= PLANESIDE_EPSILON {
        side = PSIDE_FRONT;
    }
    if d2 < PLANESIDE_EPSILON {
        side |= PSIDE_BACK;
    }
    side
}

impl Winding {
    /// qbsp `WindingIsTiny`: fewer than three edges longer than 0.2.
    pub fn is_tiny(&self) -> bool {
        let n = self.points.len();
        let mut edges = 0;
        for i in 0..n {
            if (self.points[(i + 1) % n] - self.points[i]).length() > 0.2 {
                edges += 1;
                if edges == 3 {
                    return false;
                }
            }
        }
        true
    }
}

/// Splits a brush list by a node plane (qbsp3 `SplitBrushList`), using the `side` computed by
/// the split selection.
pub fn split_list(planes: &PlaneSet, brushes: Vec<BspBrush>, planenum: usize) -> (Vec<BspBrush>, Vec<BspBrush>) {
    let mut front = Vec::new();
    let mut back = Vec::new();
    for mut b in brushes {
        let sides = b.side;
        if sides == PSIDE_BOTH {
            let (f, bk) = b.split(planes, planenum);
            front.extend(f);
            back.extend(bk);
            continue;
        }
        if sides & PSIDE_FACING != 0 {
            for s in &mut b.sides {
                if s.plane & !1 == planenum {
                    s.on_node = true;
                }
            }
        }
        if sides & PSIDE_FRONT != 0 {
            front.push(b);
        } else if sides & PSIDE_BACK != 0 {
            back.push(b);
        }
    }
    (front, back)
}

/// Cuts away everything outside an axial box (qbsp3 `ClipBrushToBox`), marking sides on the
/// box planes as non-visible node sides.
pub fn clip_to_box(planes: &PlaneSet, mut b: BspBrush, min: DVec3, max: DVec3) -> Option<BspBrush> {
    let mut box_planes = Vec::new();
    for j in 0..2 {
        let mut n = DVec3::ZERO;
        n[j] = 1.0;
        let pmax = planes.lookup(Plane::new(n, max[j])).expect("block plane");
        let pmin = planes.lookup(Plane::new(n, min[j])).expect("block plane");
        box_planes.push(pmax & !1);
        box_planes.push(pmin & !1);
        if b.bounds.max[j] > max[j] {
            b = b.split(planes, pmax).1?;
        }
        if b.bounds.min[j] < min[j] {
            b = b.split(planes, pmin).0?;
        }
    }
    for s in &mut b.sides {
        if box_planes.contains(&(s.plane & !1)) {
            s.on_node = true;
            s.visible = false;
        }
    }
    Some(b)
}
