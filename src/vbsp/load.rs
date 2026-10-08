//! VMF entities -> compile brushes: planes, texinfo/texdata, side contents and flags,
//! brush windings, bevels, brush-entity bookkeeping.

use super::BspOptions;
use crate::bspfile::{TexInfo, DTexData, Vec3};
use crate::ctx::Ctx;
use crate::flags::{contents, surf};
use crate::material::{Material, Materials};
use crate::math::{Aabb, Plane, PlaneSet, Winding};
use crate::vmf::{self, Map};
use glam::DVec3;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct BrushSide {
    pub plane: usize,
    pub texinfo: i32,
    pub winding: Option<Winding>,
    pub contents: i32,
    pub surf: i32,
    pub visible: bool,
    pub bevel: bool,
    /// VMF side id (for overlays/cubemaps `sides` lists).
    pub id: i32,
    pub smoothing: u32,
    pub material: Option<Arc<Material>>,
    /// Material name as spelled in the VMF (texdata names keep it).
    pub material_name: String,
    /// Raw `dispinfo` of a displacement side.
    pub dispinfo: Option<Vec<crate::kv::Node>>,
}

#[derive(Clone, Debug)]
pub struct MapBrush {
    pub id: i32,
    /// Index into `Level::entities`.
    pub entity: usize,
    pub contents: i32,
    pub sides: Vec<BrushSide>,
    pub bounds: Aabb,
}

/// An entity as it will be written to the entity lump, with its brushes.
#[derive(Clone, Debug, Default)]
pub struct CEntity {
    pub id: i32,
    pub keys: Vec<(String, String)>,
    pub connections: Vec<(String, String)>,
    /// Brushes compiled into this entity's model (the world also owns func_detail and
    /// func_areaportal brushes).
    pub brushes: Vec<usize>,
    /// Offset subtracted from the brushes (brush entities with an `origin`).
    pub origin: DVec3,
    /// Model index once emitted (`*N`).
    pub model: Option<usize>,
}

impl CEntity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }
    pub fn is(&self, c: &str) -> bool {
        self.classname().eq_ignore_ascii_case(c)
    }
    pub fn set(&mut self, key: &str, val: impl Into<String>) {
        let val = val.into();
        match self.keys.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(kv) => kv.1 = val,
            None => self.keys.push((key.to_string(), val)),
        }
    }
    pub fn remove(&mut self, key: &str) {
        self.keys.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
    }
    pub fn vec3(&self, key: &str) -> Option<DVec3> {
        self.get(key).and_then(vmf::parse_vec3)
    }
}

#[derive(Default)]
pub struct TexDataSet {
    pub data: Vec<DTexData>,
    pub names: Vec<String>,
    index: HashMap<String, usize>,
}

impl TexDataSet {
    /// Texdata for a material; `display` is the name written to the string table.
    pub fn find(&mut self, display: &str, m: &Material) -> usize {
        let key = crate::material::normalize_name(display);
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.data.len();
        self.data.push(DTexData {
            reflectivity: m.reflectivity.into(),
            name_string_table_id: i as i32,
            width: m.width,
            height: m.height,
            view_width: m.width,
            view_height: m.height,
        });
        self.names.push(display.replace('\\', "/"));
        self.index.insert(key, i);
        i
    }
}

#[derive(Default)]
pub struct TexInfoSet {
    pub list: Vec<TexInfo>,
    index: HashMap<Vec<u32>, usize>,
}

impl TexInfoSet {
    pub fn find(&mut self, t: TexInfo) -> usize {
        let mut key: Vec<u32> = t.texture_vecs.iter().chain(&t.lightmap_vecs).flatten().map(|f| f.to_bits()).collect();
        key.push(t.flags as u32);
        key.push(t.texdata as u32);
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.list.len();
        self.list.push(t);
        self.index.insert(key, i);
        i
    }
}

/// Everything vbsp works on.
pub struct Level {
    pub planes: PlaneSet,
    pub brushes: Vec<MapBrush>,
    pub entities: Vec<CEntity>,
    pub texinfo: TexInfoSet,
    pub texdata: TexDataSet,
    /// World brush bounds.
    pub bounds: Aabb,
}

impl Level {
    pub fn world(&self) -> &CEntity {
        &self.entities[0]
    }
}

