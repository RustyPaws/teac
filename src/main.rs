//! teac: native VMF -> BSP compiler.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use console::style;
use std::path::{Path, PathBuf};
use teac::bspfile::{self, BspFile};
use teac::config::{self, Config};
use teac::ctx::{fmt_duration, Ctx};
use teac::fgd::Fgd;
use teac::fs::GameFs;
use teac::vmf::{self, Map};

#[derive(Parser)]
#[command(name = "teac", version, about = "Native Source engine map compiler (VMF -> BSP)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Common {
    /// Game folder containing gameinfo.txt (detected from the map path if omitted).
    #[arg(long)]
    game: Option<PathBuf>,
    /// Settings file (default: teac.toml next to the map).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Worker threads (0 = all cores).
    #[arg(long)]
    threads: Option<usize>,
    /// Show detailed messages.
    #[arg(long, short)]
    verbose: bool,
    /// Output BSP path (default: next to the map). The .prt/.lin/.log files go beside it.
    #[arg(long, short)]
    out: Option<PathBuf>,
}

impl Common {
    /// `<out or map>` with the given extension (its folder is created).
    fn output(&self, map: &Path, ext: &str) -> PathBuf {
        let p = self.out.clone().unwrap_or_else(|| map.to_path_buf()).with_extension(ext);
        if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        p
    }
}

#[derive(Args, Clone, Default)]
struct BspFlags {
    #[arg(long)]
    onlyents: bool,
    #[arg(long)]
    nodetail: bool,
    #[arg(long)]
    nowater: bool,
    #[arg(long)]
    noweld: bool,
    #[arg(long)]
    nomerge: bool,
    #[arg(long)]
    nosubdiv: bool,
    #[arg(long)]
    notjunc: bool,
    #[arg(long)]
    leaktest: bool,
    /// Extra directory to search for instance VMFs.
    #[arg(long)]
    instancepath: Vec<PathBuf>,
}

#[derive(Args, Clone, Default)]
struct VisFlags {
    /// Base vis only (no portal flow).
    #[arg(long)]
    fast: bool,
    /// Maximum visibility distance.
    #[arg(long)]
    radius_override: Option<f64>,
}

#[derive(Args, Clone, Default)]
struct RadFlags {
    /// Quick preview lighting.
    #[arg(long = "fast-rad")]
    fast_rad: bool,
    /// High quality lighting.
    #[arg(long = "final")]
    final_: bool,
    /// Maximum radiosity bounces (0 = direct light only).
    #[arg(long)]
    bounce: Option<usize>,
    /// Sky ambient ray multiplier.
    #[arg(long)]
    extrasky: Option<usize>,
    /// Patch size for radiosity in world units.
    #[arg(long)]
    chop: Option<f64>,
    /// Extra .rad file with texture lights.
    #[arg(long)]
    lights: Option<PathBuf>,
    /// Also write HDR lighting.
    #[arg(long)]
    hdr: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the lump table and statistics of a BSP file.
    Info { bsp: PathBuf },
    /// Run the geometry stage (vbsp): <map>.vmf -> <map>.bsp + .prt (+ .lin on leaks).
    Bsp {
        map: PathBuf,
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        flags: BspFlags,
    },
    /// Run the visibility stage (vvis) on <map>.bsp using <map>.prt.
    Vis {
        map: PathBuf,
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        flags: VisFlags,
    },
    /// Run the lighting stage (vrad) on <map>.bsp.
    Rad {
        map: PathBuf,
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        flags: RadFlags,
    },
    /// Compile a VMF through all stages (bsp -> vis -> rad).
    Compile {
        map: PathBuf,
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        bsp: BspFlags,
        #[command(flatten)]
        vis: VisFlags,
        #[command(flatten)]
        rad: RadFlags,
        /// Skip the visibility stage.
        #[arg(long)]
        novis: bool,
        /// Skip the lighting stage.
        #[arg(long)]
        norad: bool,
    },
}

