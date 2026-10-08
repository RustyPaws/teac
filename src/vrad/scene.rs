//! The lighting scene read back from a BSP: faces in world space with luxel mappings, the
//! BSP tree for point queries, PVS, and the ray-tracing geometry.

use super::bvh::{Bvh, Tri};
use crate::bspfile::*;
use crate::flags::{contents, surf};
use crate::math::{Plane, Winding};
use anyhow::Result;
use glam::{DMat3, DVec3, Vec3};
use std::collections::HashMap;

pub struct Face {
    pub index: usize,
    pub model: usize,
    /// World-space plane.
    pub plane: Plane,
    pub offset: DVec3,
    pub tex: TexInfo,
    pub flags: i32,
    pub texdata: usize,
    pub reflectivity: DVec3,
    pub mins: [i32; 2],
    /// Luxel counts (lightmap size + 1).
    pub w: usize,
    pub h: usize,
    pub lit: bool,
    pub bump: bool,
    /// World-space polygon.
    pub winding: Winding,
    /// World position of luxel (s, t): `luxel_origin + s * luxel_s + t * luxel_t`.
    pub luxel_origin: DVec3,
    pub luxel_s: DVec3,
    pub luxel_t: DVec3,
    /// Texture-space tangent directions (for bump basis).
    pub tex_s: DVec3,
    pub tex_t: DVec3,
    pub material: String,
}

impl Face {
    pub fn normal(&self) -> DVec3 {
        self.plane.normal
    }

    pub fn luxel_pos(&self, s: f64, t: f64) -> DVec3 {
        self.luxel_origin + self.luxel_s * s + self.luxel_t * t
    }

    /// Nearest point of the face polygon to `p` (which lies on the plane).
    pub fn clamp_to_face(&self, p: DVec3) -> DVec3 {
        let n = self.normal();
        let pts = &self.winding.points;
        let mut inside = true;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            // Windings are clockwise from the front: inside is on the right of each edge.
            let edge_n = (b - a).cross(n);
            if (p - a).dot(edge_n) < -1e-6 {
                inside = false;
                break;
            }
        }
        if inside {
            return p;
        }
        let mut best = (f64::MAX, p);
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            let q = a + ab * t;
            let d = (q - p).length_squared();
            if d < best.0 {
                best = (d, q);
            }
        }
        best.1
    }
}

pub struct Scene {
    pub faces: Vec<Face>,
    pub planes: Vec<DPlane>,
    pub nodes: Vec<DNode>,
    pub leafs: Vec<DLeaf>,
    pub models: Vec<DModel>,
    pub texinfo: Vec<TexInfo>,
    pub texdata: Vec<DTexData>,
    pub texnames: Vec<String>,
    pub num_clusters: usize,
    /// Decompressed PVS per cluster (None: no vis data, everything visible).
    pub pvs: Option<Vec<Vec<u8>>>,
    /// Clusters each face is seen from.
    pub face_clusters: Vec<Vec<i32>>,
    pub bvh: Bvh,
    pub entities: Vec<Vec<(String, String)>>,
}

/// Payload of a ray-tracing triangle.
pub const TRI_SKY: u32 = 1 << 31;
pub const TRI_OPAQUE_NO_FACE: u32 = 1 << 30;

pub fn parse_entities(text: &str) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<(String, String)>> = None;
    let mut toks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                cur = Some(Vec::new());
                toks.clear();
            }
            '}' => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
            }
            '"' => {
                let mut s = String::new();
                for c2 in chars.by_ref() {
                    if c2 == '"' {
                        break;
                    }
                    s.push(c2);
                }
                toks.push(s);
                if toks.len() == 2 {
                    if let Some(e) = cur.as_mut() {
                        let v = toks.pop().unwrap();
                        let k = toks.pop().unwrap();
                        e.push((k, v));
                    }
                    toks.clear();
                }
            }
            _ => {}
        }
    }
    out
}

pub fn get<'a>(e: &'a [(String, String)], k: &str) -> Option<&'a str> {
    e.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
}

fn dv(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}

