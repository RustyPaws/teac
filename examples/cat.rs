//! Debug helper: print files from the mounted game file system.
fn main() {
    let mut args = std::env::args().skip(1);
    let game = args.next().expect("game dir");
    let fs = teac::fs::GameFs::mount(std::path::Path::new(&game)).unwrap();
    for a in args {
        println!("==== {a}");
        match fs.read_string(&a) {
            Some(s) => println!("{s}"),
            None => println!("(missing)"),
        }
    }
}
