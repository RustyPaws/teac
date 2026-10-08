//! STAGE 2 — vvis: potentially visible sets from the cluster portals.
//!
//! Follows Source vvis: every portal-file entry becomes two one-way portals; `BasePortalVis`
//! computes what each portal might see (a flood restricted to portals in front of it), then
//! `PortalFlow` refines that with separating-plane clipping. Cluster PVS = OR of its portals'
//! visibility; PAS = OR of the PVS of every visible cluster. Both are RLE-compressed into the
//! VISIBILITY lump.

pub mod flow;

use crate::bspfile::{lump, BspFile};
use crate::ctx::Ctx;
use crate::math::{Plane, Winding};
use crate::vbsp::portals::PortalFile;
use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisOptions {
    /// Skip the portal flow (base vis only): fast, but sees more.
    pub fast: bool,
    /// Limit visibility to this distance (0 = unlimited).
    pub radius_override: f64,
}

impl Default for VisOptions {
    fn default() -> Self {
        VisOptions { fast: false, radius_override: 0.0 }
    }
}

pub const ON_EPSILON: f64 = 0.1;

/// Fixed-size bitset over portals or clusters.
#[derive(Clone, Debug, PartialEq)]
pub struct Bits(pub Vec<u64>);

impl Bits {
    pub fn new(n: usize) -> Bits {
        Bits(vec![0; n.div_ceil(64)])
    }
    pub fn set(&mut self, i: usize) {
        self.0[i >> 6] |= 1 << (i & 63);
    }
    pub fn get(&self, i: usize) -> bool {
        self.0[i >> 6] & (1 << (i & 63)) != 0
    }
    pub fn count(&self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }
    pub fn or(&mut self, o: &Bits) {
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            *a |= b;
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(wi, &w)| {
            let mut w = w;
            std::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let b = w.trailing_zeros() as usize;
                w &= w - 1;
                Some(wi * 64 + b)
            })
        })
    }
}

/// A one-way portal: lives in `cluster`, leads to `leaf`; its plane faces into `leaf`.
#[derive(Clone, Debug)]
pub struct VPortal {
    pub winding: Winding,
    pub plane: Plane,
    pub leaf: usize,
    pub origin: glam::DVec3,
    pub radius: f64,
}

pub struct VisData {
    pub portals: Vec<VPortal>,
    /// Portals in each cluster.
    pub leaves: Vec<Vec<usize>>,
    pub num_clusters: usize,
}

pub fn build_portals(pf: &PortalFile) -> Result<VisData> {
    let mut portals = Vec::with_capacity(pf.portals.len() * 2);
    let mut leaves = vec![Vec::new(); pf.num_clusters];
    for p in &pf.portals {
        let [a, b] = p.clusters;
        if a < 0 || b < 0 || a as usize >= pf.num_clusters || b as usize >= pf.num_clusters {
            bail!("portal between invalid clusters {a} and {b}");
        }
        if p.winding.points.len() < 3 {
            continue;
        }
        let plane = p.winding.plane();
        let w = &p.winding;
        let origin = w.center();
        let radius = w.points.iter().map(|q| (*q - origin).length()).fold(0.0, f64::max);
        // Forward: in a, into b (plane faces b).
        leaves[a as usize].push(portals.len());
        portals.push(VPortal { winding: w.clone(), plane: plane.flipped(), leaf: b as usize, origin, radius });
        // Backward: in b, into a.
        leaves[b as usize].push(portals.len());
        portals.push(VPortal { winding: w.reversed(), plane, leaf: a as usize, origin, radius });
    }
    Ok(VisData { portals, leaves, num_clusters: pf.num_clusters })
}

/// `BasePortalVis`: portals each portal might see (flooding only through portals in front).
fn base_vis(vd: &VisData, radius: f64) -> Vec<Bits> {
    let n = vd.portals.len();
    (0..n)
        .into_par_iter()
        .map(|pi| {
            let p = &vd.portals[pi];
            let mut front = Bits::new(n);
            for (j, tp) in vd.portals.iter().enumerate() {
                if j == pi {
                    continue;
                }
                if !tp.winding.points.iter().any(|&q| p.plane.distance(q) > ON_EPSILON) {
                    continue;
                }
                if !p.winding.points.iter().any(|&q| tp.plane.distance(q) < -ON_EPSILON) {
                    continue;
                }
                if radius > 0.0 && (tp.origin - p.origin).length() - tp.radius - p.radius > radius {
                    continue;
                }
                front.set(j);
            }
            // SimpleFlood from the leaf the portal leads into.
            let mut flood = Bits::new(n);
            let mut stack = vec![p.leaf];
            let mut seen_leaf = vec![false; vd.num_clusters];
            seen_leaf[p.leaf] = true;
            while let Some(l) = stack.pop() {
                for &q in &vd.leaves[l] {
                    if !front.get(q) || flood.get(q) {
                        continue;
                    }
                    flood.set(q);
                    let dest = vd.portals[q].leaf;
                    if !seen_leaf[dest] {
                        seen_leaf[dest] = true;
                        stack.push(dest);
                    }
                }
            }
            flood
        })
        .collect()
}

