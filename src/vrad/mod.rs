//! STAGE 3 — vrad: lightmaps, ambient cubes and world lights.

pub mod bvh;
pub mod direct;
pub mod lights;
pub mod props;
pub mod radiosity;
pub mod scene;

use crate::bspfile::*;
use crate::ctx::Ctx;
use crate::flags::{contents, surf};
use crate::fs::GameFs;
use anyhow::Result;
use direct::{face_normals, sample_position, sphere_dirs, Gather};
use glam::DVec3;
use lights::{Kind, Light};
use rayon::prelude::*;
use scene::Scene;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RadOptions {
    /// One sample per luxel, few gather rays.
    pub fast: bool,
    /// High quality: 4x4 supersampling, more gather and sky rays.
    #[serde(rename = "final")]
    pub final_: bool,
    /// Maximum radiosity bounces (stops earlier once converged).
    pub bounce: usize,
    /// Multiplier for sky ambient rays.
    pub extrasky: usize,
    /// Patch size in world units.
    pub chop: f64,
    /// Extra `.rad` file with texture lights.
    pub lights: Option<std::path::PathBuf>,
    /// Write HDR lighting too.
    pub hdr: bool,
}

impl Default for RadOptions {
    fn default() -> Self {
        RadOptions { fast: false, final_: false, bounce: 100, extrasky: 1, chop: 64.0, lights: None, hdr: false }
    }
}

/// Lighting of one face: styles and per-style luxel data (`luxels * normals` values).
struct FaceLight {
    styles: Vec<u8>,
    data: Vec<Vec<DVec3>>,
    nvec: usize,
}

/// Lightmaps and ambient cubes are stored in 0..255 units (worldlights in 0..1).
const LIGHTMAP_SCALE: f64 = 255.0;

/// Source `VectorToColorRGBExp32` of a linear (0..1) value scaled to lightmap units: the largest
/// channel's mantissa lands in [128, 255].
pub fn encode(v: DVec3) -> ColorRgbExp32 {
    encode_raw(v * LIGHTMAP_SCALE)
}

pub fn encode_raw(v: DVec3) -> ColorRgbExp32 {
    let v = v.max(DVec3::ZERO);
    let m = v.max_element();
    if m < 1e-12 {
        return ColorRgbExp32::default();
    }
    let e = m.log2().floor() as i32 - 7;
    let e = e.clamp(-128, 127);
    let s = 2f64.powi(-e);
    let c = |x: f64| (x * s + 0.5).floor().clamp(0.0, 255.0) as u8;
    ColorRgbExp32 { r: c(v.x), g: c(v.y), b: c(v.z), exponent: e as i8 }
}

pub fn decode(c: ColorRgbExp32) -> DVec3 {
    DVec3::new(c.r as f64, c.g as f64, c.b as f64) * 2f64.powi(c.exponent as i32)
}

fn luxel_offsets(n: usize) -> Vec<(f64, f64)> {
    if n <= 1 {
        return vec![(0.0, 0.0)];
    }
    let mut v = Vec::new();
    for i in 0..n {
        for j in 0..n {
            v.push(((i as f64 + 0.5) / n as f64 - 0.5, (j as f64 + 0.5) / n as f64 - 0.5));
        }
    }
    v
}

