//! Full portal flow (Source vvis `PortalFlow` / `RecursiveLeafFlow` / `ClipToSeperators`).
//! Portals are processed in parallel, cheapest (fewest might-see) first; finished portals'
//! results prune later flows. Windings are small inline vectors: the flow allocates nothing
//! per step except the might-see bitset of a portal it actually passes through.

use super::{Bits, VisData, ON_EPSILON};
use crate::ctx::Ctx;
use crate::math::Plane;
use glam::DVec3;
use smallvec::SmallVec;
use std::sync::OnceLock;

type W = SmallVec<[DVec3; 16]>;

/// Keeps the part of `w` in front of `p` (Source vvis `ChopWinding`): a winding entirely in
/// front or on the plane is kept as is, one entirely behind is dropped.
fn chop(w: &[DVec3], p: &Plane) -> Option<W> {
    let n = w.len();
    let mut dists: SmallVec<[f64; 16]> = SmallVec::with_capacity(n);
    let mut sides: SmallVec<[i8; 16]> = SmallVec::with_capacity(n);
    let (mut front, mut back) = (0, 0);
    for &q in w {
        let d = p.distance(q);
        let s = if d > ON_EPSILON {
            front += 1;
            1
        } else if d < -ON_EPSILON {
            back += 1;
            -1
        } else {
            0
        };
        dists.push(d);
        sides.push(s);
    }
    if back == 0 {
        return Some(W::from_slice(w));
    }
    if front == 0 {
        return None;
    }
    let mut out = W::new();
    for i in 0..n {
        let p1 = w[i];
        if sides[i] == 0 {
            out.push(p1);
            continue;
        }
        if sides[i] == 1 {
            out.push(p1);
        }
        let j = (i + 1) % n;
        if sides[j] == 0 || sides[j] == sides[i] {
            continue;
        }
        let t = dists[i] / (dists[i] - dists[j]);
        out.push(p1 + (w[j] - p1) * t);
    }
    (out.len() >= 3).then_some(out)
}

fn clip_to_separators(source: &[DVec3], pass: &[DVec3], target: W, flipclip: bool) -> Option<W> {
    let mut target = target;
    let ns = source.len();
    for i in 0..ns {
        let l = (i + 1) % ns;
        let v1 = source[l] - source[i];
        for j in 0..pass.len() {
            let v2 = pass[j] - source[i];
            let mut normal = v1.cross(v2);
            let len2 = normal.length_squared();
            if len2 < ON_EPSILON {
                continue;
            }
            normal /= len2.sqrt();
            let mut plane = Plane::new(normal, pass[j].dot(normal));
            let mut fliptest = None;
            for (k, &q) in source.iter().enumerate() {
                if k == i || k == l {
                    continue;
                }
                let d = plane.distance(q);
                if d < -ON_EPSILON {
                    fliptest = Some(false);
                    break;
                } else if d > ON_EPSILON {
                    fliptest = Some(true);
                    break;
                }
            }
            let Some(flip) = fliptest else { continue };
            if flip {
                plane = plane.flipped();
            }
            let mut front = 0;
            let mut ok = true;
            for (k, &q) in pass.iter().enumerate() {
                if k == j {
                    continue;
                }
                let d = plane.distance(q);
                if d < -ON_EPSILON {
                    ok = false;
                    break;
                } else if d > ON_EPSILON {
                    front += 1;
                }
            }
            if !ok || front == 0 {
                continue;
            }
            if flipclip {
                plane = plane.flipped();
            }
            target = chop(&target, &plane)?;
            break;
        }
    }
    Some(target)
}

struct Stack<'a> {
    source: W,
    pass: Option<W>,
    portalplane: Plane,
    mightsee: std::borrow::Cow<'a, Bits>,
}

struct Thread<'a> {
    vd: &'a VisData,
    base_plane: Plane,
    vis: Bits,
    might: &'a [Bits],
    done: &'a [OnceLock<Bits>],
}

fn recursive_leaf_flow(t: &mut Thread, leaf: usize, prev: &Stack, depth: usize) {
    if depth > 4096 {
        return;
    }
    let n_long = prev.mightsee.0.len();
    for &pnum in &t.vd.leaves[leaf] {
        if !prev.mightsee.get(pnum) {
            continue;
        }
        let p = &t.vd.portals[pnum];
        let test = t.done[pnum].get().unwrap_or(&t.might[pnum]);
        let mut more = 0u64;
        for j in 0..n_long {
            more |= prev.mightsee.0[j] & test.0[j] & !t.vis.0[j];
        }
        if more == 0 && t.vis.get(pnum) {
            continue;
        }
        let Some(pass) = chop(&p.winding.points, &t.base_plane) else { continue };
        let Some(source) = chop(&prev.source, &p.plane.flipped()) else { continue };
        let pass = match &prev.pass {
            None => pass,
            Some(prev_pass) => {
                let Some(pass) = chop(&pass, &prev.portalplane) else { continue };
                let Some(pass) = clip_to_separators(&source, prev_pass, pass, false) else { continue };
                let Some(pass) = clip_to_separators(prev_pass, &source, pass, true) else { continue };
                pass
            }
        };
        t.vis.set(pnum);
        let mut might = Bits(vec![0; n_long]);
        for j in 0..n_long {
            might.0[j] = prev.mightsee.0[j] & test.0[j];
        }
        let st = Stack { source, pass: Some(pass), portalplane: p.plane, mightsee: std::borrow::Cow::Owned(might) };
        recursive_leaf_flow(t, p.leaf, &st, depth + 1);
    }
}

pub fn portal_flow(vd: &VisData, might: &[Bits], ctx: &Ctx) -> Vec<Bits> {
    let n = vd.portals.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&p| might[p].count());
    let done: Vec<OnceLock<Bits>> = (0..n).map(|_| OnceLock::new()).collect();
    let mut ph = ctx.progress("Portal flow", n as u64);
    // Cheap portals first: workers pull from the sorted queue, so later (expensive) flows can
    // reuse the results of finished ones.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = rayon::current_num_threads();
    rayon::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|_| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&pi) = order.get(k) else { break };
                let p = &vd.portals[pi];
                let mut t = Thread { vd, base_plane: p.plane, vis: Bits::new(n), might, done: &done };
                let head = Stack {
                    source: W::from_slice(&p.winding.points),
                    pass: None,
                    portalplane: p.plane,
                    mightsee: std::borrow::Cow::Borrowed(&might[pi]),
                };
                recursive_leaf_flow(&mut t, p.leaf, &head, 0);
                let _ = done[pi].set(t.vis);
                ph.inc(1);
            });
        }
    });
    let total: usize = done.iter().map(|d| d.get().map_or(0, Bits::count)).sum();
    ph.note(format!("{:.1} portals visible on average", total as f64 / n.max(1) as f64));
    drop(ph);
    done.into_iter().map(|d| d.into_inner().unwrap_or_else(|| Bits::new(n))).collect()
}
