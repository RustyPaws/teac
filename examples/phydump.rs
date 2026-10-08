//! Debug helper: dump the header of a model's .phy file.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let fs = teac::fs::GameFs::mount(std::path::Path::new(&a[1])).unwrap();
    let b = fs.read(&a[2]).expect("missing");
    println!("{} bytes, header {:?}", b.len(), &b[..16.min(b.len())]);
    let solids = i32::from_le_bytes(b[8..12].try_into().unwrap());
    let mut o = i32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
    for s in 0..solids {
        let size = i32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        println!("solid {s}: size {size} tag {:?}", String::from_utf8_lossy(&b[o + 4..o + 8]));
        o += 4 + size;
    }
    println!("tail: {}", String::from_utf8_lossy(&b[o..(o + 300).min(b.len())]));
}
