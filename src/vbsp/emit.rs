//! Writes the compiled models into BSP lumps (vbsp `WriteBSP` / `EmitBrushes` / entities).

use super::faces::{emit_face_vertices, fix_face_tjuncs, Edges, Faces, Vertices};
use super::load::{removed_class, Level};
use super::tree::Tree;
use crate::bspfile::*;
use crate::flags::{contents, surf};
use crate::math::{Aabb, PlaneSet};
use glam::DVec3;
use std::collections::HashMap;

/// One compiled model (the world or a brush entity).
pub struct CompiledModel {
    pub entity: usize,
    pub tree: Tree,
    pub faces: Faces,
    /// Detail faces (leaf faces) of this model.
    pub detail_faces: Vec<usize>,
    pub bounds: Aabb,
}

#[derive(Default)]
pub struct Emitter {
    pub verts: Vertices,
    pub edges: Edges,
    pub surfedges: Vec<i32>,
    pub faces: Vec<DFace>,
    pub orig_faces: Vec<DFace>,
    orig_index: HashMap<(usize, usize), i32>,
    pub face_ids: Vec<u16>,
    /// Side ids each emitted face was made from (overlays reference sides by id).
    pub face_sides: Vec<Vec<i32>>,
    pub face_brush_list: Vec<DFaceBrushList>,
    pub face_brushes: Vec<u16>,
    pub nodes: Vec<DNode>,
    pub leafs: Vec<DLeaf>,
    pub leaf_faces: Vec<u16>,
    pub leaf_brushes: Vec<u16>,
    pub models: Vec<DModel>,
    pub tjuncs: usize,
    /// Output leaf index of each tree leaf, per model (world first).
    pub leaf_map: Vec<HashMap<usize, usize>>,
}

fn i16v(v: DVec3) -> [i16; 3] {
    [v.x.clamp(-32768.0, 32767.0) as i16, v.y.clamp(-32768.0, 32767.0) as i16, v.z.clamp(-32768.0, 32767.0) as i16]
}

fn bounds_i16(b: &Aabb) -> ([i16; 3], [i16; 3]) {
    if b.is_empty() {
        return ([0; 3], [0; 3]);
    }
    (i16v(b.min.floor()), i16v(b.max.ceil()))
}

impl Emitter {
    pub fn new() -> Emitter {
        Emitter {
            edges: Edges::new(),
            // Leaf 0 is the solid dummy leaf that "outside" references point at.
            leafs: vec![DLeaf { contents: contents::SOLID, ..Default::default() }],
            ..Default::default()
        }
    }

    fn lightmap_extents(&self, lv: &Level, texinfo: i32, pts: &[DVec3]) -> ([i32; 2], [i32; 2]) {
        let t = &lv.texinfo.list[texinfo as usize];
        if t.flags & surf::NOLIGHT != 0 {
            return ([0; 2], [0; 2]);
        }
        let mut mins = [f64::MAX; 2];
        let mut maxs = [f64::MIN; 2];
        for p in pts {
            for i in 0..2 {
                let v = t.lightmap_vecs[i];
                let val = p.x * v[0] as f64 + p.y * v[1] as f64 + p.z * v[2] as f64 + v[3] as f64;
                mins[i] = mins[i].min(val);
                maxs[i] = maxs[i].max(val);
            }
        }
        let mut m = [0; 2];
        let mut s = [0; 2];
        for i in 0..2 {
            let lo = mins[i].floor();
            let hi = maxs[i].ceil();
            m[i] = lo as i32;
            s[i] = (hi - lo) as i32;
        }
        (m, s)
    }

    fn orig_face(&mut self, lv: &Level, planes: &PlaneSet, b: usize, s: usize) -> i32 {
        if let Some(&i) = self.orig_index.get(&(b, s)) {
            return i;
        }
        let side = &lv.brushes[b].sides[s];
        let Some(w) = &side.winding else { return -1 };
        let verts: Vec<u32> = w.points.iter().map(|&p| self.verts.get(p, true)).collect();
        let first = self.surfedges.len() as i32;
        for i in 0..verts.len() {
            let e = self.edges.get(verts[i], verts[(i + 1) % verts.len()], -1);
            self.surfedges.push(e);
        }
        let (mins, size) = self.lightmap_extents(lv, side.texinfo, &w.points);
        let _ = planes;
        let df = DFace {
            planenum: (side.plane & !1) as u16,
            side: (side.plane & 1) as u8,
            on_node: 0,
            firstedge: first,
            numedges: verts.len() as i16,
            texinfo: side.texinfo as i16,
            dispinfo: -1,
            surface_fog_volume_id: -1,
            styles: [0; 4],
            lightofs: 0,
            area: w.area() as f32,
            lightmap_texture_mins_in_luxels: mins,
            lightmap_texture_size_in_luxels: size,
            orig_face: 0,
            num_prims: 0,
            first_prim_id: 0,
            smoothing_groups: side.smoothing,
        };
        let i = self.orig_faces.len() as i32;
        self.orig_faces.push(df);
        self.orig_index.insert((b, s), i);
        i
    }

