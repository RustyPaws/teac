//! `func_instance` collapsing, following vbsp:
//!
//! * The instance VMF is found next to the referencing VMF, then under the outermost `maps`
//!   directory of its path, then in the configured instance directories.
//! * Nested instances are collapsed first (depth-first), so names get nested fixups.
//! * Brushes are transformed with texture lock; point/brush entities get their `origin`,
//!   `angles` and FGD-typed keys (`origin`, `vecline`, `angle`, `sidelist`, entity references)
//!   transformed or renamed.
//! * Names are fixed up with the instance name (`fixup_style`: 0 prefix, 1 postfix, 2 none);
//!   names starting with `@` or `!` are global and left alone.
//! * `$variables` from `replaceNN` keys (with `func_instance_parms` defaults) are substituted
//!   in all keys and connections.
//! * `instance:<name>;<io>` connections are rewired through the instance boundary.
//! * Solid/side/entity ids are offset so they stay unique; `sidelist` keys follow.

use super::{Entity, Map, Solid};
use crate::ctx::Ctx;
use crate::fgd::Fgd;
use crate::math::{angles_matrix, matrix_angles};
use anyhow::{Context, Result};
use glam::{DMat3, DVec3};
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 16;

pub struct Options<'a> {
    /// Extra directories searched for instance files (`-instancepath`).
    pub search: &'a [PathBuf],
    pub fgd: Option<&'a Fgd>,
}

/// The outermost `maps` directory in `dir`'s path (vbsp looks for `\maps\`).
fn maps_root(dir: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for c in dir.components() {
        root.push(c);
        if c.as_os_str().eq_ignore_ascii_case("maps") {
            return Some(root);
        }
    }
    None
}

pub fn resolve(file: &str, base_dir: &Path, search: &[PathBuf]) -> Option<PathBuf> {
    let file = file.trim();
    if file.is_empty() {
        return None;
    }
    let mut rel = PathBuf::from(file.replace('\\', "/"));
    if rel.extension().is_none() {
        rel.set_extension("vmf");
    }
    let dirs = std::iter::once(base_dir.to_path_buf()).chain(maps_root(base_dir)).chain(search.iter().cloned());
    dirs.map(|d| d.join(&rel)).find(|p| p.is_file())
}

/// How a key of an entity class must be transformed, from its FGD type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Plain,
    Name,
    Point,
    Angle,
    SideList,
}

fn key_kind(fgd: Option<&Fgd>, class: &str, key: &str) -> KeyKind {
    let k = key.to_ascii_lowercase();
    match k.as_str() {
        "targetname" | "parentname" => return KeyKind::Name,
        "origin" | "angles" | "classname" => return KeyKind::Plain, // handled explicitly
        _ => {}
    }
    let ty = fgd.and_then(|f| f.get(class)).and_then(|c| c.props.iter().find(|p| p.name.eq_ignore_ascii_case(&k)));
    match ty.map(|p| p.ty.to_ascii_lowercase()) {
        Some(t) => match t.as_str() {
            "target_destination" | "target_name_or_class" | "target_source" | "filterclass" => KeyKind::Name,
            "origin" | "vecline" => KeyKind::Point,
            "angle" => KeyKind::Angle,
            "sidelist" => KeyKind::SideList,
            _ => KeyKind::Plain,
        },
        // Without an FGD, fall back to the common reference keys.
        None if fgd.is_none() => match k.as_str() {
            "target" | "filtername" | "damagefilter" | "lightingorigin" | "measuretarget" | "landmark" => KeyKind::Name,
            "sides" => KeyKind::SideList,
            _ => KeyKind::Plain,
        },
        None => KeyKind::Plain,
    }
}

struct Fixup {
    name: String,
    style: i32,
}

impl Fixup {
    fn apply(&self, n: &str) -> String {
        if n.is_empty() || n.starts_with('@') || n.starts_with('!') || self.style == 2 {
            return n.to_string();
        }
        if self.style == 1 { format!("{n}-{}", self.name) } else { format!("{}-{n}", self.name) }
    }
}

/// `$var` substitutions, longest names first so `$ab` wins over `$a`.
fn substitute(s: &str, vars: &[(String, String)]) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut out = s.to_string();
    for (k, v) in vars {
        if out.to_ascii_lowercase().contains(&k.to_ascii_lowercase()) {
            out = replace_ci(&out, k, v);
        }
    }
    out
}

fn replace_ci(s: &str, pat: &str, with: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pl = pat.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(j) = lower[i..].find(&pl) {
        out.push_str(&s[i..i + j]);
        out.push_str(with);
        i += j + pl.len();
    }
    out.push_str(&s[i..]);
    out
}

struct Xform {
    rot: DMat3,
    origin: DVec3,
}

impl Xform {
    fn point(&self, p: DVec3) -> DVec3 {
        self.rot * p + self.origin
    }

