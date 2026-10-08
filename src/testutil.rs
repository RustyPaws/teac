//! Helpers for tests that need the local Portal 2 install (skipped when it is absent).

use std::path::PathBuf;

/// The Portal 2 install: `$PORTAL2_DIR`, else the `Portal 2` link next to Cargo.toml.
pub fn portal2_dir() -> Option<PathBuf> {
    let p = std::env::var_os("PORTAL2_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Portal 2"));
    p.join("portal2").is_dir().then_some(p)
}

/// `sdk_content/maps`, which holds sample VMFs and Valve-compiled reference BSPs.
pub fn maps_dir() -> Option<PathBuf> {
    portal2_dir().map(|p| p.join("sdk_content").join("maps"))
}
