//! Game file system: directories and VPK archives mounted from `gameinfo.txt` `SearchPaths`.
//! Thread-safe (`Send + Sync`): archives are read with positioned reads on demand.

use crate::kv::{self, NodeList, Value};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

struct VpkEntry {
    archive: u16,
    offset: u32,
    length: u32,
    preload: Vec<u8>,
}

pub struct Vpk {
    base: String,
    dir_path: PathBuf,
    data_start: u64,
    files: HashMap<String, VpkEntry>,
}

fn cstr(b: &[u8], i: &mut usize) -> Option<String> {
    let st = *i;
    let end = st + b.get(st..)?.iter().position(|&c| c == 0)?;
    *i = end + 1;
    Some(String::from_utf8_lossy(&b[st..end]).into_owned())
}

fn read_range(path: &Path, off: u64, len: usize) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

impl Vpk {
    pub fn open(dir_path: &Path) -> Result<Vpk> {
        let hdr = read_range(dir_path, 0, 12).context("short VPK header")?;
        let u32at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        if u32at(&hdr, 0) != 0x55AA1234 {
            bail!("{}: not a VPK", dir_path.display());
        }
        let header_size = if u32at(&hdr, 4) >= 2 { 28 } else { 12 };
        let tree_size = u32at(&hdr, 8) as usize;
        let tree = read_range(dir_path, header_size as u64, tree_size).context("short VPK tree")?;
        let mut files = HashMap::new();
        let mut i = 0;
        let bad = || anyhow::anyhow!("{}: corrupt VPK tree", dir_path.display());
        loop {
            let ext = cstr(&tree, &mut i).ok_or_else(bad)?;
            if ext.is_empty() {
                break;
            }
            loop {
                let path = cstr(&tree, &mut i).ok_or_else(bad)?;
                if path.is_empty() {
                    break;
                }
                loop {
                    let name = cstr(&tree, &mut i).ok_or_else(bad)?;
                    if name.is_empty() {
                        break;
                    }
                    let e = tree.get(i..i + 18).ok_or_else(bad)?;
                    let preload_len = u16::from_le_bytes([e[4], e[5]]) as usize;
                    let archive = u16::from_le_bytes([e[6], e[7]]);
                    let offset = u32at(e, 8);
                    let length = u32at(e, 12);
                    i += 18;
                    let preload = tree.get(i..i + preload_len).ok_or_else(bad)?.to_vec();
                    i += preload_len;
                    let full = if path == " " { format!("{name}.{ext}") } else { format!("{path}/{name}.{ext}") };
                    files.insert(full.to_ascii_lowercase(), VpkEntry { archive, offset, length, preload });
                }
            }
        }
        let s = dir_path.to_string_lossy();
        Ok(Vpk {
            base: s.trim_end_matches("_dir.vpk").to_string(),
            dir_path: dir_path.to_path_buf(),
            data_start: (header_size + tree_size) as u64,
            files,
        })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let e = self.files.get(name)?;
        let mut out = e.preload.clone();
        if e.length > 0 {
            let (path, off) = if e.archive == 0x7fff {
                (self.dir_path.clone(), self.data_start + e.offset as u64)
            } else {
                (PathBuf::from(format!("{}_{:03}.vpk", self.base, e.archive)), e.offset as u64)
            };
            out.extend(read_range(&path, off, e.length as usize)?);
        }
        Some(out)
    }
}

enum Source {
    Dir(PathBuf),
    Pak(Vpk),
}

/// Mounted game content, searched in priority order.
#[derive(Default)]
pub struct GameFs {
    sources: Vec<Source>,
    pub game_dir: PathBuf,
}

const CONTENT_TAGS: [&str; 4] = ["game", "mod", "platform", "custom_mod"];

fn norm(rel: &str) -> String {
    rel.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase()
}