impl Scene {
    /// `extra` occluders (static props) join the ray-tracing geometry.
    pub fn load(bsp: &BspFile, extra: Vec<Tri>) -> Result<Scene> {
        let planes: Vec<DPlane> = bsp.get(lump::PLANES)?;
        let dfaces: Vec<DFace> = bsp.get(lump::FACES)?;
        let texinfo: Vec<TexInfo> = bsp.get(lump::TEXINFO)?;
        let texdata: Vec<DTexData> = bsp.get(lump::TEXDATA)?;
        let verts: Vec<crate::bspfile::Vec3> = bsp.get(lump::VERTEXES)?;
        let edges: Vec<DEdge> = bsp.get(lump::EDGES)?;
        let surfedges: Vec<i32> = bsp.get(lump::SURFEDGES)?;
        let models: Vec<DModel> = bsp.get(lump::MODELS)?;
        let nodes: Vec<DNode> = bsp.get(lump::NODES)?;
        let leafs: Vec<DLeaf> = bsp.get(lump::LEAFS)?;
        let leaffaces: Vec<u16> = bsp.get(lump::LEAFFACES)?;
        let texnames = bsp.texdata_names()?;
        let entities = parse_entities(&bsp.entities());

        // Model origins come from the entities that use them.
        let mut model_offset = vec![DVec3::ZERO; models.len()];
        for e in &entities {
            if let Some(m) = get(e, "model").and_then(|m| m.strip_prefix('*')).and_then(|m| m.parse::<usize>().ok()) {
                if m < models.len() {
                    model_offset[m] = get(e, "origin").and_then(crate::vmf::parse_vec3).unwrap_or(DVec3::ZERO);
                }
            }
        }
        let mut face_model = vec![0usize; dfaces.len()];
        for (mi, m) in models.iter().enumerate() {
            for f in m.firstface..m.firstface + m.numfaces {
                if (f as usize) < face_model.len() {
                    face_model[f as usize] = mi;
                }
            }
        }

        let mut faces = Vec::with_capacity(dfaces.len());
        for (fi, df) in dfaces.iter().enumerate() {
            let model = face_model[fi];
            let offset = model_offset[model];
            let dp = planes[df.planenum as usize];
            let mut n = dv(dp.normal.into_glam());
            let mut dist = dp.dist as f64;
            if df.side != 0 {
                n = -n;
                dist = -dist;
            }
            let plane = Plane::new(n, dist + n.dot(offset));
            let tex = texinfo[df.texinfo.max(0) as usize];
            let td = tex.texdata.max(0) as usize;
            let mut pts = Vec::with_capacity(df.numedges as usize);
            for k in 0..df.numedges as usize {
                let se = surfedges[df.firstedge as usize + k];
                let v = if se >= 0 { edges[se as usize].v[0] } else { edges[(-se) as usize].v[1] };
                pts.push(dv(verts[v as usize].into_glam()) + offset);
            }
            let lit = tex.flags & surf::NOLIGHT == 0 && df.lightmap_texture_size_in_luxels[0] >= 0 && df.texinfo >= 0;
            let lv = |i: usize| DVec3::new(tex.lightmap_vecs[i][0] as f64, tex.lightmap_vecs[i][1] as f64, tex.lightmap_vecs[i][2] as f64);
            let tv = |i: usize| DVec3::new(tex.texture_vecs[i][0] as f64, tex.texture_vecs[i][1] as f64, tex.texture_vecs[i][2] as f64);
            // Solve local point Q from (lightmap s, t, plane): rows a, b, n.
            let (a, b) = (lv(0), lv(1));
            let local_n = if df.side != 0 { -dv(dp.normal.into_glam()) } else { dv(dp.normal.into_glam()) };
            let local_dist = if df.side != 0 { -(dp.dist as f64) } else { dp.dist as f64 };
            let m = DMat3::from_cols(a, b, local_n).transpose();
            let (luxel_origin, luxel_s, luxel_t) = if m.determinant().abs() > 1e-12 {
                let inv = m.inverse();
                let q0 = inv * DVec3::new(df.lightmap_texture_mins_in_luxels[0] as f64 - tex.lightmap_vecs[0][3] as f64, df.lightmap_texture_mins_in_luxels[1] as f64 - tex.lightmap_vecs[1][3] as f64, local_dist);
                (q0 + offset, inv * DVec3::X, inv * DVec3::Y)
            } else {
                (pts.first().copied().unwrap_or_default(), DVec3::X, DVec3::Y)
            };
            let reflectivity = texdata.get(td).map(|t| dv(t.reflectivity.into_glam())).unwrap_or(DVec3::splat(0.5));
            faces.push(Face {
                index: fi,
                model,
                plane,
                offset,
                tex,
                flags: tex.flags,
                texdata: td,
                reflectivity,
                mins: df.lightmap_texture_mins_in_luxels,
                w: (df.lightmap_texture_size_in_luxels[0] + 1).max(0) as usize,
                h: (df.lightmap_texture_size_in_luxels[1] + 1).max(0) as usize,
                lit,
                bump: tex.flags & surf::BUMPLIGHT != 0,
                winding: Winding::new(pts),
                luxel_origin,
                luxel_s,
                luxel_t,
                tex_s: tv(0),
                tex_t: tv(1),
                material: texnames.get(texdata.get(td).map_or(0, |t| t.name_string_table_id as usize)).cloned().unwrap_or_default(),
            });
        }

        // PVS.
        let vis = &bsp.lumps[lump::VISIBILITY].data;
        let num_clusters = leafs.iter().map(|l| l.cluster as i32 + 1).max().unwrap_or(0).max(0) as usize;
        let pvs = if vis.len() >= 4 {
            let nc = i32::from_le_bytes(vis[0..4].try_into().unwrap()) as usize;
            let row = nc.div_ceil(8);
            Some(
                (0..nc)
                    .map(|c| {
                        let ofs = i32::from_le_bytes(vis[4 + c * 8..8 + c * 8].try_into().unwrap()) as usize;
                        crate::vvis::decompress(&vis[ofs.min(vis.len())..], row)
                    })
                    .collect(),
            )
        } else {
            None
        };
        let mut face_clusters = vec![Vec::new(); dfaces.len()];
        for l in &leafs {
            if l.cluster < 0 {
                continue;
            }
            for k in 0..l.numleaffaces as usize {
                let f = leaffaces[l.firstleafface as usize + k] as usize;
                if f < face_clusters.len() && !face_clusters[f].contains(&(l.cluster as i32)) {
                    face_clusters[f].push(l.cluster as i32);
                }
            }
        }

        let bvh = build_bvh(bsp, &faces, &planes, &texinfo, &models, extra)?;
        Ok(Scene { faces, planes, nodes, leafs, models, texinfo, texdata, texnames, num_clusters, pvs, face_clusters, bvh, entities })
    }

