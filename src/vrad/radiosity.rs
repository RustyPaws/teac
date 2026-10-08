//! Bounced light. Every lit (or emissive) face is cut into a grid of patches; each patch
//! shoots cosine-weighted rays once to find which patches it sees (its transfer list), then
//! bounces iterate `E_ind = mean(reflectivity * (E_direct + E_ind))` over those transfers.
//! Luxels read the indirect light bilinearly from their face's patch grid.

use super::scene::{Face, Scene};
use crate::math::{Plane, Winding};
use glam::{DVec3, Vec3};
use rayon::prelude::*;
use std::collections::HashMap;

pub struct Patch {
    pub face: usize,
    pub center: DVec3,
    pub normal: DVec3,
    pub area: f64,
    pub reflectivity: DVec3,
    pub cell: (i32, i32),
}

/// Patch grid of one face.
pub struct FaceGrid {
    origin: DVec3,
    u: DVec3,
    v: DVec3,
    size: f64,
    pub cells: HashMap<(i32, i32), usize>,
}

impl FaceGrid {
    fn coords(&self, p: DVec3) -> (f64, f64) {
        let d = p - self.origin;
        (d.dot(self.u) / self.size, d.dot(self.v) / self.size)
    }

    /// Patch containing (or nearest to) a point of the face.
    pub fn patch_at(&self, p: DVec3) -> Option<usize> {
        let (cu, cv) = self.coords(p);
        let key = (cu.floor() as i32, cv.floor() as i32);
        if let Some(&i) = self.cells.get(&key) {
            return Some(i);
        }
        self.cells
            .iter()
            .min_by(|a, b| {
                let da = ((a.0 .0 as f64 + 0.5 - cu).powi(2) + (a.0 .1 as f64 + 0.5 - cv).powi(2)) as f64;
                let db = ((b.0 .0 as f64 + 0.5 - cu).powi(2) + (b.0 .1 as f64 + 0.5 - cv).powi(2)) as f64;
                da.total_cmp(&db)
            })
            .map(|(_, &i)| i)
    }

    /// Bilinear interpolation of per-patch values at a point.
    pub fn interpolate(&self, p: DVec3, values: &[DVec3]) -> DVec3 {
        let (cu, cv) = self.coords(p);
        let (fu, fv) = (cu - 0.5, cv - 0.5);
        let (iu, iv) = (fu.floor(), fv.floor());
        let (tu, tv) = (fu - iu, fv - iv);
        let mut sum = DVec3::ZERO;
        let mut wsum = 0.0;
        for (du, dv, w) in [(0, 0, (1.0 - tu) * (1.0 - tv)), (1, 0, tu * (1.0 - tv)), (0, 1, (1.0 - tu) * tv), (1, 1, tu * tv)] {
            if let Some(&i) = self.cells.get(&(iu as i32 + du, iv as i32 + dv)) {
                sum += values[i] * w;
                wsum += w;
            }
        }
        if wsum > 1e-6 {
            sum / wsum
        } else {
            self.patch_at(p).map_or(DVec3::ZERO, |i| values[i])
        }
    }
}

pub struct Radiosity {
    pub patches: Vec<Patch>,
    /// Per face (indexed like `scene.faces`), when it has patches.
    pub grids: Vec<Option<FaceGrid>>,
}

pub fn make_patches(scene: &Scene, chop: f64, wanted: &[bool]) -> Radiosity {
    let mut patches = Vec::new();
    let mut grids = Vec::with_capacity(scene.faces.len());
    for (fi, f) in scene.faces.iter().enumerate() {
        if !wanted[fi] || f.winding.points.len() < 3 {
            grids.push(None);
            continue;
        }
        let n = f.normal();
        let mut u = f.luxel_s - n * f.luxel_s.dot(n);
        if u.length_squared() < 1e-12 {
            u = n.any_orthonormal_vector();
        }
        let u = u.normalize();
        let v = n.cross(u);
        let origin = f.winding.points[0];
        let mut grid = FaceGrid { origin, u, v, size: chop, cells: HashMap::new() };
        let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for &p in &f.winding.points {
            let (a, b) = grid.coords(p);
            lo = (lo.0.min(a), lo.1.min(b));
            hi = (hi.0.max(a), hi.1.max(b));
        }
        for cu in lo.0.floor() as i32..hi.0.ceil() as i32 {
            for cv in lo.1.floor() as i32..hi.1.ceil() as i32 {
                // Clip the face to the cell.
                let a0 = origin + u * (cu as f64 * chop) + v * (cv as f64 * chop);
                let cuts = [
                    Plane::new(u, u.dot(a0)),
                    Plane::new(-u, -u.dot(a0 + u * chop)),
                    Plane::new(v, v.dot(a0)),
                    Plane::new(-v, -v.dot(a0 + v * chop)),
                ];
                let mut w: Option<Winding> = Some(f.winding.clone());
                for c in &cuts {
                    w = w.and_then(|w| w.clip(c, 0.01, false));
                }
                let Some(w) = w else { continue };
                let area = w.area();
                if area < 1.0 {
                    continue;
                }
                grid.cells.insert((cu, cv), patches.len());
                // Texdata reflectivity is gamma-encoded; bounce in linear space.
                let reflectivity = f.reflectivity.powf(2.2);
                patches.push(Patch { face: fi, center: w.center(), normal: n, area, reflectivity, cell: (cu, cv) });
            }
        }
        grids.push(Some(grid));
    }
    Radiosity { patches, grids }
}