pub fn run(opts: &RadOptions, ctx: &Ctx, bsp: &mut BspFile, fs: Option<&GameFs>, map_rad: Option<&str>) -> Result<()> {
    ctx.stage("vrad", if opts.final_ { "lighting (final)" } else if opts.fast { "lighting (fast)" } else { "lighting" });
    let mut ph = ctx.phase("Loading scene");
    let (prop_tris, nprops) = fs.map(|fs| props::prop_occluders(bsp, fs)).unwrap_or_default();
    let scene = Scene::load(bsp, prop_tris)?;
    ph.note(format!("{} faces, {} occluder triangles ({nprops} props)", scene.faces.len(), scene.bvh.tris.len()));
    drop(ph);
    if scene.pvs.is_none() {
        ctx.warn("no visibility data: every light is tested against every face (run vis first)");
    }

    // Texture lights.
    let mut texlights = HashMap::new();
    if let Some(t) = fs.and_then(|fs| std::fs::read_to_string(fs.game_dir.join("lights.rad")).ok()) {
        lights::parse_rad(&t, &mut texlights);
    }
    if let Some(t) = map_rad {
        lights::parse_rad(t, &mut texlights);
    }
    if let Some(p) = &opts.lights {
        lights::parse_rad(&std::fs::read_to_string(p)?, &mut texlights);
    }
    let mut set = lights::collect(&scene, texlights);

    // Patches for lit and emissive faces.
    let emissive: Vec<Option<DVec3>> = scene
        .faces
        .iter()
        .map(|f| set.texlights.get(&crate::material::normalize_name(&f.material)).copied().map(|c| c / 255.0))
        .collect();
    let wanted: Vec<bool> = scene.faces.iter().zip(&emissive).map(|(f, e)| (f.lit && f.w * f.h > 0) || e.is_some()).collect();
    let mut ph = ctx.phase("Patches");
    let rad = radiosity::make_patches(&scene, opts.chop, &wanted);
    // Emissive patches become surface lights.
    let mut n_surface = 0;
    for p in &rad.patches {
        if let Some(m) = emissive[p.face] {
            set.lights.push(Light {
                kind: Kind::Surface,
                origin: p.center + p.normal,
                intensity: m * (p.area / std::f64::consts::PI),
                normal: p.normal,
                cluster: scene.cluster_at(p.center + p.normal),
                style: 0,
                stopdot: 0.0,
                stopdot2: 0.0,
                exponent: 0.0,
                radius: 0.0,
                constant: 0.0,
                linear: 0.0,
                quadratic: 1.0,
                sun_spread: 1.0,
            });
            n_surface += 1;
        }
    }
    ph.note(format!("{} patches, {} lights ({n_surface} surface)", rad.patches.len(), set.lights.len()));
    drop(ph);

    let sky_rays = if opts.fast { 32 } else if opts.final_ { 162 } else { 64 } * opts.extrasky.max(1);
    let sky_dirs = sphere_dirs(sky_rays * 2);
    let gather = Gather { scene: &scene, lights: &set, sky_dirs: &sky_dirs };
    let subsamples = luxel_offsets(if opts.fast { 1 } else if opts.final_ { 4 } else { 2 });

    // ---- Direct lighting ----
    let lit: Vec<usize> = (0..scene.faces.len()).filter(|&i| scene.faces[i].lit && scene.faces[i].w * scene.faces[i].h > 0).collect();
    let ph = ctx.progress("Direct lighting", lit.len() as u64);
    let mut face_light: Vec<Option<FaceLight>> = (0..scene.faces.len()).map(|_| None).collect();
    let results: Vec<(usize, FaceLight)> = lit
        .par_iter()
        .map(|&fi| {
            let r = light_face(&gather, fi, &subsamples);
            ph.inc(1);
            (fi, r)
        })
        .collect();
    drop(ph);
    for (fi, r) in results {
        face_light[fi] = Some(r);
    }

    // ---- Radiosity ----
    if opts.bounce > 0 && !rad.patches.is_empty() {
        let strata = if opts.fast { 6 } else if opts.final_ { 16 } else { 10 };
        let mut ph = ctx.phase("Patch direct light");
        let mut direct = vec![DVec3::ZERO; rad.patches.len()];
        let mut counts = vec![0usize; rad.patches.len()];
        for (fi, fl) in face_light.iter().enumerate() {
            let (Some(fl), Some(grid)) = (fl, &rad.grids[fi]) else { continue };
            let f = &scene.faces[fi];
            let Some(si) = fl.styles.iter().position(|&s| s == 0) else { continue };
            for t in 0..f.h {
                for s in 0..f.w {
                    let p = f.clamp_to_face(f.luxel_pos(s as f64, t as f64));
                    if let Some(pi) = grid.patch_at(p) {
                        direct[pi] += fl.data[si][(t * f.w + s) * fl.nvec];
                        counts[pi] += 1;
                    }
                }
            }
        }
        for (d, &c) in direct.iter_mut().zip(&counts) {
            if c > 0 {
                *d /= c as f64;
            }
        }
        ph.note(format!("{} transfer rays per patch", strata * strata));
        drop(ph);
        let ph = ctx.phase("Transfers");
        let tr = radiosity::transfers(&scene, &rad, strata);
        drop(ph);
        let ind = radiosity::bounce(&rad, &tr, &direct, strata * strata, opts.bounce, ctx);
        let exitance: Vec<DVec3> = (0..rad.patches.len()).map(|i| rad.patches[i].reflectivity * (direct[i] + ind[i])).collect();
        // Per-patch indirect for every normal of its face (bump maps get directional light).
        let rays = strata * strata;
        let per_patch: Vec<Vec<DVec3>> = rad
            .patches
            .par_iter()
            .enumerate()
            .map(|(pi, p)| {
                let f = &scene.faces[p.face];
                if f.bump {
                    radiosity::gather_bumped(p, &tr[pi], &exitance, &face_normals(f), rays)
                } else {
                    vec![ind[pi]]
                }
            })
            .collect();
        // Indirect light into luxels: one value list per normal slot.
        let comp: Vec<Vec<DVec3>> = (0..4).map(|k| per_patch.iter().map(|v| v.get(k).copied().unwrap_or(v[0])).collect()).collect();
        for (fi, fl) in face_light.iter_mut().enumerate() {
            let (Some(fl), Some(grid)) = (fl.as_mut(), &rad.grids[fi]) else { continue };
            let f = &scene.faces[fi];
            let si = match fl.styles.iter().position(|&s| s == 0) {
                Some(i) => i,
                None => {
                    fl.styles.insert(0, 0);
                    fl.data.insert(0, vec![DVec3::ZERO; f.w * f.h * fl.nvec]);
                    0
                }
            };
            for t in 0..f.h {
                for s in 0..f.w {
                    let p = f.clamp_to_face(f.luxel_pos(s as f64, t as f64));
                    for k in 0..fl.nvec {
                        fl.data[si][(t * f.w + s) * fl.nvec + k] += grid.interpolate(p, &comp[k]);
                    }
                }
            }
        }
        write_ambient(ctx, bsp, &scene, &gather, &rad, Some(&exitance))?;
    } else {
        write_ambient(ctx, bsp, &scene, &gather, &rad, None)?;
    }

    // ---- Output ----
    let ph = ctx.phase("Writing lightmaps");
    let mut dfaces: Vec<DFace> = bsp.get(lump::FACES)?;
    let mut data: Vec<u8> = Vec::new();
    for (fi, df) in dfaces.iter_mut().enumerate() {
        match &face_light[fi] {
            Some(fl) if !fl.styles.is_empty() => {
                let n = fl.styles.len().min(4);
                // Average colour per style precedes the samples (last style first).
                for k in (0..n).rev() {
                    let flat: Vec<DVec3> = fl.data[k].iter().step_by(fl.nvec).copied().collect();
                    let avg = flat.iter().copied().sum::<DVec3>() / flat.len().max(1) as f64;
                    data.extend_from_slice(bytemuck::bytes_of(&encode(avg)));
                }
                df.lightofs = data.len() as i32;
                df.styles = [255; 4];
                for k in 0..n {
                    df.styles[k] = fl.styles[k];
                    let f = &scene.faces[fi];
                    for b in 0..fl.nvec {
                        for l in 0..f.w * f.h {
                            data.extend_from_slice(bytemuck::bytes_of(&encode(fl.data[k][l * fl.nvec + b])));
                        }
                    }
                }
            }
            _ => {
                df.lightofs = -1;
                df.styles = [255; 4];
            }
        }
    }
    let size = data.len();
    bsp.set(lump::FACES, &dfaces);
    if opts.hdr {
        bsp.set_raw(lump::LIGHTING_HDR, data.clone());
        bsp.set(lump::FACES_HDR, &dfaces);
    }
    bsp.set_raw(lump::LIGHTING, data);
    bsp.set(lump::WORLDLIGHTS, &lights::world_lights(&set));
    write_vertex_normals(bsp, &scene)?;
    set_sky_flags(bsp, &scene)?;
    drop(ph);
    ctx.info(format!("{:.1} MiB of lightmaps for {} lit faces", size as f64 / 1048576.0, lit.len()));
    Ok(())
}

