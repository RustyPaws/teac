//! BSP file container: header, raw lumps, typed lump access and game lumps.
//!
//! This is the only module that knows the on-disk layout. The version-specific structs live in
//! `vNN.rs`; exactly one version is compiled in, selected by a Cargo feature.

#[cfg(feature = "v21")]
pub mod v21;
#[cfg(feature = "v21")]
pub use v21 as fmt;

#[cfg(not(any(feature = "v21")))]
compile_error!("enable exactly one BSP version feature (e.g. `v21`)");

pub mod info;
pub mod pakfile;

pub use fmt::*;

use anyhow::{bail, ensure, Context, Result};
use bytemuck::Pod;
use std::path::Path;

/// One lump as stored in the file.
#[derive(Clone, Debug, Default)]
pub struct Lump {
    pub data: Vec<u8>,
    pub version: i32,
    pub fourcc: [u8; 4],
}

/// A game lump (static props, detail props...). Offsets are recomputed on write.
#[derive(Clone, Debug)]
pub struct GameLump {
    pub id: [u8; 4],
    pub flags: u16,
    pub version: u16,
    pub data: Vec<u8>,
}

impl GameLump {
    /// Builds the on-disk id from a readable FourCC like `"sprp"`.
    pub fn id_from(s: &str) -> [u8; 4] {
        let b = s.as_bytes();
        [b[3], b[2], b[1], b[0]]
    }
    pub fn name(&self) -> String {
        self.id.iter().rev().map(|&c| c as char).collect()
    }
}

#[derive(Clone, Debug)]
pub struct BspFile {
    pub version: i32,
    pub map_revision: i32,
    /// All lumps except `GAME_LUMP`, whose data lives in `game_lumps`.
    pub lumps: Vec<Lump>,
    pub game_lumps: Vec<GameLump>,
}

impl Default for BspFile {
    fn default() -> Self {
        BspFile::new()
    }
}

fn read_i32(b: &[u8], at: usize) -> Result<i32> {
    let s = b.get(at..at + 4).context("unexpected end of file")?;
    Ok(i32::from_le_bytes(s.try_into().unwrap()))
}

/// Casts a byte slice to `T` elements, copying (lump data is not guaranteed to be aligned).
pub fn cast_vec<T: Pod>(data: &[u8]) -> Result<Vec<T>> {
    let sz = size_of::<T>();
    ensure!(data.len() % sz == 0, "lump size {} is not a multiple of {} ({})", data.len(), sz, std::any::type_name::<T>());
    let mut v = vec![T::zeroed(); data.len() / sz];
    bytemuck::cast_slice_mut::<T, u8>(&mut v).copy_from_slice(data);
    Ok(v)
}

impl BspFile {
    pub fn new() -> BspFile {
        let lumps = (0..HEADER_LUMPS)
            .map(|i| Lump { data: Vec::new(), version: lump_version(i), fourcc: [0; 4] })
            .collect();
        BspFile { version: VERSION, map_revision: 0, lumps, game_lumps: Vec::new() }
    }

    pub fn parse(b: &[u8]) -> Result<BspFile> {
        ensure!(b.get(0..4) == Some(b"VBSP"), "not a VBSP file");
        let version = read_i32(b, 4)?;
        ensure!(version == VERSION, "BSP version {version} (this build of teac handles {VERSION})");
        let mut lumps = Vec::with_capacity(HEADER_LUMPS);
        for i in 0..HEADER_LUMPS {
            let at = 8 + i * size_of::<LumpT>();
            let h: LumpT = bytemuck::pod_read_unaligned(b.get(at..at + 16).context("truncated header")?);
            ensure!(h.fourcc == [0; 4] || h.filelen == 0, "lump {} is compressed (unsupported)", lump::NAMES[i]);
            let (o, n) = (h.fileofs as usize, h.filelen as usize);
            let data = b.get(o..o + n).with_context(|| format!("lump {} out of range", lump::NAMES[i]))?.to_vec();
            lumps.push(Lump { data, version: h.version, fourcc: h.fourcc });
        }
        let map_revision = read_i32(b, 8 + HEADER_LUMPS * 16)?;
        let mut bsp = BspFile { version, map_revision, lumps, game_lumps: Vec::new() };
        bsp.game_lumps = parse_game_lumps(b, &bsp.lumps[lump::GAME_LUMP], lump_fileofs(b, lump::GAME_LUMP)?)?;
        bsp.lumps[lump::GAME_LUMP].data.clear();
        Ok(bsp)
    }