fn f32v(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Cosine-weighted, stratified hemisphere directions around `n` (`k` x `k` strata).
fn hemisphere(n: DVec3, k: usize, seed: u64) -> Vec<DVec3> {
    let t = n.any_orthonormal_vector();
    let b = n.cross(t);
    let mut rng = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut rnd = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = Vec::with_capacity(k * k);
    for i in 0..k {
        for j in 0..k {
            let u1 = (i as f64 + rnd()) / k as f64;
            let u2 = (j as f64 + rnd()) / k as f64;
            let r = u1.sqrt();
            let phi = 2.0 * std::f64::consts::PI * u2;
            let x = r * phi.cos();
            let y = r * phi.sin();
            let z = (1.0 - u1).max(0.0).sqrt();
            out.push(t * x + b * y + n * z);
        }
    }
    out
}

/// One gather ray of a patch that hit another patch.
#[derive(Clone, Copy)]
pub struct Transfer {
    pub patch: u32,
    pub dir: Vec3,
}

/// Transfer lists: for each patch, the patches its gather rays hit (front sides only).
pub fn transfers(scene: &Scene, rad: &Radiosity, strata: usize) -> Vec<Vec<Transfer>> {
    rad.patches
        .par_iter()
        .enumerate()
        .map(|(pi, p)| {
            let o = p.center + p.normal * 0.5;
            let mut list = Vec::new();
            for d in hemisphere(p.normal, strata, pi as u64 + 1) {
                let Some(h) = scene.bvh.intersect(f32v(o), f32v(d), 65536.0) else { continue };
                let id = scene.bvh.tris[h.tri as usize].id;
                let Some(fi) = scene.tri_face(id) else { continue };
                let f: &Face = &scene.faces[fi];
                if d.dot(f.normal()) >= 0.0 {
                    continue; // back side
                }
                let Some(g) = &rad.grids[fi] else { continue };
                let hit = o + d * h.t as f64;
                if let Some(q) = g.patch_at(hit) {
                    list.push(Transfer { patch: q as u32, dir: f32v(d) });
                }
            }
            list
        })
        .collect()
}

/// Iterates bounces; returns the indirect irradiance per patch.
pub fn bounce(rad: &Radiosity, transfers: &[Vec<Transfer>], direct: &[DVec3], rays: usize, max_bounces: usize, ctx: &crate::ctx::Ctx) -> Vec<DVec3> {
    let n = rad.patches.len();
    let mut ind = vec![DVec3::ZERO; n];
    let mut ph = ctx.progress("Bounces", max_bounces as u64);
    let mut done = 0;
    for b in 0..max_bounces {
        let exitance: Vec<DVec3> = (0..n).map(|i| rad.patches[i].reflectivity * (direct[i] + ind[i])).collect();
        let next: Vec<DVec3> = transfers
            .par_iter()
            .map(|t| t.iter().fold(DVec3::ZERO, |a, q| a + exitance[q.patch as usize]) / rays as f64)
            .collect();
        let change: f64 = next.iter().zip(&ind).map(|(a, b)| (*a - *b).max_element().abs()).fold(0.0, f64::max);
        ind = next;
        done = b + 1;
        ph.inc(1);
        if change < 1e-4 {
            break;
        }
    }
    ph.note(format!("{done} bounces, {n} patches"));
    ind
}

/// Directional final gather for bumped faces: indirect irradiance on each of `normals`
/// (flat normal first), from the converged exitance.
pub fn gather_bumped(p: &Patch, transfers: &[Transfer], exitance: &[DVec3], normals: &[DVec3], rays: usize) -> Vec<DVec3> {
    let mut out = vec![DVec3::ZERO; normals.len()];
    for t in transfers {
        let d = DVec3::new(t.dir.x as f64, t.dir.y as f64, t.dir.z as f64);
        let cn = p.normal.dot(d).max(0.05);
        let b = exitance[t.patch as usize];
        out[0] += b;
        for (i, n) in normals.iter().enumerate().skip(1) {
            out[i] += b * (n.dot(d).max(0.0) / cn);
        }
    }
    for o in &mut out {
        *o /= rays as f64;
    }
    out
}
