//! On-disk structures of BSP version 21 (Portal 2).
//!
//! Everything here is `#[repr(C)]` + `Pod`, so a lump is a plain cast of its bytes. The sizes
//! asserted at the bottom match the lump lengths of Valve-compiled Portal 2 maps.

use bytemuck::{Pod, Zeroable};

pub const VERSION: i32 = 21;
pub const HEADER_LUMPS: usize = 64;

/// Lump indices.
pub mod lump {
    pub const ENTITIES: usize = 0;
    pub const PLANES: usize = 1;
    pub const TEXDATA: usize = 2;
    pub const VERTEXES: usize = 3;
    pub const VISIBILITY: usize = 4;
    pub const NODES: usize = 5;
    pub const TEXINFO: usize = 6;
    pub const FACES: usize = 7;
    pub const LIGHTING: usize = 8;
    pub const OCCLUSION: usize = 9;
    pub const LEAFS: usize = 10;
    pub const FACEIDS: usize = 11;
    pub const EDGES: usize = 12;
    pub const SURFEDGES: usize = 13;
    pub const MODELS: usize = 14;
    pub const WORLDLIGHTS: usize = 15;
    pub const LEAFFACES: usize = 16;
    pub const LEAFBRUSHES: usize = 17;
    pub const BRUSHES: usize = 18;
    pub const BRUSHSIDES: usize = 19;
    pub const AREAS: usize = 20;
    pub const AREAPORTALS: usize = 21;
    /// u16 brush indices referenced by `FACEBRUSHLIST` entries with more than one brush.
    pub const FACEBRUSHES: usize = 22;
    /// One `DFaceBrushList` per face: the brush(es) the face was made from (portal placement).
    pub const FACEBRUSHLIST: usize = 23;
    pub const PROPHULLVERTS: usize = 24;
    pub const PROPTRIS: usize = 25;
    pub const DISPINFO: usize = 26;
    pub const ORIGINALFACES: usize = 27;
    pub const PHYSDISP: usize = 28;
    pub const PHYSCOLLIDE: usize = 29;
    pub const VERTNORMALS: usize = 30;
    pub const VERTNORMALINDICES: usize = 31;
    pub const DISP_LIGHTMAP_ALPHAS: usize = 32;
    pub const DISP_VERTS: usize = 33;
    pub const DISP_LIGHTMAP_SAMPLE_POSITIONS: usize = 34;
    pub const GAME_LUMP: usize = 35;
    pub const LEAFWATERDATA: usize = 36;
    pub const PRIMITIVES: usize = 37;
    pub const PRIMVERTS: usize = 38;
    pub const PRIMINDICES: usize = 39;
    pub const PAKFILE: usize = 40;
    pub const CLIPPORTALVERTS: usize = 41;
    pub const CUBEMAPS: usize = 42;
    pub const TEXDATA_STRING_DATA: usize = 43;
    pub const TEXDATA_STRING_TABLE: usize = 44;
    pub const OVERLAYS: usize = 45;
    pub const LEAFMINDISTTOWATER: usize = 46;
    pub const FACE_MACRO_TEXTURE_INFO: usize = 47;
    pub const DISP_TRIS: usize = 48;
    pub const PROP_BLOB: usize = 49;
    pub const WATEROVERLAYS: usize = 50;
    pub const LEAF_AMBIENT_INDEX_HDR: usize = 51;
    pub const LEAF_AMBIENT_INDEX: usize = 52;
    pub const LIGHTING_HDR: usize = 53;
    pub const WORLDLIGHTS_HDR: usize = 54;
    pub const LEAF_AMBIENT_LIGHTING_HDR: usize = 55;
    pub const LEAF_AMBIENT_LIGHTING: usize = 56;
    pub const XZIPPAKFILE: usize = 57;
    pub const FACES_HDR: usize = 58;
    pub const MAP_FLAGS: usize = 59;
    pub const OVERLAY_FADES: usize = 60;
    pub const OVERLAY_SYSTEM_LEVELS: usize = 61;
    pub const PHYSLEVEL: usize = 62;
    pub const DISP_MULTIBLEND: usize = 63;