/// Classes whose brushes are merged into the world.
fn world_brush_class(class: &str) -> Option<i32> {
    match class.to_ascii_lowercase().as_str() {
        "func_detail" => Some(contents::DETAIL),
        "func_areaportal" | "func_areaportalwindow" => Some(contents::AREAPORTAL),
        _ => None,
    }
}

/// Entity classes vbsp consumes and drops from the entity lump.
pub fn removed_class(class: &str) -> bool {
    matches!(
        class.to_ascii_lowercase().as_str(),
        "func_detail" | "func_instance" | "func_instance_parms" | "func_viscluster" | "info_overlay"
            | "info_overlay_transition" | "env_cubemap" | "prop_static" | "prop_detail" | "info_no_dynamic_shadow"
            | "func_instance_origin"
    )
}

fn texinfo_for_side(
    side: &vmf::Side,
    mat: &Material,
    texdata: usize,
    origin: DVec3,
) -> TexInfo {
    let mut t = TexInfo { flags: mat.flags, texdata: texdata as i32, ..Default::default() };
    let lm_scale = if side.lightmap_scale > 0 { side.lightmap_scale as f64 } else { 16.0 };
    for (i, ax) in [&side.uaxis, &side.vaxis].into_iter().enumerate() {
        let scale = if ax.scale.abs() < 1e-6 { 1.0 } else { ax.scale };
        let tv = ax.vec / scale;
        t.texture_vecs[i] = [tv.x as f32, tv.y as f32, tv.z as f32, (ax.shift + origin.dot(tv)) as f32];
        let lv = ax.vec.normalize_or_zero() / lm_scale;
        t.lightmap_vecs[i] = [lv.x as f32, lv.y as f32, lv.z as f32, origin.dot(lv) as f32];
    }
    t
}

/// vbsp `BrushContents`: the first side decides; see-through contents anywhere make the brush
/// translucent and non-solid.
fn brush_contents(sides: &[BrushSide]) -> i32 {
    let Some(first) = sides.first() else { return 0 };
    let mut c = first.contents;
    let union = sides.iter().fold(0, |u, s| u | s.contents);
    let mut trans = 0;
    for s in sides {
        trans |= s.surf;
    }
    let see_through = union & (contents::WINDOW | contents::GRATE | contents::WATER | contents::SLIME);
    if see_through != 0 {
        c |= see_through | contents::TRANSLUCENT;
        c &= !contents::SOLID;
    }
    let _ = trans;
    c
}

/// Builds windings for every side (vbsp `MakeBrushWindings`) and the brush bounds.
pub fn make_windings(b: &mut MapBrush, planes: &PlaneSet) {
    let mut bounds = Aabb::EMPTY;
    let ps: Vec<(usize, bool)> = b.sides.iter().map(|s| (s.plane, s.bevel)).collect();
    for (i, s) in b.sides.iter_mut().enumerate() {
        if s.bevel {
            continue;
        }
        let mut w = Some(Winding::base(&planes[s.plane]));
        for (j, &(pj, bevel)) in ps.iter().enumerate() {
            if i == j || bevel {
                continue;
            }
            w = w.and_then(|w| w.clip(&planes[pj ^ 1], 0.0, false));
        }
        if let Some(w) = &w {
            s.visible = true;
            for &p in &w.points {
                bounds.add(p);
            }
        }
        s.winding = w;
    }
    b.bounds = bounds;
}

