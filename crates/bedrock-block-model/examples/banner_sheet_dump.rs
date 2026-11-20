//! Diagnostic: dump painted-region stats for the entity banner sheet, so UV windows can be
//! picked from the real pack art instead of assumptions.
use std::path::PathBuf;

use bedrock_block_model::obj::{read_texture_copy_image, ObjTextureCopy, ObjTextureResolver};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root =
        PathBuf::from(std::env::args_os().nth(1).ok_or("usage: banner_sheet_dump PACK_ROOT")?);
    // The pack stack: version root first, then every resource pack directory (the version
    // root alone does not index all overlay brarchives — mirror `probe_roots`).
    let mut roots: Vec<PathBuf> = vec![root.clone()];
    let packs = root.join("data/resource_packs");
    if let Ok(names) = std::fs::read_dir(&packs) {
        let mut names: Vec<PathBuf> = names.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        names.sort();
        roots.extend(names);
    }
    let resolver = ObjTextureResolver::with_pack_roots(&roots, "textures");
    if let Some(found) = resolver.texture_for("standing_banner", [0, 1, 0]) {
        println!("standing_banner -> {}", found.relative_path);
    } else {
        println!("standing_banner -> <MISSING>");
    }
    // Bedrock names the base sheet `entity/banner/banner` (JE spells it `banner_base`).
    let resolved = ["entity/banner/banner_base", "entity/banner/banner"]
        .iter()
        .find_map(|key| resolver.entity_texture(key))
        .ok_or("banner base sheet not found")?;
    let copy = ObjTextureCopy {
        source_path: resolved.source_path.clone(),
        relative_path: resolved.relative_path.clone(),
        tint: None,
        alpha_mask: false,
    };
    let rgba = read_texture_copy_image(&copy)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    println!("sheet {width}x{height}");
    let regions: &[(&str, u32, u32, u32, u32)] = &[
        ("front panel (1,1)-(21,41)", 1, 1, 21, 41),
        ("back panel (22,1)-(42,41)", 22, 1, 42, 41),
        ("pole east (44,2)-(46,44)", 44, 2, 46, 44),
        ("pole north (46,2)-(48,44)", 46, 2, 48, 44),
        ("pole west (48,2)-(50,44)", 48, 2, 50, 44),
        ("pole south (50,2)-(52,44)", 50, 2, 52, 44),
        ("pole top rows (44,0)-(52,2)", 44, 0, 52, 2),
        ("rod band (2,42)-(42,44)", 2, 42, 42, 44),
        ("bar north (2,44)-(22,46)", 2, 44, 22, 46),
        ("bar south (24,44)-(44,46)", 24, 44, 44, 46),
        ("bar east (0,44)-(2,46)", 0, 44, 2, 46),
        ("bar west (22,44)-(24,46)", 22, 44, 24, 46),
        ("bar up (2,42)-(22,44)", 2, 42, 22, 44),
        ("bar down (22,42)-(42,44)", 22, 42, 42, 44),
        ("flag east col (21,1)-(22,41)", 21, 1, 22, 41),
        ("flag west col (0,1)-(1,41)", 0, 1, 1, 41),
        ("flag up row (1,0)-(21,1)", 1, 0, 21, 1),
        ("flag down row (21,0)-(41,1)", 21, 0, 41, 1),
    ];
    for (name, x0, y0, x1, y1) in regions {
        let mut opaque = 0u32;
        let total = (x1 - x0) * (y1 - y0);
        let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
        for y in *y0..*y1 {
            for x in *x0..*x1 {
                let pixel = rgba.get_pixel(x, y);
                if pixel.0[3] > 8 {
                    opaque += 1;
                    r += u64::from(pixel.0[0]);
                    g += u64::from(pixel.0[1]);
                    b += u64::from(pixel.0[2]);
                }
            }
        }
        let (mean_r, mean_g, mean_b) = if opaque > 0 {
            (r / u64::from(opaque), g / u64::from(opaque), b / u64::from(opaque))
        } else {
            (0, 0, 0)
        };
        println!("{name}: opaque {opaque}/{total} mean rgb ({mean_r},{mean_g},{mean_b})");
    }
    Ok(())
}