    pub const NAMES: [&str; super::HEADER_LUMPS] = [
        "ENTITIES", "PLANES", "TEXDATA", "VERTEXES", "VISIBILITY", "NODES", "TEXINFO", "FACES",
        "LIGHTING", "OCCLUSION", "LEAFS", "FACEIDS", "EDGES", "SURFEDGES", "MODELS", "WORLDLIGHTS",
        "LEAFFACES", "LEAFBRUSHES", "BRUSHES", "BRUSHSIDES", "AREAS", "AREAPORTALS", "FACEBRUSHES",
        "FACEBRUSHLIST", "PROPHULLVERTS", "PROPTRIS", "DISPINFO", "ORIGINALFACES", "PHYSDISP",
        "PHYSCOLLIDE", "VERTNORMALS", "VERTNORMALINDICES", "DISP_LIGHTMAP_ALPHAS", "DISP_VERTS",
        "DISP_LIGHTMAP_SAMPLE_POSITIONS", "GAME_LUMP", "LEAFWATERDATA", "PRIMITIVES", "PRIMVERTS",
        "PRIMINDICES", "PAKFILE", "CLIPPORTALVERTS", "CUBEMAPS", "TEXDATA_STRING_DATA",
        "TEXDATA_STRING_TABLE", "OVERLAYS", "LEAFMINDISTTOWATER", "FACE_MACRO_TEXTURE_INFO",
        "DISP_TRIS", "PROP_BLOB", "WATEROVERLAYS", "LEAF_AMBIENT_INDEX_HDR", "LEAF_AMBIENT_INDEX",
        "LIGHTING_HDR", "WORLDLIGHTS_HDR", "LEAF_AMBIENT_LIGHTING_HDR", "LEAF_AMBIENT_LIGHTING",
        "XZIPPAKFILE", "FACES_HDR", "MAP_FLAGS", "OVERLAY_FADES", "OVERLAY_SYSTEM_LEVELS",
        "PHYSLEVEL", "DISP_MULTIBLEND",
    ];
}

/// `lump_t::version` that the Portal 2 vbsp/vrad write for each lump (observed in shipped maps).
pub fn lump_version(idx: usize) -> i32 {
    match idx {
        lump::FACES | lump::LEAFS | lump::WORLDLIGHTS | lump::LIGHTING | lump::LIGHTING_HDR
        | lump::WORLDLIGHTS_HDR | lump::LEAF_AMBIENT_LIGHTING | lump::LEAF_AMBIENT_LIGHTING_HDR
        | lump::DISP_VERTS | lump::DISP_LIGHTMAP_SAMPLE_POSITIONS | lump::FACES_HDR => 1,
        lump::OCCLUSION => 2,
        _ => 0,
    }
}

/// Static prop game lump version written by the Portal 2 vbsp.
pub const STATIC_PROP_VERSION: u16 = 9;
pub const DETAIL_PROP_VERSION: u16 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct LumpT {
    pub fileofs: i32,
    pub filelen: i32,
    pub version: i32,
    pub fourcc: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }
}

impl From<glam::DVec3> for Vec3 {
    fn from(v: glam::DVec3) -> Vec3 {
        Vec3 { x: v.x as f32, y: v.y as f32, z: v.z as f32 }
    }
}