/// Valve tools take `-flag`; turn single-dash words into `--flag` for clap.
fn valve_args() -> Vec<String> {
    std::env::args()
        .map(|a| {
            if a.len() > 2 && a.starts_with('-') && !a.starts_with("--") && a[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                format!("-{}", a.to_ascii_lowercase())
            } else {
                a
            }
        })
        .collect()
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse_from(valve_args());
    if let Err(e) = run(cli) {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Info { bsp } => {
            let b = BspFile::read(&bsp)?;
            print!("{}", bspfile::info::report(&b));
        }
        Cmd::Bsp { map, common, flags } => {
            let mut cfg = Config::find(common.config.as_deref(), &map)?;
            apply_bsp_flags(&mut cfg, &flags);
            let ctx = Ctx::new(common.verbose);
            setup_threads(common.threads.unwrap_or(cfg.game.threads));
            let res = compile_bsp(&ctx, &cfg, &common, &map);
            write_log(&ctx, &common.output(&map, "log"));
            res?;
            ctx.success(format!("done in {} ({} warnings)", fmt_duration(ctx.elapsed()), ctx.warning_count()));
        }
        Cmd::Vis { map, common, flags } => {
            let mut cfg = Config::find(common.config.as_deref(), &map)?;
            apply_vis_flags(&mut cfg, &flags);
            let ctx = Ctx::new(common.verbose);
            setup_threads(common.threads.unwrap_or(cfg.game.threads));
            let res = (|| -> Result<()> {
                let bsp_path = common.output(&map, "bsp");
                let mut bsp = BspFile::read(&bsp_path)?;
                let prt = std::fs::read_to_string(common.output(&map, "prt")).context("reading the portal file (run `teac bsp` first; leaked maps have none)")?;
                let pf = teac::vbsp::portals::PortalFile::parse(&prt)?;
                teac::vvis::run(&cfg.vis, &ctx, &mut bsp, &pf)?;
                bsp.write(&bsp_path)
            })();
            write_log(&ctx, &common.output(&map, "log"));
            res?;
            ctx.success(format!("done in {}", fmt_duration(ctx.elapsed())));
        }
        Cmd::Rad { map, common, flags } => {
            let mut cfg = Config::find(common.config.as_deref(), &map)?;
            apply_rad_flags(&mut cfg, &flags);
            let ctx = Ctx::new(common.verbose);
            setup_threads(common.threads.unwrap_or(cfg.game.threads));
            let res = (|| -> Result<()> {
                let bsp_path = common.output(&map, "bsp");
                let mut bsp = BspFile::read(&bsp_path)?;
                let fs = game_dir(&cfg, &common, &map).ok().and_then(|g| GameFs::mount(&g).ok());
                let map_rad = std::fs::read_to_string(map.with_extension("rad")).ok();
                teac::vrad::run(&cfg.rad, &ctx, &mut bsp, fs.as_ref(), map_rad.as_deref())?;
                bsp.write(&bsp_path)
            })();
            write_log(&ctx, &common.output(&map, "log"));
            res?;
            ctx.success(format!("done in {}", fmt_duration(ctx.elapsed())));
        }
        Cmd::Compile { map, common, bsp, vis, rad, novis, norad } => {
            let mut cfg = Config::find(common.config.as_deref(), &map)?;
            apply_bsp_flags(&mut cfg, &bsp);
            apply_vis_flags(&mut cfg, &vis);
            apply_rad_flags(&mut cfg, &rad);
            let ctx = Ctx::new(common.verbose);
            setup_threads(common.threads.unwrap_or(cfg.game.threads));
            let res = compile_all(&ctx, &cfg, &common, &map, novis, norad);
            write_log(&ctx, &common.output(&map, "log"));
            res?;
            ctx.success(format!("compiled in {} ({} warnings)", fmt_duration(ctx.elapsed()), ctx.warning_count()));
        }
    }
    Ok(())
}

fn apply_vis_flags(cfg: &mut Config, f: &VisFlags) {
    cfg.vis.fast |= f.fast;
    if let Some(r) = f.radius_override {
        cfg.vis.radius_override = r;
    }
}

fn apply_rad_flags(cfg: &mut Config, f: &RadFlags) {
    let r = &mut cfg.rad;
    r.fast |= f.fast_rad;
    r.final_ |= f.final_;
    r.hdr |= f.hdr;
    if let Some(b) = f.bounce {
        r.bounce = b;
    }
    if let Some(x) = f.extrasky {
        r.extrasky = x;
    }
    if let Some(c) = f.chop {
        r.chop = c;
    }
    if f.lights.is_some() {
        r.lights = f.lights.clone();
    }
}

