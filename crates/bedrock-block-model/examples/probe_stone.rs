use std::path::Path;
use bedrock_block_model::ObjTextureResolver;

fn main() {
    let root = Path::new("target/debug/BMCBL/versions/26.40");
    let resolver = ObjTextureResolver::with_package_roots([root], "textures");
    let samples = [
        ("minecraft_stone", [0, 1, 0]),
        ("minecraft_stone_side", [1, 0, 0]),
        ("minecraft_cobblestone", [0, 1, 0]),
        ("minecraft_cobblestone_side", [1, 0, 0]),
        ("minecraft_smooth_stone", [0, 1, 0]),
        ("minecraft_deepslate", [1, 0, 0]),
        ("minecraft_cobbled_deepslate", [1, 0, 0]),
        ("minecraft_andesite", [1, 0, 0]),
        ("minecraft_diorite", [1, 0, 0]),
        ("minecraft_granite", [1, 0, 0]),
        ("minecraft_tuff", [1, 0, 0]),
        ("minecraft_raw_iron_block", [1, 0, 0]),
        ("minecraft_raw_copper_block", [1, 0, 0]),
        ("minecraft_raw_gold_block", [1, 0, 0]),
        ("minecraft_gravel", [1, 0, 0]),
    ];
    for (material, normal) in samples {
        match resolver.texture_for(material, normal) {
            Some(texture) => {
                let path = texture.relative_path.clone();
                let pack = texture.pack_root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let bytes = texture.source_path.read().ok();
                let (avg, size) = match bytes {
                    Some(bytes) => match image::load_from_memory(&bytes) {
                        Ok(image) => {
                            let rgba = image.to_rgba8();
                            let mut sum = [0u64; 3];
                            let mut count = 0u64;
                            for pixel in rgba.pixels() {
                                if pixel[3] == 0 { continue; }
                                sum[0] += u64::from(pixel[0]);
                                sum[1] += u64::from(pixel[1]);
                                sum[2] += u64::from(pixel[2]);
                                count += 1;
                            }
                            let count = count.max(1);
                            (Some([(sum[0]/count) as u8, (sum[1]/count) as u8, (sum[2]/count) as u8]), Some(rgba.dimensions()))
                        }
                        Err(_) => (None, None),
                    },
                    None => (None, None),
                };
                println!("{material} -> {path} [pack {pack}] size={size:?} avg={avg:?}");
            }
            None => println!("{material} -> MISSING"),
        }
    }
}