    fn solid(&self, s: &mut Solid) {
        for sd in &mut s.sides {
            for p in &mut sd.plane {
                *p = self.point(*p);
            }
            // Texture lock: u(p) = p·a/s + shift must hold for the moved point.
            for ax in [&mut sd.uaxis, &mut sd.vaxis] {
                let a = self.rot * ax.vec;
                if ax.scale != 0.0 {
                    ax.shift -= self.origin.dot(a) / ax.scale;
                }
                ax.vec = a;
            }
        }
    }
}

fn fmt_num(v: f64) -> String {
    let r = v.round();
    if (v - r).abs() < 1e-4 { format!("{}", r as i64) } else { format!("{v:.4}").trim_end_matches('0').to_string() }
}

fn fmt_vec(v: DVec3) -> String {
    format!("{} {} {}", fmt_num(v.x), fmt_num(v.y), fmt_num(v.z))
}

struct Collapser<'a> {
    opts: &'a Options<'a>,
    ctx: &'a Ctx,
    next_id: i32,
    auto_names: usize,
    stack: Vec<PathBuf>,
}

/// Collapses every `func_instance` of `map` (loaded from `path`) in place.
pub fn collapse(map: &mut Map, path: &Path, opts: &Options, ctx: &Ctx) -> Result<usize> {
    let mut c = Collapser { opts, ctx, next_id: map.max_id() + 1, auto_names: 0, stack: vec![path.to_path_buf()] };
    let dir = path.parent().unwrap_or(Path::new("."));
    let n = c.collapse_map(map, dir)?;
    Ok(n)
}

impl Collapser<'_> {
    /// Returns the number of instances collapsed (including nested ones).
    fn collapse_map(&mut self, map: &mut Map, dir: &Path) -> Result<usize> {
        let mut count = 0;
        let mut i = 0;
        while i < map.entities.len() {
            if !map.entities[i].is("func_instance") {
                i += 1;
                continue;
            }
            let inst = map.entities.remove(i);
            let file = inst.get("file").unwrap_or("").to_string();
            let Some(p) = resolve(&file, dir, self.opts.search) else {
                if !file.trim().is_empty() {
                    self.ctx.warn(format!("func_instance {}: cannot find instance file {file:?}", inst.id));
                }
                continue;
            };
            if self.stack.len() > MAX_DEPTH || self.stack.contains(&p) {
                self.ctx.warn(format!("func_instance {}: recursive instance {}", inst.id, p.display()));
                continue;
            }
            let mut sub = Map::load(&p)?;
            self.stack.push(p.clone());
            count += 1 + self.collapse_map(&mut sub, p.parent().unwrap_or(Path::new("."))).with_context(|| format!("instance {}", p.display()))?;
            self.stack.pop();
            self.merge(map, &inst, sub);
            // Entities appended by merge are already collapsed; continue after the insertion point.
        }
        Ok(count)
    }

    fn merge(&mut self, map: &mut Map, inst: &Entity, mut sub: Map) {
        let x = Xform { rot: angles_matrix(inst.angles()), origin: inst.origin() };
        let mut name = inst.get("targetname").unwrap_or("").to_string();
        if name.is_empty() {
            self.auto_names += 1;
            name = format!("InstanceAuto{}", self.auto_names);
        }
        let fix = Fixup { name: name.clone(), style: inst.int("fixup_style").unwrap_or(0) as i32 };

        // Variables: defaults from func_instance_parms, then replaceNN values.
        let mut vars: Vec<(String, String)> = Vec::new();
        for e in sub.entities.iter().filter(|e| e.is("func_instance_parms")) {
            for (k, v) in &e.keys {
                if k.to_ascii_lowercase().starts_with("parm") {
                    let mut it = v.split_whitespace();
                    if let (Some(var), _ty, Some(def)) = (it.next(), it.next(), it.next()) {
                        let def = std::iter::once(def).chain(it).collect::<Vec<_>>().join(" ");
                        vars.push((var.to_string(), def));
                    }
                }
            }
        }
        for (k, v) in &inst.keys {
            if k.to_ascii_lowercase().starts_with("replace") {
                if let Some((var, val)) = v.trim().split_once(char::is_whitespace) {
                    vars.retain(|(n, _)| !n.eq_ignore_ascii_case(var));
                    vars.push((var.to_string(), val.trim().to_string()));
                } else if v.trim().starts_with('$') {
                    vars.retain(|(n, _)| !n.eq_ignore_ascii_case(v.trim()));
                    vars.push((v.trim().to_string(), String::new()));
                }
            }
        }
        vars.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));

        // Ids.
        let base = self.next_id;
        self.next_id += sub.max_id() + 1;
        let id = |i: i32| i + base;

        // World brushes merge into the outer world.
        for mut s in std::mem::take(&mut sub.world.solids) {
            x.solid(&mut s);
            renumber(&mut s, &id);
            map.world.solids.push(s);
        }

        let fgd = self.opts.fgd;
        // Outputs leaving the instance: `instance:<ent>;<output>` on the func_instance.
        let exits: Vec<(String, String, String)> = inst
            .connections
            .iter()
            .filter_map(|(o, v)| {
                let r = o.strip_prefix("instance:").or_else(|| o.strip_prefix("Instance:"))?;
                let (ent, out) = r.split_once(';')?;
                Some((ent.to_string(), out.to_string(), v.clone()))
            })
            .collect();

        for mut e in sub.entities {
            if e.is("func_instance_parms") {
                continue;
            }
            e.id = id(e.id);
            let class = e.classname().to_string();
            let mut keys = std::mem::take(&mut e.keys);
            for (k, v) in &mut keys {
                *v = substitute(v, &vars);
                match key_kind(fgd, &class, k) {
                    KeyKind::Name => *v = fix.apply(v),
                    KeyKind::Point => {
                        if let Some(p) = super::parse_vec3(v) {
                            *v = fmt_vec(x.point(p));
                        }
                    }
                    KeyKind::Angle => {
                        if let Some(a) = super::parse_vec3(v) {
                            *v = fmt_vec(matrix_angles(x.rot * angles_matrix(a)));
                        }
                    }
                    KeyKind::SideList => {
                        *v = v.split_whitespace().filter_map(|t| t.parse::<i32>().ok()).map(|i| id(i).to_string()).collect::<Vec<_>>().join(" ");
                    }
                    KeyKind::Plain => {}
                }
            }
            e.keys = keys;
            if e.get("origin").is_some() || e.solids.is_empty() {
                e.set("origin", fmt_vec(x.point(e.origin())));
            }
            let has_angles = e.get("angles").is_some() || e.get("angle").is_some();
            if has_angles || e.solids.is_empty() {
                let a = matrix_angles(x.rot * angles_matrix(e.angles()));
                e.remove("angle");
                e.set("angles", fmt_vec(a));
            }
            for (_, v) in &mut e.connections {
                *v = substitute(v, &vars);
                *v = fix_connection_target(v, &fix);
            }
            let raw_name = e.get("targetname").map(str::to_string);
            for (ent, out, v) in &exits {
                if raw_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&fix.apply(ent))) {
                    e.connections.push((out.clone(), v.clone()));
                }
            }
            for s in &mut e.solids {
                x.solid(s);
                renumber(s, &id);
            }
            map.entities.push(e);
        }

        // Inputs entering the instance: `<inst name>` + `instance:<ent>;<input>`.
        for e in std::iter::once(&mut map.world).chain(map.entities.iter_mut()) {
            for (_, v) in &mut e.connections {
                let sep = if v.contains('\x1b') { '\x1b' } else { ',' };
                let mut f: Vec<String> = v.split(sep).map(str::to_string).collect();
                if f.len() >= 2 && f[0].eq_ignore_ascii_case(&name) {
                    if let Some((ent, input)) = f[1].strip_prefix("instance:").and_then(|r| r.split_once(';')) {
                        let (ent, input) = (fix.apply(ent), input.to_string());
                        f[0] = ent;
                        f[1] = input;
                        *v = f.join(&sep.to_string());
                    }
                }
            }
        }
    }
}

