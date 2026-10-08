//! Materials (VMT) as the compilers see them: brush contents, surface flags, texture size,
//! reflectivity, surface property and light emission. Loaded once per name and cached.
//!
//! The `%compile*` keyword semantics were matched against the texinfo/brush lumps of
//! Valve-compiled Portal 2 maps: tool keywords *assign* the flags (so e.g. the translucency of
//! `toolsclip` doesn't leak into its faces), while `%compilenodraw`, `%noportal` and `%nopaint`
//! add to them.

use crate::flags::{contents, surf};
use crate::fs::GameFs;
use crate::kv::{self, Node, Value};
use glam::DVec3;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Material {
    /// Lowercase name without `materials/` and `.vmt`, forward slashes.
    pub name: String,
    pub found: bool,
    pub shader: String,
    /// Lowercased `$key` / `%key` parameters (after `patch` resolution).
    pub params: HashMap<String, String>,
    /// Contents of a brush side with this material (before brush-level combination).
    pub contents: i32,
    pub flags: i32,
    pub width: i32,
    pub height: i32,
    pub reflectivity: DVec3,
    pub surfaceprop: String,
    pub translucent: bool,
    pub alphatest: bool,
}

impl Material {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.param(key).is_some_and(truthy)
    }

    pub fn is_lightmapped(&self) -> bool {
        self.flags & surf::NOLIGHT == 0
    }
}

fn truthy(v: &str) -> bool {
    let v = v.trim();
    v.parse::<f64>().map(|f| f != 0.0).unwrap_or(!v.is_empty())
}

/// Shaders that receive lightmaps.
const LIGHTMAPPED_SHADERS: [&str; 9] = [
    "lightmappedgeneric",
    "worldvertextransition",
    "water",
    "lightmappedreflective",
    "lightmapped_4wayblend",
    "worldtwotextureblend",
    "lightmappedtwotexture",
    "lightmappedpaint",
    "worldgeneric",
];

pub fn normalize_name(name: &str) -> String {
    let n = name.trim().replace('\\', "/").to_ascii_lowercase();
    let n = n.strip_prefix("materials/").unwrap_or(&n).to_string();
    n.strip_suffix(".vmt").unwrap_or(&n).to_string()
}

pub struct Materials<'a> {
    fs: &'a GameFs,
    cache: Mutex<HashMap<String, Arc<Material>>>,
}