fn compile_all(ctx: &Ctx, cfg: &Config, common: &Common, map_path: &Path, novis: bool, norad: bool) -> Result<()> {
    let mut out = run_vbsp(ctx, cfg, common, map_path)?;
    match (&out.portals, novis) {
        (Some(pf), false) => teac::vvis::run(&cfg.vis, ctx, &mut out.bsp, pf)?,
        (None, false) => ctx.warn("map leaked: skipping vis"),
        _ => {}
    }
    if !norad {
        let game = game_dir(cfg, common, map_path)?;
        let fs = GameFs::mount(&game)?;
        let map_rad = std::fs::read_to_string(map_path.with_extension("rad")).ok();
        teac::vrad::run(&cfg.rad, ctx, &mut out.bsp, Some(&fs), map_rad.as_deref())?;
    }
    let bsp_path = common.output(map_path, "bsp");
    out.bsp.write(&bsp_path)?;
    ctx.info(format!("wrote {}", bsp_path.display()));
    Ok(())
}

fn apply_bsp_flags(cfg: &mut Config, f: &BspFlags) {
    let b = &mut cfg.bsp;
    b.onlyents |= f.onlyents;
    b.nodetail |= f.nodetail;
    b.nowater |= f.nowater;
    b.noweld |= f.noweld;
    b.nomerge |= f.nomerge;
    b.nosubdiv |= f.nosubdiv;
    b.notjunc |= f.notjunc;
    b.leaktest |= f.leaktest;
    cfg.game.instance_path.extend(f.instancepath.iter().cloned());
}

fn setup_threads(n: usize) {
    if n > 0 {
        let _ = rayon::ThreadPoolBuilder::new().num_threads(n).build_global();
    }
}

fn write_log(ctx: &Ctx, path: &Path) {
    let _ = std::fs::write(path.with_extension("log"), ctx.log_text());
}

fn game_dir(cfg: &Config, common: &Common, map: &Path) -> Result<PathBuf> {
    common
        .game
        .clone()
        .or_else(|| cfg.game.game_dir.clone())
        .or_else(|| config::detect_game_dir(map))
        .context("game folder not found: pass --game <folder with gameinfo.txt>")
}

fn compile_bsp(ctx: &Ctx, cfg: &Config, common: &Common, map_path: &Path) -> Result<()> {
    let out = run_vbsp(ctx, cfg, common, map_path)?;
    let bsp_path = common.output(map_path, "bsp");
    out.bsp.write(&bsp_path)?;
    ctx.info(format!("wrote {}", bsp_path.display()));
    Ok(())
}

/// vbsp plus its side files (.prt, .lin).
fn run_vbsp(ctx: &Ctx, cfg: &Config, common: &Common, map_path: &Path) -> Result<teac::vbsp::VbspOutput> {
    let game = game_dir(cfg, common, map_path)?;
    let mut ph = ctx.phase("Mounting game");
    let fs = GameFs::mount(&game)?;
    ph.note(format!("{} ({} mounts)", game.display(), fs.mount_count()));
    drop(ph);

    let mut ph = ctx.phase("Reading VMF");
    let mut map = Map::load(map_path)?;
    let fgd_path = cfg.game.fgd.clone().or_else(|| config::detect_fgd(&game));
    let fgd = fgd_path.as_deref().map(Fgd::load);
    let n = vmf::instance::collapse(&mut map, map_path, &vmf::instance::Options { search: &cfg.game.instance_path, fgd: fgd.as_ref() }, ctx)?;
    ph.note(format!("{} solids, {} entities, {n} instances", map.world.solids.len(), map.entities.len()));
    drop(ph);

    let name = map_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let out = teac::vbsp::run(&cfg.bsp, ctx, &map, &fs, &name)?;
    if let Some(pf) = &out.portals {
        std::fs::write(common.output(map_path, "prt"), pf.to_text())?;
    }
    let lin = common.output(map_path, "lin");
    if let Some(path) = &out.leak {
        let text: String = path.iter().map(|p| format!("{} {} {}\n", p.x, p.y, p.z)).collect();
        std::fs::write(&lin, text)?;
        ctx.warn(format!("leak pointfile written to {}", lin.display()));
    } else {
        let _ = std::fs::remove_file(&lin);
    }
    Ok(out)
}
