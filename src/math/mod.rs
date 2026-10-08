//! Geometry core: Source angle conventions, planes, plane sets and windings.

pub mod winding;

pub use winding::Winding;

use glam::{DMat3, DVec3};

/// Largest map coordinate.
pub const MAX_COORD: f64 = 16384.0;
/// Size of the base winding created for a plane; must exceed the map diagonal.
pub const BOGUS_RANGE: f64 = MAX_COORD * 4.0;

pub const NORMAL_EPSILON: f64 = 0.00001;
pub const DIST_EPSILON: f64 = 0.01;
/// Points this close to a plane are "on" it when splitting windings.
pub const ON_EPSILON: f64 = 0.1;

/// Source `AngleMatrix`: columns are forward, left, up for (pitch, yaw, roll) in degrees.
pub fn angles_matrix(a: DVec3) -> DMat3 {
    let (sp, cp) = a.x.to_radians().sin_cos();
    let (sy, cy) = a.y.to_radians().sin_cos();
    let (sr, cr) = a.z.to_radians().sin_cos();
    DMat3::from_cols(
        DVec3::new(cp * cy, cp * sy, -sp),
        DVec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp),
        DVec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp),
    )
}

/// Source `MatrixAngles`: inverse of [`angles_matrix`], in degrees.
pub fn matrix_angles(m: DMat3) -> DVec3 {
    let (f, l, u) = (m.x_axis, m.y_axis, m.z_axis);
    let xy = (f.x * f.x + f.y * f.y).sqrt();
    let (pitch, yaw, roll) = if xy > 0.001 {
        ((-f.z).atan2(xy), f.y.atan2(f.x), l.z.atan2(u.z))
    } else {
        ((-f.z).atan2(xy), (-l.x).atan2(l.y), 0.0)
    };
    DVec3::new(pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees())
}