/// vbsp `AddBrushBevels`: axial planes in canonical order, then edge bevels.
pub fn add_bevels(b: &mut MapBrush, planes: &mut PlaneSet) {
    let mut order = 0;
    for axis in 0..3 {
        for dir in [-1.0f64, 1.0] {
            let i = b.sides.iter().position(|s| planes[s.plane].normal[axis] == dir);
            let i = match i {
                Some(i) => i,
                None => {
                    let mut n = DVec3::ZERO;
                    n[axis] = dir;
                    let dist = if dir > 0.0 { b.bounds.max[axis] } else { -b.bounds.min[axis] };
                    let first = &b.sides[0];
                    b.sides.push(BrushSide {
                        plane: planes.find(Plane::new(n, dist)),
                        texinfo: first.texinfo,
                        winding: None,
                        contents: first.contents,
                        surf: first.surf,
                        visible: false,
                        bevel: true,
                        id: -1,
                        smoothing: 0,
                        material: first.material.clone(),
                        material_name: first.material_name.clone(),
                        dispinfo: None,
                    });
                    b.sides.len() - 1
                }
            };
            if i != order {
                b.sides.swap(i, order);
            }
            order += 1;
        }
    }
    if b.sides.len() == 6 {
        return;
    }
    let mut i = 6;
    while i < b.sides.len() {
        let Some(w) = b.sides[i].winding.clone() else {
            i += 1;
            continue;
        };
        let n = w.points.len();
        for j in 0..n {
            let mut vec = w.points[j] - w.points[(j + 1) % n];
            if vec.length() < 0.5 {
                continue;
            }
            vec = snap_vector(vec.normalize());
            if (0..3).any(|k| vec[k] == -1.0 || vec[k] == 1.0) {
                continue;
            }
            for axis in 0..3 {
                for dir in [-1.0f64, 1.0] {
                    let mut v2 = DVec3::ZERO;
                    v2[axis] = dir;
                    let normal = vec.cross(v2);
                    if normal.length() < 0.5 {
                        continue;
                    }
                    let normal = normal.normalize();
                    let dist = w.points[j].dot(normal);
                    let probe = Plane::new(normal, dist);
                    let mut ok = true;
                    for s in &b.sides {
                        let p = &planes[s.plane];
                        if (p.normal - normal).abs().max_element() < 1e-5 && (p.dist - dist).abs() < 0.01 {
                            ok = false;
                            break;
                        }
                        if let Some(w2) = &s.winding {
                            if w2.points.iter().any(|&q| probe.distance(q) > 0.1) {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    let first = &b.sides[0];
                    let side = BrushSide {
                        plane: planes.find(probe),
                        texinfo: first.texinfo,
                        winding: None,
                        contents: first.contents,
                        surf: first.surf,
                        visible: false,
                        bevel: true,
                        id: -1,
                        smoothing: 0,
                        material: first.material.clone(),
                        material_name: first.material_name.clone(),
                        dispinfo: None,
                    };
                    b.sides.push(side);
                }
            }
        }
        i += 1;
    }
}

fn snap_vector(mut v: DVec3) -> DVec3 {
    for i in 0..3 {
        if (v[i] - 1.0).abs() < 1e-5 {
            v = DVec3::ZERO;
            v[i] = 1.0;
            break;
        }
        if (v[i] + 1.0).abs() < 1e-5 {
            v = DVec3::ZERO;
            v[i] = -1.0;
            break;
        }
    }
    v
}

pub fn load(map: &Map, mats: &Materials, opts: &BspOptions, ctx: &Ctx) -> Level {
    let mut lv = Level {
        planes: PlaneSet::default(),
        brushes: Vec::new(),
        entities: Vec::new(),
        texinfo: TexInfoSet::default(),
        texdata: TexDataSet::default(),
        bounds: Aabb::EMPTY,
    };
    lv.entities.push(CEntity { id: map.world.id, keys: map.world.keys.clone(), connections: map.world.connections.clone(), ..Default::default() });
    for s in &map.world.solids {
        if let Some(b) = add_brush(&mut lv, s, 0, 0, DVec3::ZERO, mats, ctx) {
            lv.entities[0].brushes.push(b);
        }
    }
    for e in &map.entities {
        let class = e.classname();
        let to_world = world_brush_class(class);
        if e.is("func_detail") {
            // Merged into the world; the entity itself disappears.
            if !opts.nodetail {
                for s in &e.solids {
                    if let Some(b) = add_brush(&mut lv, s, 0, contents::DETAIL, DVec3::ZERO, mats, ctx) {
                        lv.entities[0].brushes.push(b);
                    }
                }
            }
            continue;
        }
        let ci = lv.entities.len();
        let origin = if e.solids.is_empty() || to_world.is_some() { DVec3::ZERO } else { e.origin() };
        lv.entities.push(CEntity { id: e.id, keys: e.keys.clone(), connections: e.connections.clone(), origin, ..Default::default() });
        for s in &e.solids {
            if let Some(b) = add_brush(&mut lv, s, ci, to_world.unwrap_or(0), origin, mats, ctx) {
                let owner = if to_world.is_some() { 0 } else { ci };
                lv.entities[owner].brushes.push(b);
            }
        }
    }

    if opts.nowater {
        let water: Vec<bool> = lv.brushes.iter().map(|b| b.contents & (contents::WATER | contents::SLIME) != 0).collect();
        for e in &mut lv.entities {
            e.brushes.retain(|&b| !water[b]);
        }
    }
    let origin_brushes = lv.brushes.iter().filter(|b| b.contents & contents::ORIGIN != 0).count();
    if origin_brushes > 0 {
        ctx.warn(format!("{origin_brushes} origin brush(es) ignored (the entity origin key is used instead)"));
    }
    for &b in &lv.entities[0].brushes {
        lv.bounds = lv.bounds.union(&lv.brushes[b].bounds);
    }
    lv
}

/// Adds a solid; `entity` is the owner (area portal brushes keep their entity although they
/// are compiled into the world). Returns the brush index.
fn add_brush(lv: &mut Level, s: &vmf::Solid, entity: usize, extra: i32, origin: DVec3, mats: &Materials, ctx: &Ctx) -> Option<usize> {
    let mut sides: Vec<BrushSide> = Vec::with_capacity(s.sides.len() + 6);
    for side in &s.sides {
        let pts = side.plane.map(|p| p - origin);
        let Some(plane) = Plane::from_points_snapped(&pts) else {
            ctx.warn(format!("brush {}: side {} has a degenerate plane", s.id, side.id));
            continue;
        };
        let pi = lv.planes.find(plane);
        if sides.iter().any(|o| o.plane == pi) {
            ctx.detail(format!("brush {}: duplicate plane on side {}", s.id, side.id));
            continue;
        }
        if sides.iter().any(|o| o.plane == pi ^ 1) {
            ctx.warn(format!("brush {}: mirrored plane on side {}", s.id, side.id));
            return None;
        }
        let mat = mats.get(&side.material);
        let td = lv.texdata.find(&side.material, &mat);
        let ti = lv.texinfo.find(texinfo_for_side(side, &mat, td, origin));
        sides.push(BrushSide {
            plane: pi,
            texinfo: ti as i32,
            winding: None,
            contents: mat.contents,
            surf: mat.flags,
            visible: false,
            bevel: false,
            id: side.id,
            smoothing: side.smoothing_groups,
            material: Some(mat),
            material_name: side.material.replace('\\', "/"),
            dispinfo: side.dispinfo.clone(),
        });
    }
    if sides.len() < 4 {
        ctx.warn(format!("brush {}: fewer than 4 valid sides, removed", s.id));
        return None;
    }
    let mut contents = brush_contents(&sides) | extra;
    if extra == contents::AREAPORTAL {
        contents = contents::AREAPORTAL;
    }
    // Clip brushes are always detail.
    if contents & (contents::PLAYERCLIP | contents::MONSTERCLIP) != 0 {
        contents |= contents::DETAIL;
    }
    let mut b = MapBrush { id: s.id, entity, contents, sides, bounds: Aabb::EMPTY };
    make_windings(&mut b, &lv.planes);
    if b.bounds.is_empty() || b.sides.iter().all(|s| s.winding.is_none()) {
        ctx.warn(format!("brush {}: no visible sides, removed", s.id));
        return None;
    }
    if b.bounds.min.min_element() < -crate::math::MAX_COORD || b.bounds.max.max_element() > crate::math::MAX_COORD {
        ctx.warn(format!("brush {}: outside the map bounds", s.id));
    }
    add_bevels(&mut b, &mut lv.planes);
    lv.brushes.push(b);
    Some(lv.brushes.len() - 1)
}

/// Texture/lightmap axes are unused by plane math but kept for overlay projection.
pub fn tex_vecs(t: &TexInfo) -> ([DVec3; 2], [f64; 2]) {
    let v = |i: usize| DVec3::new(t.texture_vecs[i][0] as f64, t.texture_vecs[i][1] as f64, t.texture_vecs[i][2] as f64);
    ([v(0), v(1)], [t.texture_vecs[0][3] as f64, t.texture_vecs[1][3] as f64])
}

pub fn vec3(v: DVec3) -> Vec3 {
    v.into()
}

pub const NODRAW_LIKE: i32 = surf::NODRAW | surf::SKIP | surf::HINT;
