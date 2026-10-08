//! Direct lighting of luxel samples: point, spot, surface (texture) lights, the sun and sky
//! ambient, with shadow rays through the BVH and Source's bump basis.

use super::lights::{Kind, Light, LightSet};
use super::scene::{Face, Scene};
use glam::{DVec3, Vec3};

/// Source's tangent-space bump basis.
pub const BUMP_BASIS: [DVec3; 3] = [
    DVec3::new(0.816_496_580_927_726, 0.0, 0.577_350_269_189_626),
    DVec3::new(-0.408_248_290_463_863, 0.707_106_781_186_548, 0.577_350_269_189_626),
    DVec3::new(-0.408_248_290_463_863, -0.707_106_781_186_548, 0.577_350_269_189_626),
];

/// World-space normals of a face: flat normal plus the three bump directions.
pub fn face_normals(f: &Face) -> Vec<DVec3> {
    let n = f.normal();
    if !f.bump {
        return vec![n];
    }
    let mut s = f.tex_s - n * f.tex_s.dot(n);
    if s.length_squared() < 1e-12 {
        s = n.any_orthonormal_vector();
    }
    let s = s.normalize();
    let mut t = n.cross(s);
    if t.dot(f.tex_t) < 0.0 {
        // Follow the texture's t direction (Source's left-handed case).
        t = -t;
    }
    let mut v = vec![n];
    for b in BUMP_BASIS {
        v.push((s * b.x + t * b.y + n * b.z).normalize());
    }
    v
}

/// Unit sphere directions (Fibonacci), for sky ambient and ambient cubes.
pub fn sphere_dirs(n: usize) -> Vec<DVec3> {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - (i as f64 + 0.5) / n as f64 * 2.0;
            let r = (1.0 - y * y).sqrt();
            let th = golden * i as f64;
            DVec3::new(th.cos() * r, th.sin() * r, y)
        })
        .collect()
}

fn f32v(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

pub struct Gather<'a> {
    pub scene: &'a Scene,
    pub lights: &'a LightSet,
    pub sky_dirs: &'a [DVec3],
}

pub const RAY_EPS: f64 = 0.5;
const SKY_DIST: f64 = 65536.0;

impl Gather<'_> {
    /// Whether the segment from `p` towards `dir` reaches `dist` unblocked.
    pub fn visible(&self, p: DVec3, dir: DVec3, dist: f64) -> bool {
        !self.scene.bvh.occluded(f32v(p), f32v(dir), (dist - RAY_EPS).max(0.0) as f32)
    }

    /// Whether a ray from `p` reaches the sky.
    pub fn sees_sky(&self, p: DVec3, dir: DVec3) -> bool {
        match self.scene.bvh.intersect(f32v(p), f32v(dir), SKY_DIST as f32) {
            Some(h) => self.scene.is_sky(self.scene.bvh.tris[h.tri as usize].id),
            None => false,
        }
    }

    /// Adds the direct light of `light` at `p` for each normal into `out` (same order).
    pub fn light_at(&self, light: &Light, p: DVec3, normals: &[DVec3], out: &mut [DVec3]) {
        let n0 = normals[0];
        let origin = p + n0 * RAY_EPS;
        match light.kind {
            Kind::Point | Kind::Spot | Kind::Surface => {
                let delta = light.origin - origin;
                let d = delta.length();
                if d < 1e-3 {
                    return;
                }
                let dir = delta / d;
                if n0.dot(dir) <= 1e-4 {
                    return;
                }
                if light.radius > 0.0 && d > light.radius {
                    return;
                }
                let mut scale = match light.kind {
                    Kind::Surface => {
                        let de = -dir.dot(light.normal);
                        if de <= 0.0 {
                            return;
                        }
                        de / (d * d).max(1.0)
                    }
                    _ => light.falloff(d),
                };
                if light.kind == Kind::Spot {
                    let dot2 = -dir.dot(light.normal);
                    if dot2 <= light.stopdot2 {
                        return;
                    }
                    if light.exponent != 0.0 && light.exponent != 1.0 {
                        scale *= dot2.powf(light.exponent);
                    } else if light.exponent == 1.0 {
                        scale *= dot2;
                    }
                    if dot2 <= light.stopdot {
                        scale *= (dot2 - light.stopdot2) / (light.stopdot - light.stopdot2);
                    }
                }
                if scale <= 0.0 || !self.visible(origin, dir, d) {
                    return;
                }
                for (o, n) in out.iter_mut().zip(normals) {
                    *o += light.intensity * (scale * n.dot(dir).max(0.0));
                }
            }
            Kind::Sky => {
                let dir = -light.normal;
                if n0.dot(dir) <= 0.0 {
                    return;
                }
                if !self.sees_sky(origin, dir) {
                    return;
                }
                for (o, n) in out.iter_mut().zip(normals) {
                    *o += light.intensity * n.dot(dir).max(0.0);
                }
            }
            Kind::SkyAmbient => {
                let mut sums = [0.0f64; 4];
                let mut totals = [0.0f64; 4];
                for &d in self.sky_dirs {
                    let dots: Vec<f64> = normals.iter().map(|n| n.dot(d)).collect();
                    if dots.iter().all(|&x| x <= 0.0) {
                        continue;
                    }
                    for (i, &x) in dots.iter().enumerate() {
                        if x > 0.0 {
                            totals[i] += x;
                        }
                    }
                    if dots[0] <= 0.0 && normals.len() == 1 {
                        continue;
                    }
                    if self.sees_sky(origin, d) {
                        for (i, &x) in dots.iter().enumerate() {
                            if x > 0.0 {
                                sums[i] += x;
                            }
                        }
                    }
                }
                for (i, o) in out.iter_mut().enumerate() {
                    if totals[i] > 0.0 {
                        *o += light.intensity * (sums[i] / totals[i]);
                    }
                }
            }
        }
    }
}

/// A luxel sample position: on the face, pushed out of solid space.
pub fn sample_position(scene: &Scene, f: &Face, s: f64, t: f64, center: DVec3) -> DVec3 {
    let mut p = f.clamp_to_face(f.luxel_pos(s, t));
    let n = f.normal();
    for _ in 0..8 {
        if !scene.point_solid(p + n * RAY_EPS) {
            break;
        }
        p = p + (center - p) * 0.35;
    }
    p
}
