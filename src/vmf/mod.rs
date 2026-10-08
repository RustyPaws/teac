//! VMF map model for compiling. Parsed from KeyValues with strict typed fields: a malformed
//! plane or texture axis is an error that names the offending brush side.
//!
//! Objects inside `hidden` blocks (hidden visgroups / quick-hide) are not compiled, like vbsp.

pub mod instance;

use crate::kv::{self, Node, NodeList, Value};
use anyhow::{anyhow, bail, Context, Result};
use glam::DVec3;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct TexAxis {
    pub vec: DVec3,
    pub shift: f64,
    /// World units per texel.
    pub scale: f64,
}

#[derive(Clone, Debug)]
pub struct Side {
    pub id: i32,
    /// Three points, clockwise when seen from outside the brush (Hammer convention).
    pub plane: [DVec3; 3],
    pub material: String,
    pub uaxis: TexAxis,
    pub vaxis: TexAxis,
    pub lightmap_scale: i32,
    pub smoothing_groups: u32,
    /// Raw `dispinfo` block, decoded by `vbsp::disp`.
    pub dispinfo: Option<Vec<Node>>,
}

#[derive(Clone, Debug, Default)]
pub struct Solid {
    pub id: i32,
    pub sides: Vec<Side>,
}

#[derive(Clone, Debug, Default)]
pub struct Entity {
    pub id: i32,
    /// Ordered key/value pairs (excluding `id`), written to the entity lump as is.
    pub keys: Vec<(String, String)>,
    /// `connections` block: (output, "target<sep>input<sep>param<sep>delay<sep>times").
    pub connections: Vec<(String, String)>,
    pub solids: Vec<Solid>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn is(&self, class: &str) -> bool {
        self.classname().eq_ignore_ascii_case(class)
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
        self.get(key).and_then(parse_vec3)
    }

    pub fn origin(&self) -> DVec3 {
        self.vec3("origin").unwrap_or(DVec3::ZERO)
    }

    /// `angles` (pitch yaw roll), with the legacy `angle` key as yaw (-1 up, -2 down).
    pub fn angles(&self) -> DVec3 {
        if let Some(a) = self.vec3("angles") {
            return a;
        }
        match self.float("angle") {
            Some(-1.0) => DVec3::new(-90.0, 0.0, 0.0),
            Some(-2.0) => DVec3::new(90.0, 0.0, 0.0),
            Some(y) => DVec3::new(0.0, y, 0.0),
            None => DVec3::ZERO,
        }
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.trim().parse().ok())
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| {
            let v = v.trim();
            v.parse::<i64>().ok().or_else(|| v.parse::<f64>().ok().map(|f| f as i64))
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Map {
    pub world: Entity,
    pub entities: Vec<Entity>,
}

/// Parses "x y z" (parentheses/brackets ignored).
pub fn parse_vec3(s: &str) -> Option<DVec3> {
    let cleaned = s.replace(['(', ')', '[', ']'], " ");
    let mut it = cleaned.split_whitespace().map(|t| t.parse::<f64>());
    let v = DVec3::new(it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    Some(v)
}

/// Strict scanner for the bracketed number groups used by `plane` and `uaxis`/`vaxis`.
struct Nums<'a> {
    s: &'a str,
}

impl<'a> Nums<'a> {
    fn skip_ws(&mut self) {
        self.s = self.s.trim_start();
    }

    fn expect(&mut self, c: char) -> Result<()> {
        self.skip_ws();
        self.s = self.s.strip_prefix(c).ok_or_else(|| anyhow!("expected '{c}'"))?;
        Ok(())
    }

    fn number(&mut self) -> Result<f64> {
        self.skip_ws();
        let end = self.s.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']')).unwrap_or(self.s.len());
        let (tok, rest) = self.s.split_at(end);
        let v = tok.parse::<f64>().map_err(|_| anyhow!("bad number {tok:?}"))?;
        if !v.is_finite() {
            bail!("non-finite number {tok:?}");
        }
        self.s = rest;
        Ok(v)
    }

    fn end(&mut self) -> Result<()> {
        self.skip_ws();
        if !self.s.is_empty() {
            bail!("trailing text {:?}", self.s);
        }
        Ok(())
    }
}

/// `(x y z) (x y z) (x y z)`
pub fn parse_plane(s: &str) -> Result<[DVec3; 3]> {
    let mut n = Nums { s };
    let mut pts = [DVec3::ZERO; 3];
    for p in &mut pts {
        n.expect('(')?;
        *p = DVec3::new(n.number()?, n.number()?, n.number()?);
        n.expect(')')?;
    }
    n.end()?;
    Ok(pts)
}

/// `[x y z shift] scale`
pub fn parse_texaxis(s: &str) -> Result<TexAxis> {
    let mut n = Nums { s };
    n.expect('[')?;
    let vec = DVec3::new(n.number()?, n.number()?, n.number()?);
    let shift = n.number()?;
    n.expect(']')?;
    let scale = n.number()?;
    n.end()?;
    Ok(TexAxis { vec, shift, scale })
}