    /// Leaf index containing a world point (model 0 tree).
    pub fn leaf_at(&self, p: DVec3) -> usize {
        let mut n = self.models.first().map_or(0, |m| m.headnode);
        while n >= 0 {
            let node = &self.nodes[n as usize];
            let pl = &self.planes[node.planenum as usize];
            let d = p.dot(dv(pl.normal.into_glam())) - pl.dist as f64;
            n = node.children[if d >= 0.0 { 0 } else { 1 }];
        }
        (-1 - n) as usize
    }

    pub fn point_solid(&self, p: DVec3) -> bool {
        self.leafs[self.leaf_at(p)].contents & contents::SOLID != 0
    }

    pub fn cluster_at(&self, p: DVec3) -> i32 {
        self.leafs[self.leaf_at(p)].cluster as i32
    }

    /// Whether cluster `b` is potentially visible from cluster `a`.
    pub fn cluster_sees(&self, a: i32, b: i32) -> bool {
        let Some(pvs) = &self.pvs else { return true };
        if a < 0 || b < 0 {
            return true;
        }
        let row = &pvs[a as usize];
        row.get(b as usize >> 3).is_some_and(|byte| byte & (1 << (b & 7)) != 0)
    }

    pub fn is_sky(&self, tri_id: u32) -> bool {
        tri_id & TRI_SKY != 0
    }

    /// Face index of a ray-tracing triangle, if it is a face.
    pub fn tri_face(&self, tri_id: u32) -> Option<usize> {
        (tri_id & (TRI_SKY | TRI_OPAQUE_NO_FACE) == 0).then_some(tri_id as usize)
    }
}

trait IntoGlam {
    fn into_glam(self) -> Vec3;
}