impl<'a> Materials<'a> {
    pub fn new(fs: &'a GameFs) -> Materials<'a> {
        Materials { fs, cache: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, name: &str) -> Arc<Material> {
        let key = normalize_name(name);
        if let Some(m) = self.cache.lock().unwrap().get(&key) {
            return m.clone();
        }
        let m = Arc::new(self.load(&key));
        self.cache.lock().unwrap().insert(key, m.clone());
        m
    }

    /// Names that could not be found (for a summary warning).
    pub fn missing(&self) -> Vec<String> {
        let mut v: Vec<String> = self.cache.lock().unwrap().values().filter(|m| !m.found).map(|m| m.name.clone()).collect();
        v.sort();
        v
    }

    /// Parses a VMT, following `patch` materials (`include` + `insert`/`replace`).
    fn read_vmt(&self, name: &str, depth: usize) -> Option<(String, HashMap<String, String>)> {
        let text = self.fs.read_string(&format!("materials/{name}.vmt"))?;
        let nodes = kv::parse(&text).ok()?;
        let root = nodes.first()?;
        let shader = root.key.to_ascii_lowercase();
        let body = root.children();
        if shader == "patch" && depth < 8 {
            let inc = body.iter().find(|n| n.key.eq_ignore_ascii_case("include"))?.as_str()?;
            let (base_shader, mut params) = self.read_vmt(&normalize_name(inc), depth + 1)?;
            for n in body.iter().filter(|n| n.key.eq_ignore_ascii_case("insert") || n.key.eq_ignore_ascii_case("replace")) {
                collect(n.children(), &mut params);
            }
            return Some((base_shader, params));
        }
        let mut params = HashMap::new();
        collect(body, &mut params);
        Some((shader, params))
    }

    fn load(&self, name: &str) -> Material {
        let (found, shader, params) = match self.read_vmt(name, 0) {
            Some((s, p)) => (true, s, p),
            None => (false, "lightmappedgeneric".to_string(), HashMap::new()),
        };
        let mut m = Material {
            name: name.to_string(),
            found,
            shader,
            params,
            contents: contents::SOLID,
            flags: 0,
            width: 64,
            height: 64,
            reflectivity: DVec3::splat(0.5),
            surfaceprop: String::new(),
            translucent: false,
            alphatest: false,
        };
        let p = |k: &str| m.params.get(k).map(String::as_str);
        let on = |k: &str| p(k).is_some_and(truthy);

        m.surfaceprop = p("$surfaceprop").unwrap_or("default").to_ascii_lowercase();
        let alpha = p("$alpha").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(1.0);
        m.translucent = on("$translucent") || on("$additive") || alpha < 1.0;
        m.alphatest = on("$alphatest");

        let mut flags = 0;
        let mut cont = contents::SOLID;
        if !LIGHTMAPPED_SHADERS.contains(&m.shader.as_str()) {
            flags |= surf::NOLIGHT;
        }
        let water = m.shader == "water" || on("%compilewater") || on("%compileslime");
        if water {
            cont = if on("%compileslime") { contents::SLIME } else { contents::WATER };
            flags |= surf::WARP | surf::NOSHADOWS | surf::NODECALS;
        } else if m.translucent {
            flags |= surf::TRANS;
            cont = contents::WINDOW;
        }
        if on("%compilepassbullets") && cont & (contents::WATER | contents::SLIME) == 0 {
            cont = contents::GRATE;
        }
        if on("%compilesky") {
            flags = surf::SKY | surf::NOLIGHT;
        }
        if on("%compile2dsky") {
            flags = surf::SKY | surf::SKY2D | surf::NOLIGHT;
        }
        if on("%compileclip") {
            cont = contents::PLAYERCLIP | contents::MONSTERCLIP;
            flags = surf::NODRAW | surf::NOLIGHT;
        }
        if on("%playerclip") || on("%compileplayerclip") {
            cont = contents::PLAYERCLIP;
            flags = surf::NODRAW | surf::NOLIGHT;
        }
        if on("%compilenpcclip") {
            cont = contents::MONSTERCLIP;
            flags = surf::NODRAW | surf::NOLIGHT;
        }
        if on("%compileblocklos") {
            cont = contents::BLOCKLOS;
            flags = surf::NODRAW | surf::NOLIGHT;
        }
        if on("%compiletrigger") {
            cont = contents::SOLID;
            flags = surf::TRIGGER | surf::NOLIGHT;
        }
        if on("%compilehint") {
            cont = contents::EMPTY;
            flags = surf::HINT | surf::NODRAW | surf::NOLIGHT;
        }
        if on("%compileskip") {
            cont = contents::EMPTY;
            flags = surf::SKIP | surf::NODRAW | surf::NOLIGHT;
        }
        if on("%compilenolight") {
            flags = surf::NOLIGHT;
        }
        if on("%compilenodraw") {
            flags |= surf::NODRAW | surf::NOLIGHT;
        }
        if on("%compileladder") {
            cont |= contents::LADDER;
        }
        if on("%compileorigin") {
            cont = contents::ORIGIN;
        }
        if on("%compileareaportal") {
            cont = contents::AREAPORTAL;
        }
        if on("%compilenonsolid") {
            cont = contents::EMPTY;
        }
        if on("%compileblocklight") {
            cont |= contents::BLOCKLIGHT;
        }
        if on("%compiledetail") {
            cont |= contents::DETAIL;
        }
        if on("%noportal") {
            flags |= surf::NOPORTAL;
        }
        if on("%nopaint") {
            flags |= surf::NODECALS;
        }
        if on("%nochop") {
            flags |= surf::NOCHOP;
        }
        let bump = p("$bumpmap").or_else(|| if water { p("$normalmap") } else { None });
        if bump.is_some() && flags & (surf::NOLIGHT | surf::NODRAW) == 0 && !on("$nodiffusebumplighting") {
            flags |= surf::BUMPLIGHT;
        }
        m.flags = flags;
        m.contents = cont;

        // Texture size and reflectivity from the base texture's VTF header.
        let base = p("$basetexture").map(|t| t.trim().replace('\\', "/").to_ascii_lowercase());
        if let Some(h) = base.and_then(|t| self.fs.read(&format!("materials/{t}.vtf"))).and_then(|b| VtfHeader::parse(&b)) {
            m.width = h.width as i32;
            m.height = h.height as i32;
            m.reflectivity = h.reflectivity;
        }
        if let Some(r) = p("$reflectivity").and_then(crate::vmf::parse_vec3) {
            m.reflectivity = r;
        }
        m
    }
}

/// Copies `$key`/`%key` parameters (lowercased); conditional (`a?$b`) keys and fallback
/// sub-blocks are ignored.
fn collect(body: &[Node], out: &mut HashMap<String, String>) {
    for n in body {
        if let Value::Str(v) = &n.value {
            let k = n.key.to_ascii_lowercase();
            if !k.contains('?') {
                out.insert(k, v.clone());
            }
        }
    }
}

/// The parts of a VTF header the compilers use.
#[derive(Clone, Copy, Debug)]
pub struct VtfHeader {
    pub width: u16,
    pub height: u16,
    pub flags: u32,
    pub reflectivity: DVec3,
}

impl VtfHeader {
    pub fn parse(b: &[u8]) -> Option<VtfHeader> {
        if b.get(0..4)? != b"VTF\0" || b.len() < 48 {
            return None;
        }
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as f64;
        Some(VtfHeader {
            width: u16::from_le_bytes([b[16], b[17]]),
            height: u16::from_le_bytes([b[18], b[19]]),
            flags: u32::from_le_bytes(b[20..24].try_into().unwrap()),
            reflectivity: DVec3::new(f(32), f(36), f(40)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flags and contents for tool and world materials, as found in Valve-compiled maps.
    #[test]
    fn matches_valve_flags() {
        let Some(p2) = crate::testutil::portal2_dir() else { return };
        let fs = GameFs::mount(&p2.join("portal2")).unwrap();
        let mats = Materials::new(&fs);
        let cases: &[(&str, i32, Option<i32>)] = &[
            ("tools/toolsnodraw", 0x4a0, Some(contents::SOLID)),
            ("tools/toolsblack", 0x420, Some(contents::SOLID)),
            ("tools/toolstrigger", 0x440, Some(contents::SOLID)),
            ("tools/toolsinvisible", 0x490, Some(contents::GRATE)),
            ("tools/toolsblock_los", 0x480, Some(contents::BLOCKLOS | contents::DETAIL)),
            ("tools/toolsclip", 0x480, Some(contents::PLAYERCLIP | contents::MONSTERCLIP)),
            ("tools/toolsplayerclip", 0x480, Some(contents::PLAYERCLIP)),
            ("tools/toolshint", 0x580, Some(0)),
            ("tools/toolsskip", 0x680, Some(0)),
            ("tools/toolsareaportal", 0x400, None),
            ("metal/black_wall_metal_002a", 0x820, Some(contents::SOLID)),
            ("metal/metalgrate018", 0x2000, Some(contents::GRATE)),
            ("metal/citadel_metalwall060a", 0x20, None),
            ("lights/light_panel_cool", 0x400, None),
            ("effects/fizzler", 0x410, Some(contents::WINDOW)),
            ("nature/toxicslime_a2_column_blocker", 0x3808, Some(contents::WATER)),
            ("glass/glasswindow_frosted", 0x30, Some(contents::WINDOW)),
            ("tile/white_wall_tile004a", 0, None),
        ];
        for &(name, flags, cont) in cases {
            let m = mats.get(name);
            assert!(m.found, "{name}");
            assert_eq!(m.flags, flags, "{name} flags {:#x}", m.flags);
            if let Some(c) = cont {
                assert_eq!(m.contents, c, "{name} contents {:#x}", m.contents);
            }
        }
        let m = mats.get("METAL\\black_wall_metal_002a.vmt");
        assert_eq!((m.width, m.height), (1024, 1024));
        assert!((m.reflectivity - DVec3::splat(0.6)).length() < 1e-6);
    }
}
