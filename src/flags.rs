//! Brush contents (`CONTENTS_*`) and surface flags (`SURF_*`) shared by all stages.

pub mod contents {
    pub const EMPTY: i32 = 0;
    pub const SOLID: i32 = 0x1;
    pub const WINDOW: i32 = 0x2;
    pub const AUX: i32 = 0x4;
    pub const GRATE: i32 = 0x8;
    pub const SLIME: i32 = 0x10;
    pub const WATER: i32 = 0x20;
    pub const BLOCKLOS: i32 = 0x40;
    pub const OPAQUE: i32 = 0x80;
    pub const TESTFOGVOLUME: i32 = 0x100;
    pub const BLOCKLIGHT: i32 = 0x400;
    pub const TEAM1: i32 = 0x800;
    pub const TEAM2: i32 = 0x1000;
    pub const IGNORE_NODRAW_OPAQUE: i32 = 0x2000;
    pub const MOVEABLE: i32 = 0x4000;
    pub const AREAPORTAL: i32 = 0x8000;
    pub const PLAYERCLIP: i32 = 0x10000;
    pub const MONSTERCLIP: i32 = 0x20000;
    pub const CURRENT_0: i32 = 0x40000;
    pub const CURRENT_90: i32 = 0x80000;
    pub const CURRENT_180: i32 = 0x100000;
    pub const CURRENT_270: i32 = 0x200000;
    pub const CURRENT_UP: i32 = 0x400000;
    pub const CURRENT_DOWN: i32 = 0x800000;
    pub const ORIGIN: i32 = 0x1000000;
    pub const MONSTER: i32 = 0x2000000;
    pub const DEBRIS: i32 = 0x4000000;
    pub const DETAIL: i32 = 0x8000000;
    pub const TRANSLUCENT: i32 = 0x10000000;
    pub const LADDER: i32 = 0x20000000;
    pub const HITBOX: i32 = 0x40000000;

    /// Contents that don't block visibility (portals pass through them).
    pub const LAST_VISIBLE: i32 = OPAQUE;
    pub const ALL_VISIBLE: i32 = LAST_VISIBLE | (LAST_VISIBLE - 1);
    /// Contents through which vis portals flow.
    pub const SEE_THROUGH: i32 = WINDOW | GRATE | WATER | SLIME | TRANSLUCENT;
}

pub mod surf {
    pub const LIGHT: i32 = 0x0001;
    pub const SKY2D: i32 = 0x0002;
    pub const SKY: i32 = 0x0004;
    pub const WARP: i32 = 0x0008;
    pub const TRANS: i32 = 0x0010;
    pub const NOPORTAL: i32 = 0x0020;
    pub const TRIGGER: i32 = 0x0040;
    pub const NODRAW: i32 = 0x0080;
    pub const HINT: i32 = 0x0100;
    pub const SKIP: i32 = 0x0200;
    pub const NOLIGHT: i32 = 0x0400;
    pub const BUMPLIGHT: i32 = 0x0800;
    pub const NOSHADOWS: i32 = 0x1000;
    pub const NODECALS: i32 = 0x2000;
    pub const NOCHOP: i32 = 0x4000;
    pub const HITBOX: i32 = 0x8000;
}