impl GameFs {
    /// Mounts the game folder holding `gameinfo.txt` (`-game`): `<game>_dlcN` siblings first,
    /// then the `SearchPaths` entries, each directory with its `pak01_dir.vpk`.
    pub fn mount(game_dir: &Path) -> Result<GameFs> {
        let text = std::fs::read_to_string(game_dir.join("gameinfo.txt"))
            .with_context(|| format!("reading {}", game_dir.join("gameinfo.txt").display()))?;
        let root = kv::parse(&text).context("parsing gameinfo.txt")?;
        let info = root.iter().find(|n| n.key.eq_ignore_ascii_case("GameInfo")).map(|n| n.children()).unwrap_or(&[]);
        let sp = info.get_block("FileSystem").and_then(|fs| fs.get_block("SearchPaths")).unwrap_or(&[]);
        let engine_root = game_dir.parent().unwrap_or(game_dir);

        let mut dirs: Vec<PathBuf> = dlc_dirs(game_dir);
        let mut vpks: Vec<(usize, PathBuf)> = Vec::new();
        for n in sp {
            let Value::Str(path) = &n.value else { continue };
            if !n.key.split('+').any(|t| CONTENT_TAGS.contains(&t.trim().to_ascii_lowercase().as_str())) {
                continue;
            }
            let p = path.trim().replace('\\', "/");
            let lower = p.to_ascii_lowercase();
            let (base, rest) = if let Some(r) = lower.strip_prefix("|gameinfo_path|") {
                (game_dir, &p[p.len() - r.len()..])
            } else if let Some(r) = lower.strip_prefix("|all_source_engine_paths|") {
                (engine_root, &p[p.len() - r.len()..])
            } else {
                (engine_root, p.as_str())
            };
            let rest = rest.trim_start_matches('/');
            if rest.to_ascii_lowercase().ends_with(".vpk") {
                let stem = rest[..rest.len() - 4].trim_end_matches("_dir");
                vpks.push((dirs.len(), base.join(format!("{stem}_dir.vpk"))));
            } else {
                let d = if rest.is_empty() || rest == "." { base.to_path_buf() } else { base.join(rest.trim_end_matches("/.")) };
                if d.is_dir() && !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
        if !dirs.iter().any(|d| d == game_dir) {
            dirs.push(game_dir.to_path_buf());
        }
        let mut sources = Vec::new();
        let mut opened: Vec<PathBuf> = Vec::new();
        let mut open = |p: PathBuf, sources: &mut Vec<Source>| {
            if p.is_file() && !opened.contains(&p) {
                if let Ok(v) = Vpk::open(&p) {
                    sources.push(Source::Pak(v));
                }
                opened.push(p);
            }
        };
        for (i, d) in dirs.iter().enumerate() {
            for (_, v) in vpks.iter().filter(|(at, _)| *at == i) {
                open(v.clone(), &mut sources);
            }
            sources.push(Source::Dir(d.clone()));
            open(d.join("pak01_dir.vpk"), &mut sources);
        }
        for (_, v) in vpks.iter().filter(|(at, _)| *at >= dirs.len()) {
            open(v.clone(), &mut sources);
        }
        Ok(GameFs { sources, game_dir: game_dir.to_path_buf() })
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let rel = norm(rel);
        self.sources.iter().find_map(|s| match s {
            Source::Dir(d) => std::fs::read(d.join(&rel)).ok(),
            Source::Pak(v) => v.read(&rel),
        })
    }

    pub fn read_string(&self, rel: &str) -> Option<String> {
        self.read(rel).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    pub fn exists(&self, rel: &str) -> bool {
        let rel = norm(rel);
        self.sources.iter().any(|s| match s {
            Source::Dir(d) => d.join(&rel).is_file(),
            Source::Pak(v) => v.contains(&rel),
        })
    }

    pub fn mount_count(&self) -> usize {
        self.sources.len()
    }
}

/// `<game>_dlc*` siblings (no language packs), highest DLC first.
fn dlc_dirs(game_dir: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(name)) = (game_dir.parent(), game_dir.file_name()) else { return vec![] };
    let prefix = format!("{}_dlc", name.to_string_lossy());
    let langs = ["_french", "_german", "_russian", "_spanish"];
    let mut dlcs: Vec<PathBuf> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&prefix) && !langs.iter().any(|l| n.contains(l)))
        .map(|n| parent.join(n))
        .collect();
    dlcs.sort();
    dlcs.reverse();
    dlcs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_portal2() {
        let Some(p2) = crate::testutil::portal2_dir() else { return };
        let fs = GameFs::mount(&p2.join("portal2")).unwrap();
        assert!(fs.mount_count() > 2);
        assert!(fs.exists("materials/tools/toolsnodraw.vmt"));
        assert!(fs.read("scripts/surfaceproperties.txt").is_some_and(|b| !b.is_empty()));
    }
}
