//! Triangle BVH (binned SAH) for shadow and gather rays. Single precision, flattened nodes,
//! Möller–Trumbore intersection.

use glam::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub v0: Vec3,
    pub e1: Vec3,
    pub e2: Vec3,
    /// Caller data (e.g. surface index).
    pub id: u32,
}

impl Tri {
    pub fn new(a: Vec3, b: Vec3, c: Vec3, id: u32) -> Tri {
        Tri { v0: a, e1: b - a, e2: c - a, id }
    }
    fn bounds(&self) -> (Vec3, Vec3) {
        let (a, b, c) = (self.v0, self.v0 + self.e1, self.v0 + self.e2);
        (a.min(b).min(c), a.max(b).max(c))
    }
    fn centroid(&self) -> Vec3 {
        self.v0 + (self.e1 + self.e2) / 3.0
    }
    /// Distance along the ray, with barycentrics.
    #[inline]
    fn intersect(&self, o: Vec3, d: Vec3, tmax: f32) -> Option<(f32, f32, f32)> {
        let p = d.cross(self.e2);
        let det = self.e1.dot(p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = o - self.v0;
        let u = s.dot(p) * inv;
        if !(-1e-5..=1.0 + 1e-5).contains(&u) {
            return None;
        }
        let q = s.cross(self.e1);
        let v = d.dot(q) * inv;
        if v < -1e-5 || u + v > 1.0 + 1e-5 {
            return None;
        }
        let t = self.e2.dot(q) * inv;
        (t > 1e-4 && t < tmax).then_some((t, u, v))
    }
}

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle; inner: right child (left child is next).
    index: u32,
    /// Triangles in a leaf, 0 for inner nodes.
    count: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    pub tris: Vec<Tri>,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub tri: u32,
    pub u: f32,
    pub v: f32,
}

#[inline]
fn ray_box(o: Vec3, inv: Vec3, min: Vec3, max: Vec3, tmax: f32) -> bool {
    let t1 = (min - o) * inv;
    let t2 = (max - o) * inv;
    let tmin = t1.min(t2).max_element().max(0.0);
    let tfar = t1.max(t2).min_element().min(tmax);
    tmin <= tfar
}

impl Bvh {
    pub fn build(mut tris: Vec<Tri>) -> Bvh {
        let mut nodes = Vec::with_capacity(tris.len() * 2);
        if tris.is_empty() {
            return Bvh { nodes, tris };
        }
        let n = tris.len();
        build_rec(&mut tris, 0, n, &mut nodes);
        Bvh { nodes, tris }
    }

    /// True if anything is hit before `tmax`.
    pub fn occluded(&self, o: Vec3, d: Vec3, tmax: f32) -> bool {
        self.trace(o, d, tmax, true, |_| true).is_some()
    }

    /// Closest hit.
    pub fn intersect(&self, o: Vec3, d: Vec3, tmax: f32) -> Option<Hit> {
        self.trace(o, d, tmax, false, |_| true)
    }

    /// Closest hit (or any hit with `any`) among triangles accepted by `filter`.
    pub fn trace(&self, o: Vec3, d: Vec3, tmax: f32, any: bool, filter: impl Fn(u32) -> bool) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::ONE / d;
        let mut best: Option<Hit> = None;
        let mut tmax = tmax;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp];
            let node = self.nodes[ni as usize];
            if !ray_box(o, inv, node.min, node.max, tmax) {
                continue;
            }
            if node.count > 0 {
                for i in node.index..node.index + node.count {
                    let tri = &self.tris[i as usize];
                    if let Some((t, u, v)) = tri.intersect(o, d, tmax) {
                        if !filter(tri.id) {
                            continue;
                        }
                        best = Some(Hit { t, tri: i, u, v });
                        if any {
                            return best;
                        }
                        tmax = t;
                    }
                }
            } else if sp + 2 <= stack.len() {
                // Left child directly follows its parent.
                stack[sp] = node.index;
                stack[sp + 1] = ni + 1;
                sp += 2;
            }
        }
        best
    }
}