impl IntoGlam for crate::bspfile::Vec3 {
    fn into_glam(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

fn f32v(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Occluders: every face that blocks light (world and shadow-casting brush models), plus brush
/// sides that have no faces (nodraw) of opaque world brushes.
fn build_bvh(bsp: &BspFile, faces: &[Face], planes: &[DPlane], texinfo: &[TexInfo], models: &[DModel], extra: Vec<Tri>) -> Result<Bvh> {
    let entities = parse_entities(&bsp.entities());
    let mut casts: HashMap<usize, bool> = HashMap::new();
    for e in &entities {
        if let Some(m) = get(e, "model").and_then(|m| m.strip_prefix('*')).and_then(|m| m.parse::<usize>().ok()) {
            let class = get(e, "classname").unwrap_or("");
            let flag = get(e, "vrad_brush_cast_shadows").is_some_and(|v| v.trim() == "1");
            casts.insert(m, flag || class.eq_ignore_ascii_case("func_detail"));
        }
    }
    let mut tris = extra;
    for f in faces {
        if f.model != 0 && !casts.get(&f.model).copied().unwrap_or(false) {
            continue;
        }
        if f.flags & (surf::TRANS | surf::WARP | surf::NODRAW) != 0 && f.flags & surf::SKY == 0 {
            continue;
        }
        let id = if f.flags & surf::SKY != 0 { TRI_SKY | f.index as u32 } else { f.index as u32 };
        let p = &f.winding.points;
        for k in 1..p.len().saturating_sub(1) {
            tris.push(Tri::new(f32v(p[0]), f32v(p[k]), f32v(p[k + 1]), id));
        }
    }
    // Nodraw / block-light sides of opaque world brushes.
    let brushes: Vec<DBrush> = bsp.get(lump::BRUSHES)?;
    let sides: Vec<DBrushSide> = bsp.get(lump::BRUSHSIDES)?;
    let names = bsp.texdata_names()?;
    let texdata: Vec<DTexData> = bsp.get(lump::TEXDATA)?;
    let world_brushes = world_brush_set(bsp, models)?;
    for (bi, b) in brushes.iter().enumerate() {
        if !world_brushes.contains(&bi) {
            continue;
        }
        let bside = &sides[b.firstside as usize..(b.firstside + b.numsides) as usize];
        let blocklight = bside.iter().any(|s| {
            s.texinfo >= 0
                && texinfo.get(s.texinfo as usize).and_then(|t| texdata.get(t.texdata.max(0) as usize)).and_then(|d| names.get(d.name_string_table_id as usize)).is_some_and(|n| n.to_ascii_lowercase().contains("toolsblocklight"))
        });
        let opaque = b.contents & (contents::SOLID | contents::MOVEABLE) != 0 && b.contents & contents::TRANSLUCENT == 0;
        if !opaque && !blocklight {
            continue;
        }
        let pls: Vec<Plane> = bside.iter().map(|s| {
            let p = planes[s.planenum as usize];
            Plane::new(dv(p.normal.into_glam()), p.dist as f64)
        }).collect();
        for (i, s) in bside.iter().enumerate() {
            let flags = texinfo.get(s.texinfo.max(0) as usize).map_or(0, |t| t.flags);
            if !blocklight && flags & surf::NODRAW == 0 {
                continue;
            }
            let mut w = Some(Winding::base(&pls[i]));
            for (j, pj) in pls.iter().enumerate() {
                if i != j {
                    w = w.and_then(|w| w.chop(pj, 0.0));
                }
            }
            let Some(w) = w else { continue };
            let p = &w.points;
            for k in 1..p.len() - 1 {
                tris.push(Tri::new(f32v(p[0]), f32v(p[k]), f32v(p[k + 1]), TRI_OPAQUE_NO_FACE));
            }
        }
    }
    Ok(Bvh::build(tris))
}

/// Brushes referenced by the world model's leaves.
fn world_brush_set(bsp: &BspFile, models: &[DModel]) -> Result<std::collections::HashSet<usize>> {
    let nodes: Vec<DNode> = bsp.get(lump::NODES)?;
    let leafs: Vec<DLeaf> = bsp.get(lump::LEAFS)?;
    let lb: Vec<u16> = bsp.get(lump::LEAFBRUSHES)?;
    let mut set = std::collections::HashSet::new();
    let Some(m) = models.first() else { return Ok(set) };
    let mut stack = vec![m.headnode];
    while let Some(n) = stack.pop() {
        if n < 0 {
            let l = &leafs[(-1 - n) as usize];
            for k in 0..l.numleafbrushes as usize {
                set.insert(lb[l.firstleafbrush as usize + k] as usize);
            }
        } else {
            stack.extend(nodes[n as usize].children);
        }
    }
    Ok(set)
}
