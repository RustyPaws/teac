//! `info_overlay` -> OVERLAYS (+ OVERLAY_FADES / OVERLAY_SYSTEM_LEVELS).
//!
//! The faces of an overlay are the emitted faces made from the brush sides it lists. The
//! texinfo is a dedicated one (all vectors zero, offsets -99999) and the basis U vector is
//! stored in the z components of the four UV points, as vbsp does.

use super::load::Level;
use crate::bspfile::{DOverlay, DOverlayFade, DOverlaySystemLevel, TexInfo, Vec3, OVERLAY_BSP_FACE_COUNT};
use crate::ctx::Ctx;
use crate::material::Materials;
use crate::vmf::parse_vec3;
use glam::DVec3;
use std::collections::HashMap;

pub struct Overlays {
    pub overlays: Vec<DOverlay>,
    pub fades: Vec<DOverlayFade>,
    pub levels: Vec<DOverlaySystemLevel>,
}

pub fn build(lv: &mut Level, mats: &Materials, face_sides: &[Vec<i32>], ctx: &Ctx) -> Overlays {
    let mut by_side: HashMap<i32, Vec<i32>> = HashMap::new();
    for (f, sides) in face_sides.iter().enumerate() {
        for &sid in sides {
            by_side.entry(sid).or_default().push(f as i32);
        }
    }
    let mut out = Overlays { overlays: Vec::new(), fades: Vec::new(), levels: Vec::new() };
    let ents: Vec<usize> = (0..lv.entities.len()).filter(|&i| lv.entities[i].is("info_overlay")).collect();
    for ei in ents {
        let e = &lv.entities[ei];
        let v = |k: &str| e.get(k).and_then(parse_vec3).unwrap_or(DVec3::ZERO);
        let f = |k: &str, d: f64| e.get(k).and_then(|x| x.trim().parse::<f64>().ok()).unwrap_or(d);
        let Some(material) = e.get("material").map(str::to_string) else { continue };
        let mut faces: Vec<i32> = Vec::new();
        for sid in e.get("sides").unwrap_or("").split_whitespace().filter_map(|t| t.parse::<i32>().ok()) {
            if let Some(list) = by_side.get(&sid) {
                faces.extend(list);
            }
        }
        faces.sort_unstable();
        faces.dedup();
        if faces.is_empty() {
            ctx.detail(format!("info_overlay (id {}): no faces", e.id));
            continue;
        }
        if faces.len() > OVERLAY_BSP_FACE_COUNT {
            ctx.warn(format!("info_overlay (id {}): {} faces, only {OVERLAY_BSP_FACE_COUNT} kept", e.id, faces.len()));
            faces.truncate(OVERLAY_BSP_FACE_COUNT);
        }
        let basis_u = v("BasisU");
        let mut uv = [DVec3::ZERO; 4];
        for (i, p) in uv.iter_mut().enumerate() {
            *p = v(&format!("uv{i}"));
            p.z = if i < 3 { basis_u[i] } else { 0.0 };
        }
        let render_order = (f("RenderOrder", 0.0) as u16).min(3);
        let (fade_min, fade_max) = (f("fademindist", -1.0), f("fademaxdist", 0.0));
        let id = out.overlays.len() as i32;
        let mat = mats.get(&material);
        let td = lv.texdata.find(&material, &mat);
        let ti = lv.texinfo.find(TexInfo {
            texture_vecs: [[0.0, 0.0, 0.0, -99999.0]; 2],
            lightmap_vecs: [[0.0, 0.0, 0.0, -99999.0]; 2],
            flags: 0,
            texdata: td as i32,
        });
        let mut ofaces = [0i32; OVERLAY_BSP_FACE_COUNT];
        ofaces[..faces.len()].copy_from_slice(&faces);
        let e = &lv.entities[ei];
        let origin = e.get("BasisOrigin").and_then(parse_vec3).unwrap_or_else(|| e.vec3("origin").unwrap_or(DVec3::ZERO));
        let normal = e.get("BasisNormal").and_then(parse_vec3).unwrap_or(DVec3::Z);
        out.overlays.push(DOverlay {
            id,
            texinfo: ti as i16,
            face_count_and_render_order: faces.len() as u16 | (render_order << 14),
            ofaces,
            u: [f("StartU", 0.0) as f32, f("EndU", 1.0) as f32],
            v: [f("StartV", 0.0) as f32, f("EndV", 1.0) as f32],
            uv_points: uv.map(Vec3::from),
            origin: origin.into(),
            basis_normal: normal.into(),
        });
        out.fades.push(DOverlayFade {
            fade_dist_min_sq: if fade_min < 0.0 { -1.0 } else { (fade_min * fade_min) as f32 },
            fade_dist_max_sq: (fade_max.max(0.0) * fade_max.max(0.0)) as f32,
        });
        out.levels.push(DOverlaySystemLevel::default());
    }
    out
}