/// Lights every luxel of a face with all candidate lights.
fn light_face(g: &Gather, fi: usize, subsamples: &[(f64, f64)]) -> FaceLight {
    let scene = g.scene;
    let f = &scene.faces[fi];
    let normals = face_normals(f);
    let nvec = normals.len();
    let clusters = &scene.face_clusters[fi];
    let cands: Vec<&Light> = g
        .lights
        .lights
        .iter()
        .filter(|l| match l.kind {
            Kind::Sky | Kind::SkyAmbient => true,
            _ => l.cluster < 0 || clusters.is_empty() || clusters.iter().any(|&c| scene.cluster_sees(c, l.cluster)),
        })
        .collect();
    let mut styles: Vec<u8> = Vec::new();
    for l in &cands {
        if !styles.contains(&l.style) {
            styles.push(l.style);
        }
    }
    styles.sort_unstable();
    styles.truncate(4);
    let mut data: Vec<Vec<DVec3>> = styles.iter().map(|_| vec![DVec3::ZERO; f.w * f.h * nvec]).collect();
    let center = f.winding.center();
    let mut acc = vec![DVec3::ZERO; nvec];
    for t in 0..f.h {
        for s in 0..f.w {
            for (si, &style) in styles.iter().enumerate() {
                for a in acc.iter_mut() {
                    *a = DVec3::ZERO;
                }
                for &(ds, dt) in subsamples {
                    let p = sample_position(scene, f, s as f64 + ds, t as f64 + dt, center);
                    for l in cands.iter().filter(|l| l.style == style) {
                        g.light_at(l, p, &normals, &mut acc);
                    }
                }
                let base = (t * f.w + s) * nvec;
                for k in 0..nvec {
                    data[si][base + k] = acc[k] / subsamples.len() as f64;
                }
            }
        }
    }
    FaceLight { styles, data, nvec }
}

