// 输出初始地形（未侵蚀）为 raw f32
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let w: usize = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(512);
    let h: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(512);
    let seed: u64 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(42);
    let out = args
        .get(4)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/heightmaps/terrain_initial.raw"));
    let mut hm = prism_build_pipeline::generate_terrain(w, h, seed, w as f64);
    hm.normalize();
    hm.denormalize(-11000.0, 8850.0);
    let mut buf = Vec::with_capacity(hm.data.len() * 4);
    for &v in &hm.data {
        buf.extend_from_slice(&(v as f32).to_le_bytes());
    }
    std::fs::create_dir_all(out.parent().unwrap())?;
    std::fs::write(&out, buf)?;
    println!("initial terrain {}x{} written", w, h);
    Ok(())
}
