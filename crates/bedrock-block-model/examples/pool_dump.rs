//! Dump raw capture geometry entries by pool id, for families the catalog skips.
//!
//! `pool_dump <export-root> <id>...` prints one entry's bounds and vertices.
//! `pool_dump --quads <export-root> <id>...` groups the entry's triangles into the planar quads
//! they came from, which is how a family rebuilt from capture data recovers authored box faces
//! and single-sided cards instead of guessing from a vertex cloud.
use std::path::Path;

use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let quads = args.first().map(String::as_str) == Some("--quads");
    if quads {
        args.remove(0);
    }
    let Some(root) = args.first().cloned() else {
        std::process::exit(2);
    };
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    for id in &args[1..] {
        let id: u32 = id.parse()?;
        let Some(vertices) = catalog.debug_geometry_vertices(id) else {
            println!("id={id} <none>");
            continue;
        };
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &vertices {
            for a in 0..3 {
                min[a] = min[a].min(v[a]);
                max[a] = max[a].max(v[a]);
            }
        }
        println!(
            "id={id} verts={} bounds=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
            vertices.len(),
            min[0],
            min[1],
            min[2],
            max[0],
            max[1],
            max[2]
        );
        if quads {
            for quad in group_quads(&catalog.debug_geometry_triangles(id).unwrap_or_default()) {
                println!("   quad {}", format_quad(&quad));
            }
        } else {
            for v in vertices.iter().take(16) {
                println!("   {:.4} {:.4} {:.4}", v[0], v[1], v[2]);
            }
        }
    }
    Ok(())
}

/// Pair triangles that share three corners into one quad.
///
/// Captured geometry is a triangle soup, but every authored face is two coplanar triangles with
/// four distinct corners, so a shared edge identifies the pair.
fn group_quads(triangles: &[[[f32; 3]; 3]]) -> Vec<[[f32; 3]; 4]> {
    let mut remaining: Vec<[[f32; 3]; 3]> = triangles.to_vec();
    let mut quads = Vec::new();
    while let Some(triangle) = remaining.pop() {
        let shared = remaining.iter().position(|other| {
            triangle
                .iter()
                .filter(|corner| other.iter().any(|point| points_match(point, corner)))
                .count()
                == 2
        });
        let Some(index) = shared else {
            continue;
        };
        let other = remaining.remove(index);
        let mut corners: Vec<[f32; 3]> = Vec::with_capacity(4);
        for corner in triangle.iter().chain(other.iter()) {
            if !corners.iter().any(|point| points_match(point, corner)) {
                corners.push(*corner);
            }
        }
        if corners.len() != 4 {
            continue;
        }
        let mut corners: [[f32; 3]; 4] = [corners[0], corners[1], corners[2], corners[3]];
        // Order the four corners around their centre so the quad reads as a ring.
        let centre = corners.iter().fold([0.0_f32; 3], |mut sum, corner| {
            for axis in 0..3 {
                sum[axis] += corner[axis] / 4.0;
            }
            sum
        });
        let normal = quad_normal(&corners);
        let basis_u = normalize(sub(corners[0], centre));
        let basis_v = cross(normal, basis_u);
        corners.sort_by(|left, right| {
            let angle = |corner: &[f32; 3]| {
                let offset = sub(*corner, centre);
                dot(offset, basis_v).atan2(dot(offset, basis_u))
            };
            angle(left)
                .partial_cmp(&angle(right))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        quads.push([corners[0], corners[1], corners[2], corners[3]]);
    }
    quads
}

fn quad_normal(corners: &[[f32; 3]; 4]) -> [f32; 3] {
    let normal = cross(sub(corners[1], corners[0]), sub(corners[2], corners[0]));
    normalize(normal)
}

fn points_match(left: &[f32; 3], right: &[f32; 3]) -> bool {
    (0..3).all(|axis| (left[axis] - right[axis]).abs() < 1e-4)
}

fn sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt();
    if length < 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    }
}

fn format_quad(quad: &[[f32; 3]; 4]) -> String {
    quad.iter()
        .map(|corner| format!("({:.4},{:.4},{:.4})", corner[0], corner[1], corner[2]))
        .collect::<Vec<_>>()
        .join(" ")
}