    pub fn read(path: &Path) -> Result<BspFile> {
        let b = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        BspFile::parse(&b).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let header_len = 8 + HEADER_LUMPS * 16 + 4;
        let mut out = vec![0u8; header_len];
        out[0..4].copy_from_slice(b"VBSP");
        out[4..8].copy_from_slice(&self.version.to_le_bytes());
        out[header_len - 4..].copy_from_slice(&self.map_revision.to_le_bytes());
        for (i, l) in self.lumps.iter().enumerate() {
            while out.len() % 4 != 0 {
                out.push(0);
            }
            let ofs = out.len();
            let data = if i == lump::GAME_LUMP { write_game_lumps(&self.game_lumps, ofs) } else { l.data.clone() };
            out.extend_from_slice(&data);
            let h = LumpT { fileofs: ofs as i32, filelen: data.len() as i32, version: l.version, fourcc: l.fourcc };
            let at = 8 + i * 16;
            out[at..at + 16].copy_from_slice(bytemuck::bytes_of(&h));
        }
        out
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_bytes()).with_context(|| format!("writing {}", path.display()))
    }

    /// Decodes a lump as an array of `T`.
    pub fn get<T: Pod>(&self, idx: usize) -> Result<Vec<T>> {
        cast_vec(&self.lumps[idx].data).with_context(|| format!("lump {}", lump::NAMES[idx]))
    }

    /// Replaces a lump with an array of `T`, resetting its version to the format default.
    pub fn set<T: Pod>(&mut self, idx: usize, v: &[T]) {
        self.set_raw(idx, bytemuck::cast_slice(v).to_vec());
    }

    pub fn set_raw(&mut self, idx: usize, data: Vec<u8>) {
        self.lumps[idx] = Lump { data, version: lump_version(idx), fourcc: [0; 4] };
    }

    pub fn entities(&self) -> String {
        let d = &self.lumps[lump::ENTITIES].data;
        let end = d.iter().position(|&c| c == 0).unwrap_or(d.len());
        String::from_utf8_lossy(&d[..end]).into_owned()
    }

    pub fn set_entities(&mut self, text: &str) {
        let mut d = text.as_bytes().to_vec();
        d.push(0);
        self.set_raw(lump::ENTITIES, d);
    }

    /// Texture names from `TEXDATA_STRING_DATA` / `TEXDATA_STRING_TABLE`.
    pub fn texdata_names(&self) -> Result<Vec<String>> {
        let table: Vec<i32> = self.get(lump::TEXDATA_STRING_TABLE)?;
        let data = &self.lumps[lump::TEXDATA_STRING_DATA].data;
        table
            .iter()
            .map(|&o| {
                let s = data.get(o as usize..).context("bad texdata string offset")?;
                let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                Ok(String::from_utf8_lossy(&s[..end]).into_owned())
            })
            .collect()
    }

    pub fn game_lump(&self, name: &str) -> Option<&GameLump> {
        let id = GameLump::id_from(name);
        self.game_lumps.iter().find(|g| g.id == id)
    }
}

fn lump_fileofs(b: &[u8], idx: usize) -> Result<usize> {
    Ok(read_i32(b, 8 + idx * 16)? as usize)
}

/// Game lump directory: `int count; dgamelump_t[count]`, entries hold absolute file offsets.
fn parse_game_lumps(file: &[u8], l: &Lump, base: usize) -> Result<Vec<GameLump>> {
    if l.data.is_empty() {
        return Ok(Vec::new());
    }
    let n = read_i32(&l.data, 0)?;
    ensure!((0..1024).contains(&n), "bad game lump count {n}");
    let mut out = Vec::new();
    for k in 0..n as usize {
        let at = 4 + k * 16;
        let g: DGameLump = bytemuck::pod_read_unaligned(l.data.get(at..at + 16).context("truncated game lump dir")?);
        ensure!(g.flags & 1 == 0, "compressed game lumps are unsupported");
        let (o, len) = (g.fileofs as usize, g.filelen as usize);
        // Offsets are absolute; data normally sits right after the directory inside the lump.
        let data = match file.get(o..o + len) {
            Some(d) => d.to_vec(),
            None => bail!("game lump {k} out of range (base {base})"),
        };
        out.push(GameLump { id: g.id, flags: g.flags, version: g.version, data });
    }
    Ok(out)
}

fn write_game_lumps(g: &[GameLump], lump_ofs: usize) -> Vec<u8> {
    let mut out = (g.len() as i32).to_le_bytes().to_vec();
    let mut data_ofs = lump_ofs + 4 + g.len() * 16;
    for l in g {
        let d = DGameLump { id: l.id, flags: l.flags, version: l.version, fileofs: data_ofs as i32, filelen: l.data.len() as i32 };
        out.extend_from_slice(bytemuck::bytes_of(&d));
        data_ofs += l.data.len();
    }
    for l in g {
        out.extend_from_slice(&l.data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every lump of the reference maps decodes with our struct sizes, and a write/read cycle
    /// reproduces all lumps and game lumps exactly.
    #[test]
    fn roundtrip_reference_maps() {
        let Some(dir) = crate::testutil::maps_dir() else { return };
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_none_or(|x| x != "bsp") {
                continue;
            }
            let a = BspFile::read(&p).unwrap();
            info::check_lump_sizes(&a).unwrap_or_else(|e| panic!("{}: {e:#}", p.display()));
            let b = BspFile::parse(&a.to_bytes()).unwrap();
            for i in 0..HEADER_LUMPS {
                assert_eq!(a.lumps[i].data, b.lumps[i].data, "{} lump {}", p.display(), lump::NAMES[i]);
                assert_eq!(a.lumps[i].version, b.lumps[i].version);
            }
            assert_eq!(a.game_lumps.len(), b.game_lumps.len());
            for (x, y) in a.game_lumps.iter().zip(&b.game_lumps) {
                assert_eq!((x.id, x.version, &x.data), (y.id, y.version, &y.data));
            }
            // Typed decode -> encode is lossless.
            let mut c = a.clone();
            c.set(lump::FACES, &a.get::<DFace>(lump::FACES).unwrap());
            c.set(lump::LEAFS, &a.get::<DLeaf>(lump::LEAFS).unwrap());
            assert_eq!(a.lumps[lump::FACES].data, c.lumps[lump::FACES].data);
            assert_eq!(a.lumps[lump::LEAFS].data, c.lumps[lump::LEAFS].data);
        }
    }
}