    fn emit_face(&mut self, lv: &Level, planes: &PlaneSet, m: &mut CompiledModel, f: usize, on_node: bool) {
        let face = &m.faces.list[f];
        if !face.is_final() || face.verts.len() < 3 || face.texinfo < 0 {
            return;
        }
        let tflags = lv.texinfo.list[face.texinfo as usize].flags;
        if tflags & surf::NODRAW != 0 && face.dispinfo < 0 {
            return;
        }
        let verts = face.verts.clone();
        let (plane, texinfo, contents_, side, smoothing, dispinfo) = (face.plane, face.texinfo, face.contents, face.side, face.smoothing, face.dispinfo);
        let all_sides: Vec<i32> = face.side.iter().chain(&face.merged_sides).map(|&(b, s)| lv.brushes[b].sides[s].id).collect();
        let pts: Vec<DVec3> = verts.iter().map(|&v| self.verts.list[v as usize]).collect();
        let first = self.surfedges.len() as i32;
        for i in 0..verts.len() {
            let e = self.edges.get(verts[i], verts[(i + 1) % verts.len()], contents_);
            self.surfedges.push(e);
        }
        let (mins, size) = self.lightmap_extents(lv, texinfo, &pts);
        let orig = side.map(|(b, s)| self.orig_face(lv, planes, b, s)).unwrap_or(-1).max(0);
        let area = crate::math::Winding::new(pts).area();
        let out = self.faces.len();
        self.faces.push(DFace {
            planenum: (plane & !1) as u16,
            side: (plane & 1) as u8,
            on_node: on_node as u8,
            firstedge: first,
            numedges: verts.len() as i16,
            texinfo: texinfo as i16,
            dispinfo: dispinfo as i16,
            surface_fog_volume_id: -1,
            styles: [0; 4],
            lightofs: 0,
            area: area as f32,
            lightmap_texture_mins_in_luxels: mins,
            lightmap_texture_size_in_luxels: size,
            orig_face: orig,
            num_prims: 0,
            first_prim_id: 0,
            smoothing_groups: smoothing,
        });
        let side_id = side.map(|(b, s)| lv.brushes[b].sides[s].id).unwrap_or(0);
        self.face_ids.push(side_id.clamp(0, 65535) as u16);
        self.face_sides.push(all_sides);
        match side {
            Some((b, _)) => self.face_brush_list.push(DFaceBrushList { count: 1, start: b as u16 }),
            None => self.face_brush_list.push(DFaceBrushList { count: 0, start: 0 }),
        }
        m.faces.list[f].output = out as i32;
    }

    /// Emits faces (node faces in node order, then detail faces), nodes and leaves of a model.
    pub fn emit_model(&mut self, lv: &Level, planes: &PlaneSet, m: &mut CompiledModel, weld: bool, tjunc: bool) {
        self.edges.start_model();
        // Vertices for every final face, then t-junction repair against all of them.
        let mut all = Vec::new();
        for n in 0..m.tree.nodes.len() {
            for &f in &m.tree.nodes[n].faces {
                m.faces.finals(f, &mut all);
            }
        }
        for &f in &m.detail_faces {
            m.faces.finals(f, &mut all);
        }
        all.sort_unstable();
        all.dedup();
        for &f in &all {
            emit_face_vertices(&mut m.faces, f, &mut self.verts, weld);
        }
        if tjunc {
            for &f in &all {
                self.tjuncs += fix_face_tjuncs(&mut m.faces, f, &self.verts);
            }
        }

        let first_face = self.faces.len();
        // Node faces in preorder.
        let mut order = Vec::new();
        let mut stack = vec![m.tree.head];
        while let Some(n) = stack.pop() {
            if m.tree.nodes[n].is_leaf() {
                continue;
            }
            order.push(n);
            stack.push(m.tree.nodes[n].children[1]);
            stack.push(m.tree.nodes[n].children[0]);
        }
        let mut node_faces: HashMap<usize, (usize, usize)> = HashMap::new();
        for &n in &order {
            let start = self.faces.len();
            let mut fl = Vec::new();
            for &f in &m.tree.nodes[n].faces.clone() {
                m.faces.finals(f, &mut fl);
            }
            fl.dedup();
            for f in fl {
                self.emit_face(lv, planes, m, f, true);
            }
            node_faces.insert(n, (start, self.faces.len() - start));
        }
        let mut det = Vec::new();
        for &f in &m.detail_faces.clone() {
            m.faces.finals(f, &mut det);
        }
        det.sort_unstable();
        det.dedup();
        for f in det {
            self.emit_face(lv, planes, m, f, false);
        }

        // Nodes and leaves.
        let mut leaf_map = HashMap::new();
        let head = self.emit_node(lv, m, m.tree.head, &node_faces, &mut leaf_map);
        let model_bounds = if m.bounds.is_empty() { Aabb { min: DVec3::ZERO, max: DVec3::ZERO } } else { m.bounds };
        self.models.push(DModel {
            mins: (model_bounds.min - DVec3::ONE).into(),
            maxs: (model_bounds.max + DVec3::ONE).into(),
            origin: lv.entities[m.entity].origin.into(),
            headnode: head,
            firstface: first_face as i32,
            numfaces: (self.faces.len() - first_face) as i32,
        });
        self.leaf_map.push(leaf_map);
    }

