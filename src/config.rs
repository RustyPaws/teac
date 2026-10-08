//! `teac.toml`: per-stage settings. CLI flags override the file.
//!
//! ```toml
//! [game]
//! game_dir = "C:/Program Files (x86)/Steam/steamapps/common/Portal 2/portal2"
//! fgd = ".../bin/portal2.fgd"          # used for func_instance key fixups
//! instance_path = ["extra/instances"]
//! threads = 0                         # 0 = all cores
//!
//! [bsp]
//! nodetail = false
//! max_lightmap_dim = 32
//! ```

use crate::vbsp::BspOptions;
use crate::vrad::RadOptions;
use crate::vvis::VisOptions;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GameOptions {
    /// Folder holding `gameinfo.txt` (`-game`).
    pub game_dir: Option<PathBuf>,
    pub fgd: Option<PathBuf>,
    pub instance_path: Vec<PathBuf>,
    pub threads: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub game: GameOptions,
    pub bsp: BspOptions,
    pub vis: VisOptions,
    pub rad: RadOptions,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// `--config`, else `teac.toml` next to the map, else defaults.
    pub fn find(explicit: Option<&Path>, map: &Path) -> Result<Config> {
        if let Some(p) = explicit {
            return Config::load(p);
        }
        let near = map.parent().unwrap_or(Path::new(".")).join("teac.toml");
        if near.is_file() {
            return Config::load(&near);
        }
        Ok(Config::default())
    }
}

/// Guesses the game folder for a map: walks up from the VMF and picks a sibling folder with
/// `gameinfo.txt` (e.g. `Portal 2/sdk_content/maps/x.vmf` -> `Portal 2/portal2`).
pub fn detect_game_dir(map: &Path) -> Option<PathBuf> {
    let abs = std::fs::canonicalize(map).ok()?;
    // Drop Windows' verbatim prefix (`\?\C:\...`) for readable paths.
    let abs = match abs.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(s) if !s.starts_with("UNC") => PathBuf::from(s),
        _ => abs,
    };
    for dir in abs.ancestors().skip(1) {
        if dir.join("gameinfo.txt").is_file() {
            return Some(dir.to_path_buf());
        }
        let mut cands: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("gameinfo.txt").is_file())
            .collect();
        cands.sort();
        // Prefer a game folder with its own VPK over DLC folders.
        if let Some(p) = cands.iter().find(|p| p.join("pak01_dir.vpk").is_file()).or(cands.first()) {
            return Some(p.clone());
        }
    }
    None
}

/// `<engine root>/bin/<game folder name>.fgd`.
pub fn detect_fgd(game_dir: &Path) -> Option<PathBuf> {
    let name = game_dir.file_name()?.to_string_lossy().to_string();
    let p = game_dir.parent()?.join("bin").join(format!("{name}.fgd"));
    p.is_file().then_some(p)
}
