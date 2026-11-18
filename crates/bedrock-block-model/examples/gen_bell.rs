//! One-shot: decode standalone glass pane capture.
use std::path::Path;
use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;
fn main() {
    let root = Path::new("target/debug/BMCBL/versions/26.60/mods/BedrockSceneCapture/data/export/1.26.60.5");
    let catalog = RuntimeCaptureCatalog::open(root).expect("catalog");
    match catalog.debug_decoded_shape(290, 3075, 501) {
        Some(shape) => {
            println!("cuboids={} planes={}", shape.cuboids.len(), shape.planes.len());
            for c in shape.cuboids.iter() {
                println!("cuboid ({:.3},{:.3},{:.3})..({:.3},{:.3},{:.3}) slot={:?}", c.min[0],c.min[1],c.min[2],c.max[0],c.max[1],c.max[2], c.material_slot);
                for (f, uv) in &c.face_uvs {
                    let px = uv.map(|p| (p[0]*16.0, p[1]*16.0));
                    println!("   {:?} uv {:?}", f, px);
                }
            }
            for p in shape.planes.iter() {
                let uv = p.uv.map(|uv| uv.map(|c| (c[0]*16.0, c[1]*16.0)));
                println!("plane n={:?} slot={:?} uv={:?}", p.normal, p.material_slot, uv);
            }
        }
        None => println!("<decode failed>"),
    }
}
