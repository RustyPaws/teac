//! STAGE 1 — vbsp: VMF geometry -> BSP tree, faces, leaves, brushes, entities, portal file.

pub mod areaportal;
pub mod brush;
pub mod cubemap;
pub mod detail;
pub mod emit;
pub mod faces;
pub mod load;
pub mod overlay;
pub mod physcollide;
pub mod portals;
pub mod props;
pub mod tree;
pub mod water;

use crate::bspfile::{lump, BspFile};
use crate::ctx::Ctx;
use crate::flags::contents;
use crate::fs::GameFs;
use crate::material::Materials;
use crate::math::{Aabb, MAX_COORD};
use crate::vmf::Map;
use anyhow::{bail, Result};
use brush::{clip_to_box, BspBrush};
use emit::{CompiledModel, Emitter};
use faces::Faces;
use glam::DVec3;
use load::Level;
use portals::PortalFile;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tree::Tree;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BspOptions {
    /// Only replace the entity lump of an existing BSP.
    pub onlyents: bool,
    /// Drop func_detail brushes.
    pub nodetail: bool,
    pub nowater: bool,
    /// Don't weld vertices.
    pub noweld: bool,
    pub nomerge: bool,
    pub nosubdiv: bool,
    pub notjunc: bool,
    pub noprune: bool,
    /// Stop with an error when the map leaks.
    pub leaktest: bool,
    /// Brushes smaller than this volume are warned about.
    pub micro: f64,
    /// Largest lightmap dimension in luxels.
    pub max_lightmap_dim: f64,
}

impl Default for BspOptions {
    fn default() -> Self {
        BspOptions {
            onlyents: false,
            nodetail: false,
            nowater: false,
            noweld: false,
            nomerge: false,
            nosubdiv: false,
            notjunc: false,
            noprune: false,
            leaktest: false,
            micro: 1.0,
            max_lightmap_dim: 32.0,
        }
    }
}

pub struct VbspOutput {
    pub bsp: BspFile,
    /// Cluster portals for vvis (none when the map leaked).
    pub portals: Option<PortalFile>,
    /// Leak path for the `.lin` pointfile.
    pub leak: Option<Vec<DVec3>>,
}

fn structural(lv: &Level, b: usize) -> bool {
    lv.brushes[b].contents & contents::DETAIL == 0
}

/// Builds the world tree from 1024-unit blocks (vbsp `ProcessBlock` + `BlockTree`).
fn build_world_tree(lv: &Level, brushes: &[usize]) -> Tree {
    let (xl, yl, xh, yh) = tree::block_range(&lv.bounds);
    let mut coords = Vec::new();
    for x in xl..=xh {
        for y in yl..=yh {
            coords.push((x, y));
        }
    }
    let planes = &lv.planes;
    let built: Vec<((i32, i32), tree::BNode)> = coords
        .par_iter()
        .map(|&(x, y)| {
            let (min, max) = tree::block_bounds(x, y);
            let list: Vec<BspBrush> = brushes
                .iter()
                .filter(|&&b| {
                    let bb = &lv.brushes[b].bounds;
                    !(bb.min.x >= max.x || bb.max.x <= min.x || bb.min.y >= max.y || bb.max.y <= min.y)
                })
                .filter_map(|&b| clip_to_box(planes, BspBrush::from_map(lv, b), min, max))
                .collect();
            let node = if list.is_empty() {
                tree::BNode::Leaf { brushes: Vec::new(), contents: contents::SOLID }
            } else {
                tree::brush_bsp(planes, list, min, max)
            };
            ((x, y), node)
        })
        .collect();
    let mut blocks: HashMap<(i32, i32), tree::BNode> = built.into_iter().collect();
    let root = tree::block_tree(planes, &mut blocks, xl - 1, yl - 1, xh + 1, yh + 1);
    let bounds = Aabb {
        min: DVec3::new(xl as f64 * tree::BLOCK_SIZE, yl as f64 * tree::BLOCK_SIZE, lv.bounds.min.z - 8.0),
        max: DVec3::new((xh + 1) as f64 * tree::BLOCK_SIZE, (yh + 1) as f64 * tree::BLOCK_SIZE, lv.bounds.max.z + 8.0),
    };
    Tree::from_bnode(root, bounds)
}

