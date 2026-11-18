//! Probe: resolve terrain keys and print mean RGBA of the texture.
use std::path::Path;

use bedrock_block_model::obj::{decode_texture_image, path_extension_eq};
use bedrock_block_model::ObjTextureResolver;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: probe_liquid <version-root> <key>...");
        std::process::exit(2);
    };
    let resolver = ObjTextureResolver::with_package_roots([Path::new(&root)], "textures");
    for key in args {
        let ascii = key.starts_with("ascii:");
        let ascii2 = key.starts_with("ascii2:");
        if ascii2 {
            let key = key.trim_start_matches("ascii2:");
            if let Some(entity_key) = key.strip_prefix("entity:") {
            let ascii2 = ascii || key.starts_with("ascii2:");
            if let Some(texture) = resolver.entity_texture(entity_key) {
                let bytes = texture.source_path.read();
                let tga = path_extension_eq(texture.source_path.path(), "tga");
                if let Ok(image) = bytes
                    .and_then(|bytes| decode_texture_image(&bytes, tga))
                    .map(|image| image.to_rgba8())
                {
                    println!("{entity_key} {} {}x{}", texture.relative_path, image.width(), image.height());
                    if ascii2 || ascii {
                        for y in 0..image.height().min(64) {
                            let mut row = String::new();
                            for x in 0..image.width().min(64) {
                                let px = image.get_pixel(x, y);
                                let lum = (u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2])) / 3;
                                row.push(if px[3] < 64 { '.' } else if lum > 160 { '#' } else if lum > 96 { '+' } else { ':' });
                            }
                            println!("  {row}");
                        }
                    }
                }
            }
            continue;
        }
        let material = if key.contains("__mat_") {
                key.to_owned()
            } else {
                format!("minecraft_{key}__mat_probe")
            };
            if let Some(texture) = resolver.texture_for(&material, [0, 1, 0]) {
                let bytes = texture.source_path.read();
                let tga = path_extension_eq(texture.source_path.path(), "tga");
                if let Ok(image) = bytes
                    .and_then(|bytes| decode_texture_image(&bytes, tga))
                    .map(|image| image.to_rgba8())
                {
                    println!("{key} {} {}x{}", texture.relative_path, image.width(), image.height());
                    for y in 0..image.height().min(64) {
                        let mut row = String::new();
                        for x in 0..image.width().min(64) {
                            let px = image.get_pixel(x, y);
                            let lum = (u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2])) / 3;
                            row.push(if px[3] < 64 { '.' } else if lum > 160 { '#' } else if lum > 96 { '+' } else { ':' });
                        }
                        println!("  {row}");
                    }
                }
            }
            continue;
        }
        let key = key.strip_prefix("ascii:").unwrap_or(&key);
        if let Some(entity_key) = key.strip_prefix("entity:") {
            let ascii2 = ascii || key.starts_with("ascii2:");
            if let Some(texture) = resolver.entity_texture(entity_key) {
                let bytes = texture.source_path.read();
                let tga = path_extension_eq(texture.source_path.path(), "tga");
                if let Ok(image) = bytes
                    .and_then(|bytes| decode_texture_image(&bytes, tga))
                    .map(|image| image.to_rgba8())
                {
                    println!("{entity_key} {} {}x{}", texture.relative_path, image.width(), image.height());
                    if ascii2 || ascii {
                        for y in 0..image.height().min(64) {
                            let mut row = String::new();
                            for x in 0..image.width().min(64) {
                                let px = image.get_pixel(x, y);
                                let lum = (u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2])) / 3;
                                row.push(if px[3] < 64 { '.' } else if lum > 160 { '#' } else if lum > 96 { '+' } else { ':' });
                            }
                            println!("  {row}");
                        }
                    }
                }
            }
            continue;
        }
        let material = if key.contains("__mat_") {
            key.to_owned()
        } else {
            format!("minecraft_{key}__mat_probe")
        };
        let Some(texture) = resolver.texture_for(&material, [0, 1, 0]) else {
            println!("{key} -> <MISSING>");
            continue;
        };
        let bytes = texture.source_path.read();
        let tga = path_extension_eq(texture.source_path.path(), "tga");
        match bytes.and_then(|bytes| decode_texture_image(&bytes, tga)).map(|image| image.to_rgba8()) {
            Ok(image) if ascii => {
                println!("{key} {} {}x{}", texture.relative_path, image.width(), image.height());
                for y in 0..image.height().min(64) {
                    let mut row = String::new();
                    for x in 0..image.width().min(64) {
                        let px = image.get_pixel(x, y);
                        let lum = (u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2])) / 3;
                        row.push(if px[3] < 64 { '.' } else if lum > 160 { '#' } else if lum > 96 { '+' } else { ':' });
                    }
                    println!("  {row}");
                }
            }
            Ok(image) => {
                let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
                let mut n = 0u64;
                for px in image.pixels() {
                    let c = px.0;
                    r += u64::from(c[0]);
                    g += u64::from(c[1]);
                    b += u64::from(c[2]);
                    a += u64::from(c[3]);
                    n += 1;
                }
                if n > 0 {
                    println!(
                        "{key} -> {} mean rgba=({:.0},{:.0},{:.0},{:.0}) size={}x{}",
                        texture.relative_path,
                        r as f64 / n as f64,
                        g as f64 / n as f64,
                        b as f64 / n as f64,
                        a as f64 / n as f64,
                        image.width(),
                        image.height()
                    );
                }
            }
            Err(err) => println!("{key} -> {} read error: {err}", texture.relative_path),
        }
    }
}
