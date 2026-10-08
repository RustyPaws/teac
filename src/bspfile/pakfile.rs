//! The embedded pakfile: a ZIP archive with uncompressed (stored) entries.

use super::{lump, BspFile};
use anyhow::Result;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// All files in the pakfile, in archive order.
pub fn read_all(bsp: &BspFile) -> Result<Vec<(String, Vec<u8>)>> {
    let data = &bsp.lumps[lump::PAKFILE].data;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut z = ZipArchive::new(Cursor::new(data))?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)?;
        out.push((f.name().to_string(), buf));
    }
    Ok(out)
}

pub fn list(bsp: &BspFile) -> Result<Vec<String>> {
    let data = &bsp.lumps[lump::PAKFILE].data;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let z = ZipArchive::new(Cursor::new(data))?;
    Ok(z.file_names().map(str::to_string).collect())
}

/// Replaces the pakfile with `files` (paths use forward slashes, lowercase).
pub fn write_all(bsp: &mut BspFile, files: &[(String, Vec<u8>)]) -> Result<()> {
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, data) in files {
        z.start_file(name.replace('\\', "/"), opts)?;
        z.write_all(data)?;
    }
    let data = z.finish()?.into_inner();
    bsp.set_raw(lump::PAKFILE, data);
    Ok(())
}

/// Adds or replaces files, keeping the rest.
pub fn add_files(bsp: &mut BspFile, files: Vec<(String, Vec<u8>)>) -> Result<()> {
    let mut all = read_all(bsp)?;
    for (name, data) in files {
        let name = name.replace('\\', "/").to_lowercase();
        match all.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            Some(e) => e.1 = data,
            None => all.push((name, data)),
        }
    }
    write_all(bsp, &all)
}
