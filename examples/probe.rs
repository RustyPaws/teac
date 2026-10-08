//! Debug helper: list the brushes (after instance collapsing) that contain a point.
use glam::DVec3;
use std::path::Path;
use teac::math::Plane;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = Path::new(&a[1]);
    let p = DVec3::new(a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let search: Vec<std::path::PathBuf> = a.get(5).map(|s| vec![s.into()]).unwrap_or_default();
    let mut map = teac::vmf::Map::load(path).unwrap();
    let ctx = teac::ctx::Ctx::quiet();
    teac::vmf::instance::collapse(&mut map, path, &teac::vmf::instance::Options { search: &search, fgd: None }, &ctx).unwrap();
    for e in std::iter::once(&map.world).chain(&map.entities) {
        for s in &e.solids {
            let inside = s.sides.iter().all(|sd| Plane::from_points(&sd.plane).map_or(true, |pl| pl.distance(p) <= 0.5));
            if inside {
                println!("{} solid {} ({} sides) mats {:?}", e.classname(), s.id, s.sides.len(), s.sides.iter().map(|x| x.material.as_str()).collect::<Vec<_>>());
            }
        }
    }
}