impl From<Vec3> for glam::DVec3 {
    fn from(v: Vec3) -> glam::DVec3 {
        glam::DVec3::new(v.x as f64, v.y as f64, v.z as f64)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DPlane {
    pub normal: Vec3,
    pub dist: f32,
    pub kind: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DTexData {
    pub reflectivity: Vec3,
    pub name_string_table_id: i32,
    pub width: i32,
    pub height: i32,
    pub view_width: i32,
    pub view_height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DNode {
    pub planenum: i32,
    /// Negative children are leaves: `-1 - leaf`.
    pub children: [i32; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub firstface: u16,
    pub numfaces: u16,
    pub area: i16,
    pub padding: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TexInfo {
    pub texture_vecs: [[f32; 4]; 2],
    pub lightmap_vecs: [[f32; 4]; 2],
    pub flags: i32,
    pub texdata: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DFace {
    pub planenum: u16,
    pub side: u8,
    pub on_node: u8,
    pub firstedge: i32,
    pub numedges: i16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub surface_fog_volume_id: i16,
    pub styles: [u8; 4],
    pub lightofs: i32,
    pub area: f32,
    pub lightmap_texture_mins_in_luxels: [i32; 2],
    pub lightmap_texture_size_in_luxels: [i32; 2],
    pub orig_face: i32,
    pub num_prims: u16,
    pub first_prim_id: u16,
    pub smoothing_groups: u32,
}

/// `dleaf_t` version 1: ambient lighting lives in its own lumps.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DLeaf {
    pub contents: i32,
    pub cluster: i16,
    /// `area:9, flags:7` bitfield.
    pub area_flags: i16,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub firstleafface: u16,
    pub numleaffaces: u16,
    pub firstleafbrush: u16,
    pub numleafbrushes: u16,
    pub leaf_water_data_id: i16,
    pub padding: i16,
}

impl DLeaf {
    pub fn area(&self) -> i32 {
        (self.area_flags as u16 & 0x1ff) as i32
    }
    pub fn flags(&self) -> i32 {
        (self.area_flags as u16 >> 9) as i32
    }
    pub fn set_area_flags(&mut self, area: i32, flags: i32) {
        self.area_flags = ((area as u16 & 0x1ff) | ((flags as u16 & 0x7f) << 9)) as i16;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DEdge {
    pub v: [u16; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DModel {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub headnode: i32,
    pub firstface: i32,
    pub numfaces: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DBrush {
    pub firstside: i32,
    pub numsides: i32,
    pub contents: i32,
}

/// Per face: `count` brushes. With one brush `start` is the brush index itself, otherwise
/// it indexes the `FACEBRUSHES` list.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DFaceBrushList {
    pub count: u16,
    pub start: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DBrushSide {
    pub planenum: u16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub bevel: u8,
    pub thin: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DArea {
    pub numareaportals: i32,
    pub firstareaportal: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DAreaPortal {
    pub portal_key: u16,
    pub other_area: u16,
    pub first_clip_portal_vert: u16,
    pub clip_portal_verts: u16,
    pub planenum: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DLeafWaterData {
    pub surface_z: f32,
    pub min_z: f32,
    pub surface_texinfo_id: i16,
    pub padding: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DPrimitive {
    pub kind: u8,
    pub pad: u8,
    pub first_index: u16,
    pub index_count: u16,
    pub first_vert: u16,
    pub vert_count: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DCubemapSample {
    pub origin: [i32; 3],
    pub size: i32,
}

pub const OVERLAY_BSP_FACE_COUNT: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DOverlay {
    pub id: i32,
    pub texinfo: i16,
    /// Face count (14 bits) | render order (2 bits).
    pub face_count_and_render_order: u16,
    pub ofaces: [i32; OVERLAY_BSP_FACE_COUNT],
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub uv_points: [Vec3; 4],
    pub origin: Vec3,
    pub basis_normal: Vec3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DOverlayFade {
    pub fade_dist_min_sq: f32,
    pub fade_dist_max_sq: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DOverlaySystemLevel {
    pub min_cpu_level: u8,
    pub max_cpu_level: u8,
    pub min_gpu_level: u8,
    pub max_gpu_level: u8,
}

/// `dworldlight_t` (lump version 1, with `shadow_cast_offset`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DWorldLight {
    pub origin: Vec3,
    pub intensity: Vec3,
    pub normal: Vec3,
    pub shadow_cast_offset: Vec3,
    pub cluster: i32,
    pub kind: i32,
    pub style: i32,
    pub stopdot: f32,
    pub stopdot2: f32,
    pub exponent: f32,
    pub radius: f32,
    pub constant_attn: f32,
    pub linear_attn: f32,
    pub quadratic_attn: f32,
    pub flags: i32,
    pub texinfo: i32,
    pub owner: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ColorRgbExp32 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub exponent: i8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct CompressedLightCube {
    pub color: [ColorRgbExp32; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DLeafAmbientIndex {
    pub ambient_sample_count: u16,
    pub first_ambient_sample: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DLeafAmbientLighting {
    pub cube: CompressedLightCube,
    /// Position inside the leaf box, 0..255 on each axis.
    pub x: u8,
    pub y: u8,
    pub z: u8,
    pub pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DispSubNeighbor {
    pub neighbor: u16,
    pub orientation: u8,
    pub span: u8,
    pub neighbor_span: u8,
    pub pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DispNeighbor {
    pub sub: [DispSubNeighbor; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DispCornerNeighbors {
    pub neighbors: [u16; 4],
    pub count: u8,
    pub pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DDispInfo {
    pub start_position: Vec3,
    pub disp_vert_start: i32,
    pub disp_tri_start: i32,
    pub power: i32,
    pub min_tess: i32,
    pub smoothing_angle: f32,
    pub contents: i32,
    pub map_face: u16,
    pub pad0: u16,
    pub lightmap_alpha_start: i32,
    pub lightmap_sample_position_start: i32,
    pub edge_neighbors: [DispNeighbor; 4],
    pub corner_neighbors: [DispCornerNeighbors; 4],
    pub allowed_verts: [u32; 10],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DDispVert {
    pub vec: Vec3,
    pub dist: f32,
    pub alpha: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DOccluderData {
    pub flags: i32,
    pub firstpoly: i32,
    pub polycount: i32,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub area: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DOccluderPolyData {
    pub firstvertexindex: i32,
    pub vertexcount: i32,
    pub planenum: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct DGameLump {
    /// FourCC stored as a little-endian int: `sprp` reads as the bytes `prps`.
    pub id: [u8; 4],
    pub flags: u16,
    pub version: u16,
    pub fileofs: i32,
    pub filelen: i32,
}

const _: () = {
    assert!(size_of::<LumpT>() == 16);
    assert!(size_of::<DPlane>() == 20);
    assert!(size_of::<DTexData>() == 32);
    assert!(size_of::<DNode>() == 32);
    assert!(size_of::<TexInfo>() == 72);
    assert!(size_of::<DFace>() == 56);
    assert!(size_of::<DLeaf>() == 32);
    assert!(size_of::<DModel>() == 48);
    assert!(size_of::<DBrush>() == 12);
    assert!(size_of::<DBrushSide>() == 8);
    assert!(size_of::<DAreaPortal>() == 12);
    assert!(size_of::<DLeafWaterData>() == 12);
    assert!(size_of::<DPrimitive>() == 10);
    assert!(size_of::<DCubemapSample>() == 16);
    assert!(size_of::<DOverlay>() == 352);
    assert!(size_of::<DWorldLight>() == 100);
    assert!(size_of::<DLeafAmbientLighting>() == 28);
    assert!(size_of::<DDispInfo>() == 176);
    assert!(size_of::<DDispVert>() == 20);
    assert!(size_of::<DGameLump>() == 16);
};
