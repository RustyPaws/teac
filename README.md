# teac

A native Rust map compiler for the Source engine: VMF → BSP in one program, replacing
`vbsp`, `vvis` and `vrad`. Portal 2 (BSP v21) is the first target; the BSP version is a Cargo
feature (`v21`, default).

```
teac compile maps/mymap.vmf                 # bsp -> vis -> rad, writes maps/mymap.bsp
teac compile maps/mymap.vmf -o out/mymap.bsp --final
teac bsp  maps/mymap.vmf                    # geometry only (+ .prt, or .lin on a leak)
teac vis  maps/mymap.vmf                    # needs mymap.bsp + mymap.prt
teac rad  maps/mymap.vmf                    # lighting on mymap.bsp
teac info maps/mymap.bsp                    # lump table
```

Valve-style flags work too (`-game <dir>`, `-nodetail`, `-instancepath <dir>`, …). The game
folder (the one with `gameinfo.txt`) is detected from the map path (`…/Portal 2/sdk_content/maps`
→ `…/Portal 2/portal2`) or given with `--game`.

## Configuration

`teac.toml` next to the map (or `--config file`); command-line flags override it.

```toml
[game]
game_dir = "C:/Program Files (x86)/Steam/steamapps/common/Portal 2/portal2"
fgd = "C:/Program Files (x86)/Steam/steamapps/common/Portal 2/bin/portal2.fgd"
instance_path = []
threads = 0                 # 0 = all cores

[bsp]
nodetail = false
nowater = false
noweld = false
nomerge = false
nosubdiv = false
notjunc = false
noprune = false
leaktest = false            # fail on leaks
max_lightmap_dim = 32

[vis]
fast = false
radius_override = 0.0

[rad]
fast = false
final = false
bounce = 100                # maximum, stops once converged
extrasky = 1
chop = 64.0                 # radiosity patch size
hdr = false
# lights = "extra.rad"
```

## Stages

| module | does |
|---|---|
| `vmf`, `vmf::instance` | VMF parsing, `func_instance` collapsing (fixups, `$parm`s, I/O proxies, FGD-typed keys) |
| `vbsp` | brushes → 1024-unit block BSP trees, portals, entity flood / leak `.lin`, faces (merge, lightmap subdivision, t-junctions), detail, brush entities, areas & area portals, water, overlays, cubemaps, static props, IVP physics collision, `.prt` |
| `vvis` | base vis + full portal flow (parallel), PVS/PAS, compressed VISIBILITY lump |
| `vrad` | BVH ray tracing (world, nodraw, `toolsblocklight`, static prop `.phy` shadows), point/spot/sun/sky ambient/texture lights, supersampled luxels, bump-mapped lightmaps, radiosity bounces, leaf ambient cubes, world lights, vertex normals, sky leaf flags |
| `bspfile` | v21 lump structs, read/write, game lumps, pakfile |

Output is checked against Valve's tools: the format round-trips Valve-compiled maps
byte-for-byte, Valve `vvis`/`vrad`/`vbspinfo` accept teac's BSPs, vis results match Valve's
within 0.5%, and direct lighting matches Valve's lightmaps within 0.4%.

## Not done yet

Displacements, detail props (`detail.vbsp`), occluders, `func_viscluster`, static prop
per-vertex lighting (`.vhv`).