const AXES: [DVec3; 6] = [DVec3::X, DVec3::NEG_X, DVec3::Y, DVec3::NEG_Y, DVec3::Z, DVec3::NEG_Z];

/// Leaf ambient cubes: one sample per non-solid leaf (direct lights per axis plus surfaces seen
/// by gather rays).
fn write_ambient(ctx: &Ctx, bsp: &mut BspFile, scene: &Scene, g: &Gather, rad: &radiosity::Radiosity, exitance: Option<&[DVec3]>) -> Result<()> {
    let ph = ctx.progress("Ambient cubes", scene.leafs.len() as u64);
    let dirs = sphere_dirs(96);
    let cubes: Vec<Option<DLeafAmbientLighting>> = (0..scene.leafs.len())
        .into_par_iter()
        .map(|li| {
            ph.inc(1);
            let l = &scene.leafs[li];
            if l.contents & contents::SOLID != 0 || li == 0 {
                return None;
            }
            let min = DVec3::new(l.mins[0] as f64, l.mins[1] as f64, l.mins[2] as f64);
            let max = DVec3::new(l.maxs[0] as f64, l.maxs[1] as f64, l.maxs[2] as f64);
            let mut p = (min + max) * 0.5;
            if scene.leaf_at(p) != li {
                // Try a few other points of the box.
                for k in 1u32..16 {
                    let f = |i: u32| ((k.wrapping_mul(2654435761u32)).rotate_left(i) % 1000) as f64 / 1000.0;
                    let q = min + (max - min) * DVec3::new(f(3), f(11), f(19));
                    if scene.leaf_at(q) == li {
                        p = q;
                        break;
                    }
                }
            }
            let mut cube = [DVec3::ZERO; 6];
            for (ai, axis) in AXES.iter().enumerate() {
                let mut v = [DVec3::ZERO];
                for light in g.lights.lights.iter().filter(|l| l.style == 0) {
                    let ok = match light.kind {
                        Kind::Sky | Kind::SkyAmbient => true,
                        _ => scene.cluster_sees(l.cluster as i32, light.cluster),
                    };
                    if ok {
                        g.light_at(light, p - *axis * direct::RAY_EPS, &[*axis], &mut v);
                    }
                }
                cube[ai] += v[0];
            }
            if let Some(ex) = exitance {
                let mut sums = [DVec3::ZERO; 6];
                let mut wts = [0.0; 6];
                for &d in &dirs {
                    let w: Vec<f64> = AXES.iter().map(|a| a.dot(d).max(0.0)).collect();
                    for (i, x) in w.iter().enumerate() {
                        wts[i] += x;
                    }
                    let Some(h) = scene.bvh.intersect(p.as_vec3(), d.as_vec3(), 65536.0) else { continue };
                    let Some(fi) = scene.tri_face(scene.bvh.tris[h.tri as usize].id) else { continue };
                    if d.dot(scene.faces[fi].normal()) >= 0.0 {
                        continue;
                    }
                    let Some(grid) = &rad.grids[fi] else { continue };
                    let hit = p + d * h.t as f64;
                    let b = grid.interpolate(hit, ex);
                    for (i, x) in w.iter().enumerate() {
                        sums[i] += b * *x;
                    }
                }
                for i in 0..6 {
                    if wts[i] > 0.0 {
                        cube[i] += sums[i] / wts[i];
                    }
                }
            }
            let rel = |v: f64, lo: f64, hi: f64| if hi > lo { ((v - lo) / (hi - lo) * 255.0).round().clamp(0.0, 255.0) as u8 } else { 128 };
            Some(DLeafAmbientLighting {
                cube: CompressedLightCube { color: cube.map(encode) },
                x: rel(p.x, min.x, max.x),
                y: rel(p.y, min.y, max.y),
                z: rel(p.z, min.z, max.z),
                pad: 0,
            })
        })
        .collect();
    drop(ph);
    let mut index = Vec::with_capacity(cubes.len());
    let mut samples = Vec::new();
    for c in cubes {
        let first = samples.len() as u16;
        match c {
            Some(s) => {
                samples.push(s);
                index.push(DLeafAmbientIndex { ambient_sample_count: 1, first_ambient_sample: first });
            }
            None => index.push(DLeafAmbientIndex { ambient_sample_count: 0, first_ambient_sample: first }),
        }
    }
    bsp.set(lump::LEAF_AMBIENT_INDEX, &index);
    bsp.set(lump::LEAF_AMBIENT_LIGHTING, &samples);
    bsp.set(lump::LEAF_AMBIENT_INDEX_HDR, &index);
    bsp.set(lump::LEAF_AMBIENT_LIGHTING_HDR, &samples);
    Ok(())
}