/// Source `CompressVis`: zero bytes are run-length encoded as `0, count`.
pub fn compress(vis: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vis.len());
    let mut j = 0;
    while j < vis.len() {
        out.push(vis[j]);
        if vis[j] != 0 {
            j += 1;
            continue;
        }
        let mut rep = 1u8;
        j += 1;
        while j < vis.len() && vis[j] == 0 && rep < 255 {
            rep += 1;
            j += 1;
        }
        out.push(rep);
    }
    out
}

pub fn decompress(data: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(row);
    let mut i = 0;
    while out.len() < row && i < data.len() {
        if data[i] != 0 {
            out.push(data[i]);
            i += 1;
        } else {
            let n = data.get(i + 1).copied().unwrap_or(1) as usize;
            out.extend(std::iter::repeat_n(0, n));
            i += 2;
        }
    }
    out.resize(row, 0);
    out
}

fn bits_to_bytes(b: &Bits, n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n.div_ceil(8)];
    for i in b.iter().filter(|&i| i < n) {
        v[i >> 3] |= 1 << (i & 7);
    }
    v
}

pub struct VisResult {
    pub pvs: Vec<Bits>,
    pub average_visible: f64,
}

/// Computes cluster PVS sets.
pub fn compute(vd: &VisData, opts: &VisOptions, ctx: &Ctx) -> VisResult {
    let n = vd.portals.len();
    let mut ph = ctx.phase("Base vis");
    let might = base_vis(vd, opts.radius_override);
    let avg_might = if n > 0 { might.iter().map(Bits::count).sum::<usize>() as f64 / n as f64 } else { 0.0 };
    ph.note(format!("{} portals, {avg_might:.1} might see", n));
    drop(ph);

    let portal_vis: Vec<Bits> = if opts.fast {
        might.clone()
    } else {
        flow::portal_flow(vd, &might, ctx)
    };

    let ph = ctx.phase("Cluster merge");
    let nc = vd.num_clusters;
    let pvs: Vec<Bits> = (0..nc)
        .into_par_iter()
        .map(|c| {
            let mut pv = Bits::new(n);
            for &p in &vd.leaves[c] {
                pv.or(&portal_vis[p]);
                pv.set(p);
            }
            let mut cv = Bits::new(nc);
            for p in pv.iter() {
                cv.set(vd.portals[p].leaf);
            }
            cv.set(c);
            cv
        })
        .collect();
    drop(ph);
    let total: usize = pvs.iter().map(Bits::count).sum();
    VisResult { average_visible: if nc > 0 { total as f64 / nc as f64 } else { 0.0 }, pvs }
}

/// Builds the VISIBILITY lump (PVS + PAS).
pub fn vis_lump(pvs: &[Bits]) -> Vec<u8> {
    let nc = pvs.len();
    let pas: Vec<Bits> = (0..nc)
        .into_par_iter()
        .map(|c| {
            let mut a = pvs[c].clone();
            for o in pvs[c].iter() {
                a.or(&pvs[o]);
            }
            a
        })
        .collect();
    let header = 4 + nc * 8;
    let mut data = Vec::new();
    let mut offs = Vec::with_capacity(nc * 2);
    for c in 0..nc {
        offs.push((header + data.len()) as i32);
        data.extend(compress(&bits_to_bytes(&pvs[c], nc)));
        offs.push((header + data.len()) as i32);
        data.extend(compress(&bits_to_bytes(&pas[c], nc)));
    }
    let mut out = Vec::with_capacity(header + data.len());
    out.extend_from_slice(&(nc as i32).to_le_bytes());
    for o in offs {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend(data);
    out
}

/// Runs vvis on a BSP with its portal file.
pub fn run(opts: &VisOptions, ctx: &Ctx, bsp: &mut BspFile, pf: &PortalFile) -> Result<()> {
    ctx.stage("vvis", if opts.fast { "visibility (fast)" } else { "visibility" });
    let leafs: Vec<crate::bspfile::DLeaf> = bsp.get(lump::LEAFS)?;
    let max_cluster = leafs.iter().map(|l| l.cluster as i32).max().unwrap_or(-1);
    if max_cluster + 1 != pf.num_clusters as i32 {
        bail!("portal file has {} clusters but the BSP has {}", pf.num_clusters, max_cluster + 1);
    }
    let vd = build_portals(pf)?;
    let res = compute(&vd, opts, ctx);
    bsp.set_raw(lump::VISIBILITY, vis_lump(&res.pvs));
    ctx.info(format!("{} clusters, {:.1} visible on average", pf.num_clusters, res.average_visible));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_roundtrip() {
        let v = vec![1, 0, 0, 0, 5, 0, 7, 0, 0];
        let c = compress(&v);
        assert_eq!(c, vec![1, 0, 3, 5, 0, 1, 7, 0, 2]);
        assert_eq!(decompress(&c, v.len()), v);
        let z = vec![0u8; 600];
        assert_eq!(decompress(&compress(&z), 600), z);
    }
}