    fn emit_node(&mut self, lv: &Level, m: &CompiledModel, n: usize, nf: &HashMap<usize, (usize, usize)>, leaf_map: &mut HashMap<usize, usize>) -> i32 {
        let node = &m.tree.nodes[n];
        if node.is_leaf() {
            let li = self.emit_leaf(lv, m, n);
            leaf_map.insert(n, li);
            return -1 - li as i32;
        }
        let i = self.nodes.len();
        let (mins, maxs) = bounds_i16(&node.bounds);
        let (ff, nfaces) = nf.get(&n).copied().unwrap_or((self.faces.len(), 0));
        self.nodes.push(DNode {
            planenum: node.plane.unwrap() as i32,
            children: [0, 0],
            mins,
            maxs,
            firstface: ff as u16,
            numfaces: nfaces as u16,
            area: node.area as i16,
            padding: 0,
        });
        let [c0, c1] = node.children;
        let a = self.emit_node(lv, m, c0, nf, leaf_map);
        let b = self.emit_node(lv, m, c1, nf, leaf_map);
        self.nodes[i].children = [a, b];
        i as i32
    }

    fn emit_leaf(&mut self, _lv: &Level, m: &CompiledModel, n: usize) -> usize {
        let node = &m.tree.nodes[n];
        let li = self.leafs.len();
        let (mins, maxs) = bounds_i16(&node.bounds);
        let mut leaf = DLeaf {
            contents: node.contents,
            cluster: node.cluster as i16,
            mins,
            maxs,
            firstleafface: self.leaf_faces.len() as u16,
            firstleafbrush: self.leaf_brushes.len() as u16,
            leaf_water_data_id: node.water_id.unwrap_or(-1),
            ..Default::default()
        };
        leaf.set_area_flags(node.area, 0);
        for &b in &node.leaf_brushes {
            self.leaf_brushes.push(b as u16);
        }
        if node.contents & contents::SOLID == 0 {
            let mut list = Vec::new();
            for &f in &node.mark_faces {
                m.faces.finals(f, &mut list);
            }
            let start = self.leaf_faces.len();
            for f in list {
                let o = m.faces.list[f].output;
                if o >= 0 && !self.leaf_faces[start..].contains(&(o as u16)) {
                    self.leaf_faces.push(o as u16);
                    // Faces seen from inside water are drawn with its fog.
                    if let Some(w) = node.water_id {
                        self.faces[o as usize].surface_fog_volume_id = w;
                    }
                }
            }
        }
        leaf.numleaffaces = (self.leaf_faces.len() - leaf.firstleafface as usize) as u16;
        leaf.numleafbrushes = (self.leaf_brushes.len() - leaf.firstleafbrush as usize) as u16;
        self.leafs.push(leaf);
        li
    }
}

/// Entity lump text: keys in reverse order (as vbsp writes them), `hammerid`, then outputs.
pub fn entity_text(lv: &Level, world_bounds: &Aabb) -> String {
    let mut s = String::new();
    for (i, e) in lv.entities.iter().enumerate() {
        if i > 0 && removed_class(e.classname()) {
            continue;
        }
        s.push_str("{\n");
        if i == 0 && !world_bounds.is_empty() {
            let f = |v: DVec3| format!("{} {} {}", fmt_num(v.x), fmt_num(v.y), fmt_num(v.z));
            s.push_str(&format!("\"world_maxs\" \"{}\"\n", f(world_bounds.max)));
            s.push_str(&format!("\"world_mins\" \"{}\"\n", f(world_bounds.min)));
        }
        for (k, v) in e.keys.iter().rev() {
            if k.eq_ignore_ascii_case("id") {
                continue;
            }
            s.push_str(&format!("\"{k}\" \"{v}\"\n"));
        }
        if let Some(mi) = e.model {
            if mi > 0 {
                s.push_str(&format!("\"model\" \"*{mi}\"\n"));
            }
        }
        s.push_str(&format!("\"hammerid\" \"{}\"\n", e.id));
        for (k, v) in &e.connections {
            s.push_str(&format!("\"{k}\" \"{}\"\n", v.replace(',', "\x1b")));
        }
        s.push_str("}\n");
    }
    s
}

