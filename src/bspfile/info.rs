//! `teac info`: lump table and element counts (like Valve's vbspinfo).

use super::*;
use std::fmt::Write;

/// Element size of each structured lump (`None` for blobs).
fn elem_size(idx: usize) -> Option<usize> {
    use lump::*;
    Some(match idx {
        PLANES => size_of::<DPlane>(),
        TEXDATA => size_of::<DTexData>(),
        VERTEXES | VERTNORMALS | CLIPPORTALVERTS | PRIMVERTS => 12,
        NODES => size_of::<DNode>(),
        TEXINFO => size_of::<TexInfo>(),
        FACES | ORIGINALFACES | FACES_HDR => size_of::<DFace>(),
        LEAFS => size_of::<DLeaf>(),
        FACEIDS | LEAFFACES | LEAFBRUSHES | VERTNORMALINDICES | PRIMINDICES | LEAFMINDISTTOWATER
        | FACE_MACRO_TEXTURE_INFO | DISP_TRIS | FACEBRUSHES => 2,
        FACEBRUSHLIST => size_of::<DFaceBrushList>(),
        EDGES => size_of::<DEdge>(),
        SURFEDGES | TEXDATA_STRING_TABLE => 4,
        MODELS => size_of::<DModel>(),
        WORLDLIGHTS | WORLDLIGHTS_HDR => size_of::<DWorldLight>(),
        BRUSHES => size_of::<DBrush>(),
        BRUSHSIDES => size_of::<DBrushSide>(),
        AREAS => size_of::<DArea>(),
        AREAPORTALS => size_of::<DAreaPortal>(),
        DISPINFO => size_of::<DDispInfo>(),
        DISP_VERTS => size_of::<DDispVert>(),
        LEAFWATERDATA => size_of::<DLeafWaterData>(),
        PRIMITIVES => size_of::<DPrimitive>(),
        CUBEMAPS => size_of::<DCubemapSample>(),
        OVERLAYS => size_of::<DOverlay>(),
        OVERLAY_FADES => size_of::<DOverlayFade>(),
        OVERLAY_SYSTEM_LEVELS => size_of::<DOverlaySystemLevel>(),
        LEAF_AMBIENT_INDEX | LEAF_AMBIENT_INDEX_HDR => size_of::<DLeafAmbientIndex>(),
        LEAF_AMBIENT_LIGHTING | LEAF_AMBIENT_LIGHTING_HDR => size_of::<DLeafAmbientLighting>(),
        LIGHTING | LIGHTING_HDR => size_of::<ColorRgbExp32>(),
        _ => return None,
    })
}

/// Verifies that every structured lump is a whole number of elements.
pub fn check_lump_sizes(bsp: &BspFile) -> Result<()> {
    for (i, l) in bsp.lumps.iter().enumerate() {
        if let Some(sz) = elem_size(i) {
            ensure!(l.data.len() % sz == 0, "lump {} has {} bytes, not a multiple of {sz}", lump::NAMES[i], l.data.len());
        }
    }
    Ok(())
}

pub fn report(bsp: &BspFile) -> String {
    use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, CellAlignment, Color, ContentArrangement, Table};
    let mut t = Table::new();
    t.load_preset(UTF8_FULL_CONDENSED).set_content_arrangement(ContentArrangement::Dynamic);
    t.set_header(["#", "lump", "ver", "bytes", "count"].map(|h| Cell::new(h).fg(Color::Cyan)));
    let right = |s: String| Cell::new(s).set_alignment(CellAlignment::Right);
    for (i, l) in bsp.lumps.iter().enumerate() {
        let bytes = if i == lump::GAME_LUMP { bsp.game_lumps.iter().map(|g| g.data.len()).sum() } else { l.data.len() };
        if bytes == 0 {
            continue;
        }
        let count = elem_size(i).map(|sz| (bytes / sz).to_string()).unwrap_or_default();
        t.add_row([right(i.to_string()), Cell::new(lump::NAMES[i]), right(l.version.to_string()), right(fmt_bytes(bytes)), right(count)]);
    }
    for g in &bsp.game_lumps {
        t.add_row([
            Cell::new(""),
            Cell::new(format!("  game lump {}", g.name())).fg(Color::DarkGrey),
            right(g.version.to_string()),
            right(fmt_bytes(g.data.len())),
            Cell::new(""),
        ]);
    }
    let mut s = String::new();
    let _ = writeln!(s, "VBSP version {}, map revision {}", bsp.version, bsp.map_revision);
    let _ = writeln!(s, "{t}");
    if let Ok(names) = pakfile::list(bsp) {
        let _ = writeln!(s, "pakfile: {} files", names.len());
    }
    let _ = writeln!(s, "entities: {}", bsp.entities().matches("\"classname\"").count());
    s
}

fn fmt_bytes(n: usize) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1048576 => format!("{:.1} KiB", n as f64 / 1024.0),
        _ => format!("{:.2} MiB", n as f64 / 1048576.0),
    }
}