const BINS: usize = 12;

fn build_rec(tris: &mut [Tri], start: usize, end: usize, nodes: &mut Vec<Node>) -> u32 {
    let me = nodes.len() as u32;
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let (mut cmin, mut cmax) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for t in &tris[start..end] {
        let (a, b) = t.bounds();
        min = min.min(a);
        max = max.max(b);
        let c = t.centroid();
        cmin = cmin.min(c);
        cmax = cmax.max(c);
    }
    nodes.push(Node { min, max, index: start as u32, count: (end - start) as u32 });
    let count = end - start;
    if count <= 4 {
        return me;
    }
    // Binned SAH along the widest centroid axis.
    let ext = cmax - cmin;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    if ext[axis] < 1e-6 {
        return me;
    }
    let mut bins = [(0usize, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); BINS];
    let scale = BINS as f32 / ext[axis];
    let bin_of = |t: &Tri| (((t.centroid()[axis] - cmin[axis]) * scale) as usize).min(BINS - 1);
    for t in &tris[start..end] {
        let b = &mut bins[bin_of(t)];
        let (a, c) = t.bounds();
        b.0 += 1;
        b.1 = b.1.min(a);
        b.2 = b.2.max(c);
    }
    let area = |mn: Vec3, mx: Vec3| {
        let d = (mx - mn).max(Vec3::ZERO);
        d.x * d.y + d.y * d.z + d.z * d.x
    };
    let mut best = (f32::MAX, 0);
    for split in 1..BINS {
        let (mut n0, mut a0, mut b0) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let (mut n1, mut a1, mut b1) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (i, b) in bins.iter().enumerate() {
            if b.0 == 0 {
                continue;
            }
            if i < split {
                n0 += b.0;
                a0 = a0.min(b.1);
                b0 = b0.max(b.2);
            } else {
                n1 += b.0;
                a1 = a1.min(b.1);
                b1 = b1.max(b.2);
            }
        }
        if n0 == 0 || n1 == 0 {
            continue;
        }
        let cost = n0 as f32 * area(a0, b0) + n1 as f32 * area(a1, b1);
        if cost < best.0 {
            best = (cost, split);
        }
    }
    if best.0 == f32::MAX || best.0 >= count as f32 * area(min, max) {
        return me;
    }
    // Partition.
    let mut i = start;
    let mut j = end;
    while i < j {
        if bin_of(&tris[i]) < best.1 {
            i += 1;
        } else {
            j -= 1;
            tris.swap(i, j);
        }
    }
    let mid = i;
    if mid == start || mid == end {
        return me;
    }
    nodes[me as usize].count = 0;
    build_rec(tris, start, mid, nodes);
    let right = build_rec(tris, mid, end, nodes);
    nodes[me as usize].index = right;
    me
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_and_misses() {
        let mut tris = Vec::new();
        for i in 0..100 {
            let z = i as f32 * 10.0;
            tris.push(Tri::new(Vec3::new(-1.0, -1.0, z), Vec3::new(1.0, -1.0, z), Vec3::new(0.0, 1.0, z), i));
        }
        let b = Bvh::build(tris);
        let h = b.intersect(Vec3::new(0.0, 0.0, -5.0), Vec3::Z, 1e9).unwrap();
        assert_eq!(b.tris[h.tri as usize].id, 0);
        assert!((h.t - 5.0).abs() < 1e-4);
        let h = b.intersect(Vec3::new(0.0, 0.0, 55.0), Vec3::Z, 1e9).unwrap();
        assert_eq!(b.tris[h.tri as usize].id, 6);
        assert!(!b.occluded(Vec3::new(5.0, 5.0, -5.0), Vec3::Z, 1e9));
        assert!(b.occluded(Vec3::new(0.0, 0.0, 991.0), -Vec3::Z, 2.0));
        assert!(!b.occluded(Vec3::new(0.0, 0.0, 991.0), -Vec3::Z, 0.5));
    }
}
