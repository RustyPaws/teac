//! `prop_static` -> the `sprp` game lump (version 9: 72-byte records), with the leaves each
//! prop touches.

use super::load::Level;
use super::tree::Tree;
use crate::bspfile::Vec3;
use crate::ctx::Ctx;
use crate::flags::contents;
use crate::fs::GameFs;
use crate::math::{angles_matrix, Aabb, PlaneSet};
use glam::DVec3;
use std::collections::HashMap;

pub const FLAG_FADES: u8 = 0x1;
pub const FLAG_USE_LIGHTING_ORIGIN: u8 = 0x2;
pub const FLAG_IGNORE_NORMALS: u8 = 0x8;
pub const FLAG_NO_SHADOW: u8 = 0x10;
pub const FLAG_SCREEN_SPACE_FADE: u8 = 0x20;
pub const FLAG_NO_PER_VERTEX_LIGHTING: u8 = 0x40;
pub const FLAG_NO_SELF_SHADOWING: u8 = 0x80;

/// A static prop as vrad also needs it (model, transform, flags).
#[derive(Clone, Debug)]
pub struct StaticProp {
    pub model: String,
    pub origin: DVec3,
    pub angles: DVec3,
    pub skin: i32,
    pub solid: u8,
    pub flags: u8,
    pub fade_min: f32,
    pub fade_max: f32,
    pub lighting_origin: DVec3,
    pub fade_scale: f32,
    pub color: [u8; 4],
    pub leaves: Vec<u16>,
}

/// Hull (or view bbox) of a studio model from its header.
pub fn model_bounds(fs: &GameFs, model: &str) -> Option<(DVec3, DVec3)> {
    let b = fs.read(model)?;
    if b.get(0..4)? != b"IDST" || b.len() < 152 {
        return None;
    }
    let v = |o: usize| DVec3::new(
        f32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as f64,
        f32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as f64,
        f32::from_le_bytes(b[o + 8..o + 12].try_into().unwrap()) as f64,
    );
    let (hmin, hmax) = (v(104), v(116));
    if hmax.cmpgt(hmin).all() {
        return Some((hmin, hmax));
    }
    let (vmin, vmax) = (v(128), v(140));
    vmax.cmpgt(vmin).all().then_some((vmin, vmax))
}

fn rotated_bounds(min: DVec3, max: DVec3, origin: DVec3, angles: DVec3) -> Aabb {
    let m = angles_matrix(angles);
    let mut b = Aabb::EMPTY;
    for i in 0..8 {
        let c = DVec3::new(if i & 1 != 0 { max.x } else { min.x }, if i & 2 != 0 { max.y } else { min.y }, if i & 4 != 0 { max.z } else { min.z });
        b.add(m * c + origin);
    }
    b
}

/// Non-solid leaves of the (emitted) world tree that a box touches.
fn leaves_in_box(tree: &Tree, planes: &PlaneSet, b: &Aabb, leaf_map: &HashMap<usize, usize>) -> Vec<u16> {
    let mut out = Vec::new();
    let mut stack = vec![tree.head];
    while let Some(n) = stack.pop() {
        let node = &tree.nodes[n];
        match node.plane {
            None => {
                if node.contents & contents::SOLID == 0 {
                    if let Some(&l) = leaf_map.get(&n) {
                        out.push(l as u16);
                    }
                }
            }
            Some(p) => {
                let s = super::brush::box_on_plane_side(b, &planes[p]);
                if s & super::brush::PSIDE_FRONT != 0 {
                    stack.push(node.children[0]);
                }
                if s & super::brush::PSIDE_BACK != 0 {
                    stack.push(node.children[1]);
                }
            }
        }
    }
    out.sort_unstable();
    out
}