fn fmt_num(v: f64) -> String {
    let r = v.round();
    if (v - r).abs() < 1e-3 { format!("{}", r as i64) } else { format!("{v}") }
}

/// Fills the BSP lumps from the emitter and level.
pub fn write_lumps(bsp: &mut BspFile, lv: &Level, planes: &PlaneSet, em: &Emitter, world_bounds: &Aabb) {
    let dplanes: Vec<DPlane> = planes
        .planes
        .iter()
        .map(|p| DPlane { normal: p.normal.into(), dist: p.dist as f32, kind: p.kind() })
        .collect();
    bsp.set(lump::PLANES, &dplanes);

    // Texdata + string table (names keep their VMF spelling).
    let mut string_data = Vec::new();
    let mut table = Vec::new();
    for n in &lv.texdata.names {
        table.push(string_data.len() as i32);
        string_data.extend_from_slice(n.as_bytes());
        string_data.push(0);
    }
    bsp.set(lump::TEXDATA, &lv.texdata.data);
    bsp.set_raw(lump::TEXDATA_STRING_DATA, string_data);
    bsp.set(lump::TEXDATA_STRING_TABLE, &table);
    bsp.set(lump::TEXINFO, &lv.texinfo.list);

    let verts: Vec<Vec3> = em.verts.list.iter().map(|&v| v.into()).collect();
    bsp.set(lump::VERTEXES, &verts);
    let edges: Vec<DEdge> = em.edges.list.iter().map(|&v| DEdge { v }).collect();
    bsp.set(lump::EDGES, &edges);
    bsp.set(lump::SURFEDGES, &em.surfedges);
    bsp.set(lump::FACES, &em.faces);
    bsp.set(lump::ORIGINALFACES, &em.orig_faces);
    bsp.set(lump::FACEIDS, &em.face_ids);
    bsp.set(lump::FACE_MACRO_TEXTURE_INFO, &vec![0xffffu16; em.faces.len()]);
    bsp.set(lump::FACEBRUSHLIST, &em.face_brush_list);
    bsp.set(lump::FACEBRUSHES, &em.face_brushes);
    bsp.set(lump::NODES, &em.nodes);
    bsp.set(lump::LEAFS, &em.leafs);
    bsp.set(lump::LEAFFACES, &em.leaf_faces);
    bsp.set(lump::LEAFBRUSHES, &em.leaf_brushes);
    bsp.set(lump::LEAFMINDISTTOWATER, &vec![0xffffu16; em.leafs.len()]);
    bsp.set(lump::MODELS, &em.models);

    // Brushes and sides: map order, axial sides first (vbsp order), no bevels.
    let mut dbrushes = Vec::with_capacity(lv.brushes.len());
    let mut dsides = Vec::new();
    for b in &lv.brushes {
        let first = dsides.len();
        for s in b.sides.iter().filter(|s| !s.bevel) {
            dsides.push(DBrushSide { planenum: s.plane as u16, texinfo: s.texinfo as i16, dispinfo: 0, bevel: 0, thin: 0 });
        }
        dbrushes.push(DBrush { firstside: first as i32, numsides: (dsides.len() - first) as i32, contents: b.contents });
    }
    bsp.set(lump::BRUSHES, &dbrushes);
    bsp.set(lump::BRUSHSIDES, &dsides);

    bsp.set_entities(&entity_text(lv, world_bounds));
    bsp.set_raw(lump::MAP_FLAGS, vec![0; 4]);
    bsp.set_raw(lump::OCCLUSION, vec![0; 12]);
    bsp.set_raw(lump::PHYSDISP, vec![0; 2]);
    bsp.game_lumps = vec![
        GameLump { id: GameLump::id_from("sprp"), flags: 0, version: STATIC_PROP_VERSION, data: vec![0; 12] },
        GameLump { id: GameLump::id_from("dprp"), flags: 0, version: DETAIL_PROP_VERSION, data: vec![0; 12] },
    ];
}
