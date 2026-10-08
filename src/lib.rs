//! teac: native Source engine map compiler (VMF -> BSP).
//!
//! Stages: `vbsp` (geometry), `vvis` (visibility), `vrad` (lighting), all working on the
//! in-memory [`bspfile::BspFile`].

pub mod bspfile;
pub mod config;
pub mod ctx;
pub mod fgd;
pub mod flags;
pub mod fs;
pub mod kv;
pub mod material;
pub mod math;
pub mod vbsp;
pub mod vmf;
pub mod vrad;
pub mod vvis;
#[cfg(test)]
mod testutil;