fn parse_id(nodes: &[Node]) -> i32 {
    nodes.get_str("id").and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

fn parse_side(nodes: &[Node]) -> Result<Side> {
    let id = parse_id(nodes);
    let field = |k: &str| nodes.get_str(k).with_context(|| format!("side {id}: missing \"{k}\""));
    let ctx = |k: &'static str| move |e: anyhow::Error| e.context(format!("side {id}: bad \"{k}\""));
    Ok(Side {
        id,
        plane: parse_plane(field("plane")?).map_err(ctx("plane"))?,
        material: field("material")?.to_string(),
        uaxis: parse_texaxis(field("uaxis")?).map_err(ctx("uaxis"))?,
        vaxis: parse_texaxis(field("vaxis")?).map_err(ctx("vaxis"))?,
        lightmap_scale: nodes.get_str("lightmapscale").and_then(|v| v.trim().parse().ok()).unwrap_or(16),
        smoothing_groups: nodes.get_str("smoothing_groups").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
        dispinfo: nodes.get_block("dispinfo").map(<[Node]>::to_vec),
    })
}

fn parse_solid(nodes: &[Node]) -> Result<Solid> {
    let id = parse_id(nodes);
    let sides = nodes.blocks("side").map(|n| parse_side(n.children())).collect::<Result<Vec<_>>>()
        .with_context(|| format!("solid {id}"))?;
    Ok(Solid { id, sides })
}

fn parse_entity(nodes: &[Node]) -> Result<Entity> {
    let mut e = Entity { id: parse_id(nodes), ..Default::default() };
    for n in nodes {
        match (&n.value, n.key.to_ascii_lowercase().as_str()) {
            (Value::Str(_), "id") => {}
            (Value::Str(v), _) => e.keys.push((n.key.clone(), v.clone())),
            (Value::Block(c), "solid") => e.solids.push(parse_solid(c).with_context(|| format!("entity {}", e.id))?),
            (Value::Block(c), "connections") => {
                e.connections.extend(c.iter().filter_map(|n| Some((n.key.clone(), n.as_str()?.to_string()))))
            }
            // `hidden`, `editor` and unknown blocks are not compiled.
            _ => {}
        }
    }
    Ok(e)
}

impl Map {
    pub fn parse(text: &str) -> Result<Map> {
        let nodes = kv::parse(text)?;
        let mut map = Map::default();
        let mut have_world = false;
        for n in &nodes {
            match (&n.value, n.key.to_ascii_lowercase().as_str()) {
                (Value::Block(c), "world") => {
                    map.world = parse_entity(c).context("world")?;
                    have_world = true;
                }
                (Value::Block(c), "entity") => map.entities.push(parse_entity(c)?),
                _ => {}
            }
        }
        if !have_world {
            bail!("no world block");
        }
        Ok(map)
    }

    pub fn load(path: &Path) -> Result<Map> {
        let b = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Map::parse(&String::from_utf8_lossy(&b)).with_context(|| format!("parsing {}", path.display()))
    }

    /// Highest solid/side/entity id, used to offset ids of collapsed instances.
    pub fn max_id(&self) -> i32 {
        let mut m = 0;
        for e in std::iter::once(&self.world).chain(&self.entities) {
            m = m.max(e.id);
            for s in &e.solids {
                m = m.max(s.id);
                for sd in &s.sides {
                    m = m.max(sd.id);
                }
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_fields() {
        let p = parse_plane("(0 0 64) (64 0 64) (64 -64 64)").unwrap();
        assert_eq!(p[2], DVec3::new(64.0, -64.0, 64.0));
        assert!(parse_plane("(0 0 64) (64 0 64)").is_err());
        assert!(parse_plane("(0 0 64) (64 0 64) (64 -64 x)").is_err());
        let a = parse_texaxis("[1 0 0 -16] 0.25").unwrap();
        assert_eq!((a.vec, a.shift, a.scale), (DVec3::X, -16.0, 0.25));
        assert!(parse_texaxis("[1 0 0] 0.25").is_err());
    }

    #[test]
    fn hidden_blocks_are_skipped() {
        let m = Map::parse(
            r#"world { "id" "1" "classname" "worldspawn"
                solid { "id" "2" side { "id" "3" "plane" "(0 0 0) (1 0 0) (0 1 0)" "material" "A"
                    "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" } }
                hidden { solid { "id" "9" } } }
            entity { "id" "4" "classname" "info_target" "targetname" "t"
                connections { "OnUser1" "x,Kill,,0,-1" } }"#,
        )
        .unwrap();
        assert_eq!(m.world.solids.len(), 1);
        assert_eq!(m.entities[0].get("targetname"), Some("t"));
        assert_eq!(m.entities[0].connections.len(), 1);
        assert_eq!(m.max_id(), 4);
    }

    #[test]
    fn parses_sample_maps() {
        let Some(dir) = crate::testutil::maps_dir() else { return };
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "vmf") {
                let m = Map::load(&p).unwrap_or_else(|e| panic!("{e:#}"));
                assert!(!m.world.solids.is_empty() || !m.entities.is_empty(), "{}", p.display());
            }
        }
    }
}
