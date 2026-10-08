//! Area portals: AREAS / AREAPORTALS / CLIPPORTALVERTS and the entities' `portalnumber`.

use super::load::Level;
use super::portals::PortalAreas;
use super::tree::Tree;
use crate::bspfile::{DArea, DAreaPortal, Vec3};
use crate::ctx::Ctx;
use crate::math::Winding;

pub struct AreaLumps {
    pub areas: Vec<DArea>,
    pub portals: Vec<DAreaPortal>,
    pub verts: Vec<Vec3>,
}

struct Emitted {
    key: u16,
    areas: [i32; 2],
    /// Plane (even/odd) with its front facing area 0's side, and the clip polygon.
    plane: usize,
    winding: Winding,
}

pub fn build(lv: &mut Level, tree: &Tree, info: &[PortalAreas], num_areas: i32, ctx: &Ctx) -> AreaLumps {
    let mut verts: Vec<Vec3> = Vec::new();
    let mut list: Vec<Emitted> = Vec::new();
    let mut key = 0u16;
    for ei in 0..lv.entities.len() {
        let e = &lv.entities[ei];
        if !(e.is("func_areaportal") || e.is("func_areaportalwindow")) {
            continue;
        }
        let pa = info[ei];
        if pa.areas[0] == 0 || pa.areas[1] == 0 || pa.extra {
            ctx.warn(format!("{} (id {}) does not separate exactly two areas", e.classname(), e.id));
            continue;
        }
        // Brushes of this portal: owned by the world model but tagged with this entity.
        let brushes: Vec<usize> = (0..lv.brushes.len()).filter(|&b| lv.brushes[b].entity == ei).collect();
        // The largest side is the portal polygon.
        let mut best: Option<(f64, usize, Winding)> = None;
        for &b in &brushes {
            for s in &lv.brushes[b].sides {
                if let Some(w) = &s.winding {
                    let a = w.area();
                    if best.as_ref().is_none_or(|x| a > x.0) {
                        best = Some((a, s.plane, w.clone()));
                    }
                }
            }
        }
        let Some((_, mut plane, w)) = best else { continue };
        // Orient the plane so that area 0's sample leaf is in front.
        let sample = tree.nodes[pa.sample[0]].bounds.center();
        if lv.planes[plane].distance(sample) < 0.0 {
            plane ^= 1;
        }
        key += 1;
        list.push(Emitted { key, areas: pa.areas, plane, winding: w });
        lv.entities[ei].set("portalnumber", key.to_string());
    }
    let mut areas = vec![DArea::default(); num_areas as usize + 1];
    // Entry 0 is a zeroed dummy, as in Valve's maps.
    let mut portals = vec![DAreaPortal::default()];
    for a in 1..=num_areas {
        areas[a as usize].firstareaportal = portals.len() as i32;
        for p in &list {
            let (other, plane, w) = if p.areas[0] == a {
                (p.areas[1], p.plane, p.winding.clone())
            } else if p.areas[1] == a {
                (p.areas[0], p.plane ^ 1, p.winding.reversed())
            } else {
                continue;
            };
            let first = verts.len() as u16;
            verts.extend(w.points.iter().map(|&p| Vec3::from(p)));
            portals.push(DAreaPortal {
                portal_key: p.key,
                other_area: other as u16,
                first_clip_portal_vert: first,
                clip_portal_verts: w.points.len() as u16,
                planenum: plane as i32,
            });
        }
        areas[a as usize].numareaportals = portals.len() as i32 - areas[a as usize].firstareaportal;
    }
    AreaLumps { areas, portals, verts }
}