/// Creates the planes the world and submodel volumes need, before parallel tree building.
fn prepare_planes(lv: &mut Level) {
    let (xl, yl, xh, yh) = tree::block_range(&lv.bounds);
    for x in (xl - 1)..=(xh + 2) {
        for y in (yl - 1)..=(yh + 2) {
            let (min, max) = tree::block_bounds(x, y);
            tree::ensure_box_planes(&mut lv.planes, min, max);
        }
    }
    let mut boxes = Vec::new();
    for e in &lv.entities[1..] {
        if let Some(b) = model_bounds(lv, &e.brushes) {
            boxes.push(b);
        }
    }
    for b in boxes {
        let (min, max) = model_volume(&b);
        tree::ensure_box_planes(&mut lv.planes, min, max);
    }
}

fn model_bounds(lv: &Level, brushes: &[usize]) -> Option<Aabb> {
    let mut b = Aabb::EMPTY;
    for &i in brushes {
        b = b.union(&lv.brushes[i].bounds);
    }
    (!b.is_empty()).then_some(b)
}

fn model_volume(b: &Aabb) -> (DVec3, DVec3) {
    ((b.min - DVec3::splat(8.0)).floor(), (b.max + DVec3::splat(8.0)).ceil())
}

/// `map_name` is the BSP name without extension (used for patched material paths).
pub fn run(opts: &BspOptions, ctx: &Ctx, map: &Map, fs: &GameFs, map_name: &str) -> Result<VbspOutput> {
    ctx.stage("vbsp", "geometry");
    let mats = Materials::new(fs);
    let mut ph = ctx.phase("Loading brushes");
    let mut lv = load::load(map, &mats, opts, ctx);
    ph.note(format!("{} brushes, {} entities", lv.brushes.len(), lv.entities.len()));
    drop(ph);
    for m in mats.missing() {
        ctx.warn(format!("material not found: {m}"));
    }
    if lv.entities[0].brushes.is_empty() {
        bail!("the world has no brushes");
    }
    for b in &lv.brushes {
        if b.bounds.min.min_element() < -MAX_COORD || b.bounds.max.max_element() > MAX_COORD {
            bail!("brush {} is outside the map limits (±{MAX_COORD})", b.id);
        }
    }
    prepare_planes(&mut lv);
    let cubemaps = cubemap::process(&mut lv, &mats, map_name, ctx);
    let mut pak_files = cubemaps.files.clone();
    if !opts.nowater {
        pak_files.extend(water::patch_depth(&mut lv, &mats, map_name));
    }

    // ---- World ----
    let world_brushes = lv.entities[0].brushes.clone();
    let structural_brushes: Vec<usize> = world_brushes.iter().copied().filter(|&b| structural(&lv, b)).collect();
    let detail_brushes: Vec<usize> = world_brushes.iter().copied().filter(|&b| !structural(&lv, b)).collect();

    let mut leak = None;
    let mut tree;
    let mut ps;
    let mut pass = 0;
    loop {
        let mut ph = ctx.phase(if pass == 0 { "BSP tree" } else { "BSP tree (optimized)" });
        tree = build_world_tree(&lv, &structural_brushes);
        ps = portals::make_tree_portals(&mut tree, &lv.planes, false);
        let leaves = tree.nodes.iter().filter(|n| n.is_leaf()).count();
        ph.note(format!("{} nodes, {leaves} leaves, {} portals", tree.nodes.len() - leaves, ps.list.len()));
        drop(ph);

        let ph = ctx.phase("Flood entities");
        let sealed = portals::flood_entities(&mut tree, &ps, &lv.planes, &lv);
        drop(ph);
        if sealed {
            let (filled, inside) = portals::fill_outside(&mut tree);
            ctx.detail(format!("filled {filled} outside leaves, {inside} inside"));
        } else if tree.nodes[tree.outside].occupied != 0 {
            let (path, ent) = portals::leak_path(&tree, &ps, &lv);
            let who = ent.map(|e| format!("{} (id {})", lv.entities[e].classname(), lv.entities[e].id)).unwrap_or_default();
            ctx.error(format!("**** LEAKED **** entity {who} can reach the void"));
            if opts.leaktest {
                bail!("map leaked");
            }
            leak = Some(path);
        } else {
            ctx.warn("no entities inside the map: nothing was filled");
        }
        portals::mark_visible_sides(&tree, &mut ps, &mut lv, &world_brushes);
        let vis = world_brushes.iter().map(|&b| lv.brushes[b].sides.iter().filter(|s| s.visible).count()).sum::<usize>();
        let empty = tree.nodes.iter().filter(|n| n.is_leaf() && n.contents & contents::SOLID == 0).count();
        ctx.detail(format!("pass {pass}: {vis} visible sides, {empty} non-solid leaves"));
        pass += 1;
        if pass == 2 || leak.is_some() {
            break;
        }
    }
    portals::calc_bounds(&mut tree, &ps);

    let ph = ctx.phase("Areas");
    let mut portal_areas = vec![portals::PortalAreas::default(); lv.entities.len()];
    let num_areas = portals::flood_areas(&mut tree, &ps, &lv, &mut portal_areas);
    drop(ph);

    let (water_data, water_leaves) = water::leaf_data(&lv, &tree);
    for (&n, &id) in &water_leaves {
        tree.nodes[n].water_id = Some(id);
    }

    let mut ph = ctx.phase("Faces");
    let mut faces = Faces::default();
    portals::find_all_portal_sides(&tree, &mut ps, &lv.planes, &lv);
    faces::make_faces(&mut tree, &mut ps, &lv.planes, &lv, &mut faces);
    faces::merge_and_subdivide(&mut tree, &mut faces, &lv.planes, &lv, !opts.nomerge, !opts.nosubdiv, opts.max_lightmap_dim);
    let det = detail::detail_faces(&lv, &lv.planes, &detail_brushes, &mut faces, !opts.nomerge, !opts.nosubdiv, opts.max_lightmap_dim);
    let detail_faces = detail::filter_faces(&mut tree, &lv.planes, &mut faces, det);
    detail::filter_brushes(&mut tree, &lv.planes, &lv, &world_brushes);
    ph.note(format!("{} faces", faces.list.iter().filter(|f| f.is_final()).count()));
    drop(ph);

    if !opts.noprune {
        let n = tree.prune();
        ctx.detail(format!("pruned {n} nodes"));
    }

    let portal_file = if leak.is_none() {
        let mut ph = ctx.phase("Vis clusters");
        let pf = portals::make_vis_portals(&mut tree, &lv.planes);
        ph.note(format!("{} clusters, {} portals", pf.num_clusters, pf.portals.len()));
        Some(pf)
    } else {
        None
    };

    let world = CompiledModel { entity: 0, tree, faces, detail_faces, bounds: lv.bounds };

    // ---- Brush entities ----
    let mut models = vec![world];
    let ents: Vec<usize> = (1..lv.entities.len()).filter(|&e| !lv.entities[e].brushes.is_empty()).collect();
    let mut ph = ctx.progress("Brush entities", ents.len() as u64);
    for e in ents {
        let brushes = lv.entities[e].brushes.clone();
        let Some(bounds) = model_bounds(&lv, &brushes) else { continue };
        let (min, max) = model_volume(&bounds);
        let list: Vec<BspBrush> = brushes.iter().map(|&b| BspBrush::from_map(&lv, b)).collect();
        let root = tree::brush_bsp(&lv.planes, list, min, max);
        let mut t = Tree::from_bnode(root, bounds);
        let mut ps = portals::make_tree_portals(&mut t, &lv.planes, false);
        portals::find_all_portal_sides(&t, &mut ps, &lv.planes, &lv);
        let mut faces = Faces::default();
        faces::make_faces(&mut t, &mut ps, &lv.planes, &lv, &mut faces);
        faces::merge_and_subdivide(&mut t, &mut faces, &lv.planes, &lv, !opts.nomerge, !opts.nosubdiv, opts.max_lightmap_dim);
        portals::calc_bounds(&mut t, &ps);
        detail::filter_brushes(&mut t, &lv.planes, &lv, &brushes);
        portals::free_portals(&mut t);
        models.push(CompiledModel { entity: e, tree: t, faces, detail_faces: Vec::new(), bounds });
        ph.inc(1);
    }
    ph.note(format!("{} models", models.len() - 1));
    drop(ph);

    // ---- Emit ----
    let ph = ctx.phase("Writing lumps");
    let mut em = Emitter::new();
    for (mi, m) in models.iter_mut().enumerate() {
        em.emit_model(&lv, &lv.planes, m, !opts.noweld, !opts.notjunc);
        lv.entities[m.entity].model = Some(mi);
    }
    let ov = overlay::build(&mut lv, &mats, &em.face_sides, ctx);
    let al = areaportal::build(&mut lv, &models[0].tree, &portal_areas, num_areas, ctx);
    let mut bsp = BspFile::new();
    emit::write_lumps(&mut bsp, &lv, &lv.planes, &em, &lv.bounds);
    bsp.set(lump::CUBEMAPS, &cubemaps.samples);
    bsp.set(lump::AREAS, &al.areas);
    bsp.set(lump::AREAPORTALS, &al.portals);
    bsp.set(lump::CLIPPORTALVERTS, &al.verts);
    bsp.set(lump::LEAFWATERDATA, &water_data);
    {
        let wb = water::water_boxes(&lv);
        let lb: Vec<(Aabb, bool)> = em
            .leafs
            .iter()
            .map(|l| {
                let b = Aabb { min: DVec3::new(l.mins[0] as f64, l.mins[1] as f64, l.mins[2] as f64), max: DVec3::new(l.maxs[0] as f64, l.maxs[1] as f64, l.maxs[2] as f64) };
                (b, l.contents & (contents::WATER | contents::SLIME) != 0)
            })
            .collect();
        bsp.set(lump::LEAFMINDISTTOWATER, &water::min_dist_to_water(&lb, &water_data, &wb));
    }
    bsp.set(lump::OVERLAYS, &ov.overlays);
    bsp.set(lump::OVERLAY_FADES, &ov.fades);
    bsp.set(lump::OVERLAY_SYSTEM_LEVELS, &ov.levels);
    ctx.detail(format!("{} overlays", ov.overlays.len()));
    drop(ph);

    let mut ph = ctx.phase("Static props");
    let props = props::collect(&lv, fs, &models[0].tree, &em.leaf_map[0], ctx);
    ph.note(format!("{} props", props.len()));
    if let Some(g) = bsp.game_lumps.iter_mut().find(|g| g.name() == "sprp") {
        g.data = props::lump(&props);
    }
    drop(ph);
    let mut ph = ctx.phase("Physics collision");
    let phys_models: Vec<(usize, usize)> = models.iter().enumerate().map(|(mi, m)| (mi, m.entity)).collect();
    let phys = physcollide::build(&lv, &phys_models, ctx);
    ph.note(format!("{} KiB", phys.len() / 1024));
    bsp.set_raw(lump::PHYSCOLLIDE, phys);
    drop(ph);
    let ph = ctx.phase("Finishing");
    pak_files.sort_by(|a, b| a.0.cmp(&b.0));
    pak_files.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    crate::bspfile::pakfile::write_all(&mut bsp, &pak_files)?;
    drop(ph);
    ctx.info(format!("{} faces, {} leaves, {} t-junctions fixed", em.faces.len(), em.leafs.len(), em.tjuncs));

    Ok(VbspOutput { bsp, portals: portal_file, leak })
}
