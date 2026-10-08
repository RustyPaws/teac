//! Convex polygons ("windings") with vbsp-style clipping and splitting.

use super::{Plane, BOGUS_RANGE};
use glam::DVec3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Winding {
    pub points: Vec<DVec3>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Front,
    Back,
    On,
    Cross,
}

impl Winding {
    pub fn new(points: Vec<DVec3>) -> Winding {
        Winding { points }
    }

    /// A huge square on `plane`, wound clockwise seen from the front (vbsp `BaseWindingForPlane`).
    pub fn base(plane: &Plane) -> Winding {
        let n = plane.normal;
        let a = n.abs();
        let up = if a.z >= a.x && a.z >= a.y { DVec3::X } else { DVec3::Z };
        let up = (up - n * up.dot(n)).normalize();
        let org = n * plane.dist;
        let right = up.cross(n);
        let (up, right) = (up * BOGUS_RANGE, right * BOGUS_RANGE);
        Winding { points: vec![org - right + up, org + right + up, org + right - up, org - right - up] }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.len() < 3
    }

    pub fn area(&self) -> f64 {
        let mut total = 0.0;
        for i in 2..self.points.len() {
            let d1 = self.points[i - 1] - self.points[0];
            let d2 = self.points[i] - self.points[0];
            total += d1.cross(d2).length() * 0.5;
        }
        total
    }

    pub fn center(&self) -> DVec3 {
        self.points.iter().copied().sum::<DVec3>() / self.points.len().max(1) as f64
    }

    pub fn bounds(&self) -> super::Aabb {
        let mut b = super::Aabb::EMPTY;
        for &p in &self.points {
            b.add(p);
        }
        b
    }

    /// Plane of the winding (vbsp `WindingPlane`: points are clockwise seen from the front).
    pub fn plane(&self) -> Plane {
        let p = &self.points;
        let n = (p[2] - p[0]).cross(p[1] - p[0]).normalize_or_zero();
        Plane::new(n, n.dot(p[0]))
    }

    pub fn reversed(&self) -> Winding {
        Winding { points: self.points.iter().rev().copied().collect() }
    }

    pub fn classify(&self, plane: &Plane, eps: f64) -> Side {
        let (mut front, mut back) = (false, false);
        for &p in &self.points {
            let d = plane.distance(p);
            if d > eps {
                front = true;
            } else if d < -eps {
                back = true;
            }
        }
        match (front, back) {
            (true, true) => Side::Cross,
            (true, false) => Side::Front,
            (false, true) => Side::Back,
            _ => Side::On,
        }
    }

    /// Splits by `plane` into (front, back) (qbsp `ClipWindingEpsilon`). A winding lying on
    /// the plane goes to the back.
    pub fn split(&self, plane: &Plane, eps: f64) -> (Option<Winding>, Option<Winding>) {
        let n = self.points.len();
        let mut dists = Vec::with_capacity(n + 1);
        let mut sides = Vec::with_capacity(n + 1);
        let (mut cf, mut cb) = (0, 0);
        for &p in &self.points {
            let d = plane.distance(p);
            let s = if d > eps {
                cf += 1;
                1
            } else if d < -eps {
                cb += 1;
                -1
            } else {
                0
            };
            dists.push(d);
            sides.push(s);
        }
        if cf == 0 {
            return (None, Some(self.clone()));
        }
        if cb == 0 {
            return (Some(self.clone()), None);
        }
        let mut f = Vec::with_capacity(n + 4);
        let mut b = Vec::with_capacity(n + 4);
        for i in 0..n {
            let p1 = self.points[i];
            match sides[i] {
                0 => {
                    f.push(p1);
                    b.push(p1);
                    continue;
                }
                1 => f.push(p1),
                _ => b.push(p1),
            }
            let j = (i + 1) % n;
            if sides[j] == 0 || sides[j] == sides[i] {
                continue;
            }
            let p2 = self.points[j];
            let t = dists[i] / (dists[i] - dists[j]);
            let mut mid = p1 + (p2 - p1) * t;
            // Exact on axial planes avoids drift.
            for k in 0..3 {
                if plane.normal[k] == 1.0 {
                    mid[k] = plane.dist;
                } else if plane.normal[k] == -1.0 {
                    mid[k] = -plane.dist;
                }
            }
            f.push(mid);
            b.push(mid);
        }
        let mk = |v: Vec<DVec3>| (v.len() >= 3).then(|| Winding { points: v });
        (mk(f), mk(b))
    }

    /// Keeps the part in front of `plane` (qbsp `ChopWindingInPlace`); a coplanar winding is
    /// kept only with `keep_on`.
    pub fn clip(&self, plane: &Plane, eps: f64, keep_on: bool) -> Option<Winding> {
        if keep_on && self.points.iter().all(|&p| plane.distance(p).abs() <= eps) {
            return Some(self.clone());
        }
        self.split(plane, eps).0
    }

    /// Keeps the part behind `plane` (used to chop a base winding by a brush's other sides).
    pub fn chop(&self, plane: &Plane, eps: f64) -> Option<Winding> {
        self.clip(&plane.flipped(), eps, false)
    }

    /// Removes collinear and duplicate points.
    pub fn remove_colinear(&mut self) {
        let n = self.points.len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let prev = self.points[(i + n - 1) % n];
            let cur = self.points[i];
            let next = self.points[(i + 1) % n];
            let a = (cur - prev).normalize_or_zero();
            let b = (next - cur).normalize_or_zero();
            if a.dot(b) < 0.999 && (cur - prev).length() > 0.001 {
                out.push(cur);
            }
        }
        self.points = out;
    }

    /// True if all points are finite and inside the map.
    pub fn is_valid(&self) -> bool {
        self.points.iter().all(|p| p.is_finite() && p.abs().max_element() < super::MAX_COORD * 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Winding {
        Winding::new(vec![DVec3::new(0., 0., 0.), DVec3::new(0., 64., 0.), DVec3::new(64., 64., 0.), DVec3::new(64., 0., 0.)])
    }

    #[test]
    fn split_and_area() {
        let w = square();
        assert!((w.area() - 4096.0).abs() < 1e-9);
        let (f, b) = w.split(&Plane::new(DVec3::X, 16.0), 0.1);
        assert!((f.unwrap().area() - 3072.0).abs() < 1e-9);
        assert!((b.unwrap().area() - 1024.0).abs() < 1e-9);
        let (f, b) = w.split(&Plane::new(DVec3::Z, 0.0), 0.1);
        assert!(f.is_none() && b.is_some(), "coplanar goes to the back");
        assert!(w.clip(&Plane::new(DVec3::Z, 0.0), 0.1, false).is_none());
        assert!(w.clip(&Plane::new(DVec3::Z, 0.0), 0.1, true).is_some());
    }

    #[test]
    fn base_winding_matches_plane() {
        let p = Plane::new(DVec3::new(0.6, 0.0, 0.8), 10.0);
        let w = Winding::base(&p);
        let q = w.plane();
        assert!((q.normal - p.normal).length() < 1e-9 && (q.dist - p.dist).abs() < 1e-6);
    }
}