/// VERTNORMALS / VERTNORMALINDICES: flat normals, smoothed across faces sharing a smoothing
/// group.
fn write_vertex_normals(bsp: &mut BspFile, scene: &Scene) -> Result<()> {
    let dfaces: Vec<DFace> = bsp.get(lump::FACES)?;
    let edges: Vec<DEdge> = bsp.get(lump::EDGES)?;
    let surfedges: Vec<i32> = bsp.get(lump::SURFEDGES)?;
    let vert_of = |df: &DFace, k: usize| {
        let se = surfedges[df.firstedge as usize + k];
        if se >= 0 { edges[se as usize].v[0] } else { edges[(-se) as usize].v[1] }
    };
    let mut by_vert: HashMap<u16, Vec<(usize, u32)>> = HashMap::new();
    for (fi, df) in dfaces.iter().enumerate() {
        if df.smoothing_groups == 0 {
            continue;
        }
        for k in 0..df.numedges as usize {
            by_vert.entry(vert_of(df, k)).or_default().push((fi, df.smoothing_groups));
        }
    }
    let mut normals: Vec<Vec3> = Vec::new();
    let mut lookup: HashMap<[i32; 3], u16> = HashMap::new();
    let mut indices: Vec<u16> = Vec::new();
    for (fi, df) in dfaces.iter().enumerate() {
        let n0 = scene.faces[fi].normal();
        for k in 0..df.numedges as usize {
            let mut n = n0;
            if df.smoothing_groups != 0 {
                if let Some(list) = by_vert.get(&vert_of(df, k)) {
                    n = DVec3::ZERO;
                    for &(o, g) in list {
                        if g & df.smoothing_groups != 0 {
                            n += scene.faces[o].normal() * scene.faces[o].winding.area().max(1.0);
                        }
                    }
                    n = n.normalize_or(n0);
                }
            }
            let key = [(n.x * 1e4) as i32, (n.y * 1e4) as i32, (n.z * 1e4) as i32];
            let i = *lookup.entry(key).or_insert_with(|| {
                normals.push(n.into());
                (normals.len() - 1) as u16
            });
            indices.push(i);
        }
    }
    bsp.set(lump::VERTNORMALS, &normals);
    bsp.set(lump::VERTNORMALINDICES, &indices);
    Ok(())
}

/// Leaves that can see sky faces get LEAF_FLAGS_SKY (3D sky) / LEAF_FLAGS_SKY2D.
fn set_sky_flags(bsp: &mut BspFile, scene: &Scene) -> Result<()> {
    let mut sky3d = vec![false; scene.num_clusters];
    let mut sky2d = vec![false; scene.num_clusters];
    for (fi, f) in scene.faces.iter().enumerate() {
        if f.flags & surf::SKY == 0 {
            continue;
        }
        for &c in &scene.face_clusters[fi] {
            if c >= 0 {
                if f.flags & surf::SKY2D != 0 {
                    sky2d[c as usize] = true;
                } else {
                    sky3d[c as usize] = true;
                }
            }
        }
    }
    let mut leafs: Vec<DLeaf> = bsp.get(lump::LEAFS)?;
    for l in &mut leafs {
        if l.cluster < 0 {
            continue;
        }
        let c = l.cluster as i32;
        let (mut s3, mut s2) = (false, false);
        for o in 0..scene.num_clusters as i32 {
            if scene.cluster_sees(c, o) {
                s3 |= sky3d[o as usize];
                s2 |= sky2d[o as usize];
            }
        }
        let flags = (s3 as i32) | ((s2 as i32) << 2);
        l.set_area_flags(l.area(), flags);
    }
    bsp.set(lump::LEAFS, &leafs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgbe_matches_valve() {
        // Valve wrote 0.0996 as cc cc cc f5.
        let c = encode_raw(DVec3::splat(204.0 * 2f64.powi(-11)));
        assert_eq!((c.r, c.exponent), (0xcc, -11));
        let v = DVec3::new(0.3, 1.7, 12.0);
        assert!((decode(encode_raw(v)) - v).abs().max_element() < 0.05);
    }
}