fn fix_connection_target(v: &str, fix: &Fixup) -> String {
    let sep = if v.contains('\x1b') { '\x1b' } else { ',' };
    let mut f: Vec<&str> = v.split(sep).collect();
    let t = f.first().map(|t| fix.apply(t)).unwrap_or_default();
    if !f.is_empty() {
        f[0] = &t;
    }
    f.join(&sep.to_string())
}

fn renumber(s: &mut Solid, id: &dyn Fn(i32) -> i32) {
    s.id = id(s.id);
    for sd in &mut s.sides {
        sd.id = id(sd.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixups_and_vars() {
        let f = Fixup { name: "door1".into(), style: 0 };
        assert_eq!(f.apply("relay"), "door1-relay");
        assert_eq!(f.apply("@global"), "@global");
        assert_eq!(f.apply("!player"), "!player");
        assert_eq!(Fixup { name: "d".into(), style: 1 }.apply("r"), "r-d");
        let vars = vec![("$skin2".to_string(), "B".to_string()), ("$skin".to_string(), "A".to_string())];
        assert_eq!(substitute("$SKIN and $skin2", &vars), "A and B");
    }

    #[test]
    fn collapses_sample_maps() {
        let Some(dir) = crate::testutil::maps_dir() else { return };
        let p = dir.join("sp_a2_column_blocker.vmf");
        let mut m = Map::load(&p).unwrap();
        let before = m.entities.iter().filter(|e| e.is("func_instance")).count();
        let ctx = Ctx::quiet();
        let n = collapse(&mut m, &p, &Options { search: &[], fgd: None }, &ctx).unwrap();
        assert!(n >= before && before > 0);
        assert!(!m.entities.iter().any(|e| e.is("func_instance")));
        assert_eq!(ctx.warning_count(), 0, "{}", ctx.log_text());
        // Ids stay unique.
        let mut ids: Vec<i32> = m.world.solids.iter().flat_map(|s| s.sides.iter().map(|sd| sd.id)).collect();
        let len = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), len);
    }
}