/// Plane type: 0-2 axial X/Y/Z, 3-5 "mostly" X/Y/Z.
pub fn plane_type(n: DVec3) -> i32 {
    if n.x == 1.0 || n.x == -1.0 {
        return 0;
    }
    if n.y == 1.0 || n.y == -1.0 {
        return 1;
    }
    if n.z == 1.0 || n.z == -1.0 {
        return 2;
    }
    let a = n.abs();
    if a.x >= a.y && a.x >= a.z {
        3
    } else if a.y >= a.x && a.y >= a.z {
        4
    } else {
        5
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: DVec3,
    pub dist: f64,
}

impl Plane {
    pub fn new(normal: DVec3, dist: f64) -> Plane {
        Plane { normal, dist }
    }

    /// Plane through three points, normal facing out of a Hammer brush side
    /// (points are clockwise seen from the front). `None` if degenerate.
    pub fn from_points(p: &[DVec3; 3]) -> Option<Plane> {
        let n = (p[0] - p[1]).cross(p[2] - p[1]);
        let len = n.length();
        if len < 1e-9 || !len.is_finite() {
            return None;
        }
        let n = n / len;
        Some(Plane { normal: n, dist: n.dot(p[1]) })
    }

    pub fn distance(&self, p: DVec3) -> f64 {
        self.normal.dot(p) - self.dist
    }

    pub fn flipped(&self) -> Plane {
        Plane { normal: -self.normal, dist: -self.dist }
    }

    pub fn kind(&self) -> i32 {
        plane_type(self.normal)
    }

    /// Plane through three points with near-axial normals snapped about the points' centroid,
    /// so the snapped plane still passes through them (a slightly skewed brush side otherwise
    /// moves by `normal error * distance from origin`, opening gaps).
    pub fn from_points_snapped(p: &[DVec3; 3]) -> Option<Plane> {
        let raw = Plane::from_points(p)?;
        let snapped = raw.snapped();
        if snapped.normal == raw.normal {
            return Some(snapped);
        }
        let center = (p[0] + p[1] + p[2]) / 3.0;
        Some(Plane { normal: snapped.normal, dist: snapped.normal.dot(center) }.snapped())
    }

    /// Snaps nearly-axial normals and nearly-integer distances (vbsp `SnapPlane`).
    pub fn snapped(mut self) -> Plane {
        for i in 0..3 {
            if (self.normal[i] - 1.0).abs() < NORMAL_EPSILON {
                self.normal = DVec3::ZERO;
                self.normal[i] = 1.0;
                break;
            }
            if (self.normal[i] + 1.0).abs() < NORMAL_EPSILON {
                self.normal = DVec3::ZERO;
                self.normal[i] = -1.0;
                break;
            }
        }
        let r = self.dist.round();
        if (self.dist - r).abs() < DIST_EPSILON {
            self.dist = r;
        }
        self
    }
}

/// Deduplicated planes. Planes are always stored in pairs: `n` and `n ^ 1` are opposites, and
/// the even one of each pair faces a positive axis (vbsp's convention, required by the engine).
#[derive(Default, Clone)]
pub struct PlaneSet {
    pub planes: Vec<Plane>,
    hash: std::collections::HashMap<i64, Vec<usize>>,
}

const HASH_SCALE: f64 = 1.0 / 8.0;

impl PlaneSet {
    fn bucket(dist: f64) -> i64 {
        (dist.abs() * HASH_SCALE).floor() as i64
    }

    fn matches(p: &Plane, n: DVec3, d: f64) -> bool {
        (p.normal - n).abs().max_element() < NORMAL_EPSILON && (p.dist - d).abs() < DIST_EPSILON
    }

    /// Index of an existing plane.
    pub fn lookup(&self, p: Plane) -> Option<usize> {
        let p = p.snapped();
        let b = Self::bucket(p.dist);
        for bb in [b - 1, b, b + 1] {
            if let Some(list) = self.hash.get(&bb) {
                for &i in list {
                    if Self::matches(&self.planes[i], p.normal, p.dist) {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    /// Index of the plane, adding it (and its opposite) if new.
    pub fn find(&mut self, p: Plane) -> usize {
        if let Some(i) = self.lookup(p) {
            return i;
        }
        let p = p.snapped();
        // New pair: the even plane faces a positive axis.
        let flipped = p.flipped();
        let positive = {
            let k = plane_type(p.normal);
            let axis = (k % 3) as usize;
            p.normal[axis] > 0.0
        };
        let (a, c) = if positive { (p, flipped) } else { (flipped, p) };
        let i = self.planes.len();
        self.planes.push(a);
        self.planes.push(c);
        self.hash.entry(Self::bucket(a.dist)).or_default().push(i);
        self.hash.entry(Self::bucket(c.dist)).or_default().push(i + 1);
        if positive { i } else { i + 1 }
    }

    pub fn len(&self) -> usize {
        self.planes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }
}

impl std::ops::Index<usize> for PlaneSet {
    type Output = Plane;
    fn index(&self, i: usize) -> &Plane {
        &self.planes[i]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Default for Aabb {
    fn default() -> Self {
        Aabb::EMPTY
    }
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: DVec3::splat(f64::MAX), max: DVec3::splat(f64::MIN) };

    pub fn add(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    pub fn intersects(&self, o: &Aabb) -> bool {
        self.min.cmple(o.max).all() && self.max.cmpge(o.min).all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_roundtrip() {
        for a in [DVec3::new(0.0, 90.0, 0.0), DVec3::new(-30.0, 45.0, 10.0), DVec3::new(20.0, -170.0, -60.0)] {
            let b = matrix_angles(angles_matrix(a));
            assert!((a - b).abs().max_element() < 1e-9, "{a} -> {b}");
        }
        let m = angles_matrix(DVec3::new(0.0, 90.0, 0.0));
        assert!((m * DVec3::X - DVec3::Y).length() < 1e-12);
    }

    #[test]
    fn hammer_points_face_outward() {
        // Top face of a box from Hammer: normal +Z.
        let p = Plane::from_points(&[DVec3::new(0., 0., 64.), DVec3::new(64., 0., 64.), DVec3::new(64., -64., 64.)]).unwrap();
        assert_eq!(p.normal, DVec3::Z);
        assert_eq!(p.dist, 64.0);
    }

    #[test]
    fn skewed_side_snaps_through_its_points() {
        let pts = [DVec3::new(-256.01, -576.0, 0.0), DVec3::new(-256.0, -592.0, 0.0), DVec3::new(-255.99, -592.0, 128.0)];
        let p = Plane::from_points_snapped(&pts).unwrap();
        assert_eq!(p.normal, DVec3::X);
        assert_eq!(p.dist, -256.0);
    }

    #[test]
    fn plane_pairs() {
        let mut s = PlaneSet::default();
        let a = s.find(Plane::new(-DVec3::Z, -32.0));
        let b = s.find(Plane::new(DVec3::Z, 32.0));
        assert_eq!(a ^ 1, b);
        assert_eq!(b & 1, 0, "positive-facing plane is even");
        assert_eq!(s.find(Plane::new(DVec3::Z, 32.000001)), b);
        assert_eq!(s.len(), 2);
    }
}