pub fn collect(lv: &Level, fs: &GameFs, tree: &Tree, leaf_map: &HashMap<usize, usize>, ctx: &Ctx) -> Vec<StaticProp> {
    let mut props = Vec::new();
    let targets: HashMap<String, DVec3> = lv
        .entities
        .iter()
        .filter(|e| e.is("info_lighting"))
        .filter_map(|e| Some((e.get("targetname")?.to_ascii_lowercase(), e.vec3("origin")?)))
        .collect();
    let mut bounds_cache: HashMap<String, Option<(DVec3, DVec3)>> = HashMap::new();
    for e in lv.entities.iter().filter(|e| e.is("prop_static")) {
        let Some(model) = e.get("model").map(|m| m.replace('\\', "/").to_ascii_lowercase()) else { continue };
        let origin = e.vec3("origin").unwrap_or(DVec3::ZERO);
        let angles = e.vec3("angles").unwrap_or(DVec3::ZERO);
        let bounds = *bounds_cache.entry(model.clone()).or_insert_with(|| model_bounds(fs, &model));
        let Some((min, max)) = bounds else {
            ctx.warn(format!("prop_static (id {}): can't load model {model}", e.id));
            continue;
        };
        let num = |k: &str, d: f64| e.get(k).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(d);
        let on = |k: &str| num(k, 0.0) != 0.0;
        let mut flags = 0;
        let fade_min = num("fademindist", 0.0).max(0.0) as f32;
        let fade_max = num("fademaxdist", 0.0).max(0.0) as f32;
        if fade_max > 0.0 {
            flags |= FLAG_FADES;
        }
        if on("ignorenormals") {
            flags |= FLAG_IGNORE_NORMALS;
        }
        if on("disableshadows") {
            flags |= FLAG_NO_SHADOW;
        }
        if on("screenspacefade") {
            flags |= FLAG_SCREEN_SPACE_FADE;
        }
        if on("disablevertexlighting") {
            flags |= FLAG_NO_PER_VERTEX_LIGHTING;
        }
        if on("disableselfshadowing") {
            flags |= FLAG_NO_SELF_SHADOWING;
        }
        let mut lighting_origin = DVec3::ZERO;
        if let Some(t) = e.get("lightingorigin").filter(|t| !t.is_empty()) {
            match targets.get(&t.to_ascii_lowercase()) {
                Some(&o) => {
                    lighting_origin = o;
                    flags |= FLAG_USE_LIGHTING_ORIGIN;
                }
                None => ctx.warn(format!("prop_static (id {}): lighting origin {t:?} not found", e.id)),
            }
        }
        let color = e.get("rendercolor").and_then(crate::vmf::parse_vec3).unwrap_or(DVec3::splat(255.0));
        let alpha = num("renderamt", 255.0);
        let b = rotated_bounds(min, max, origin, angles);
        props.push(StaticProp {
            model,
            origin,
            angles,
            skin: num("skin", 0.0) as i32,
            solid: num("solid", 6.0) as u8,
            flags,
            fade_min,
            fade_max,
            lighting_origin,
            fade_scale: num("fadescale", 1.0) as f32,
            color: [color.x as u8, color.y as u8, color.z as u8, alpha.clamp(0.0, 255.0) as u8],
            leaves: leaves_in_box(tree, &lv.planes, &b, leaf_map),
        });
    }
    props
}

/// Serializes the `sprp` game lump.
pub fn lump(props: &[StaticProp]) -> Vec<u8> {
    let mut dict: Vec<String> = Vec::new();
    let mut index: HashMap<&str, u16> = HashMap::new();
    for p in props {
        if !index.contains_key(p.model.as_str()) {
            index.insert(&p.model, dict.len() as u16);
            dict.push(p.model.clone());
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(dict.len() as i32).to_le_bytes());
    for n in &dict {
        let mut name = [0u8; 128];
        let b = n.as_bytes();
        name[..b.len().min(127)].copy_from_slice(&b[..b.len().min(127)]);
        out.extend_from_slice(&name);
    }
    let total_leaves: usize = props.iter().map(|p| p.leaves.len()).sum();
    out.extend_from_slice(&(total_leaves as i32).to_le_bytes());
    for p in props {
        for &l in &p.leaves {
            out.extend_from_slice(&l.to_le_bytes());
        }
    }
    out.extend_from_slice(&(props.len() as i32).to_le_bytes());
    let mut first_leaf = 0u16;
    for p in props {
        let push_v = |o: &mut Vec<u8>, v: DVec3| o.extend_from_slice(bytemuck::bytes_of(&Vec3::from(v)));
        push_v(&mut out, p.origin);
        push_v(&mut out, p.angles);
        out.extend_from_slice(&index[p.model.as_str()].to_le_bytes());
        out.extend_from_slice(&first_leaf.to_le_bytes());
        out.extend_from_slice(&(p.leaves.len() as u16).to_le_bytes());
        out.push(p.solid);
        out.push(p.flags);
        out.extend_from_slice(&p.skin.to_le_bytes());
        out.extend_from_slice(&p.fade_min.to_le_bytes());
        out.extend_from_slice(&p.fade_max.to_le_bytes());
        push_v(&mut out, p.lighting_origin);
        out.extend_from_slice(&p.fade_scale.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]); // min/max CPU and GPU level
        out.extend_from_slice(&p.color);
        out.extend_from_slice(&[0, 0, 0, 0]); // disable on X360
        first_leaf += p.leaves.len() as u16;
    }
    out
}
