//! Conservative solid proxies from full opaque cube geometry, never mesh bounds.
use super::*;

pub(super) fn cube_cell(face: &p3d::Preview3dStreamFace) -> Option<([i32; 3], u8)> {
    if face.pass != p3d::Preview3dStreamPass::Opaque {
        return None;
    }
    full_face(face.corners, face.normal)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_closed_opaque_unit_cube_templates_certify_occlusion() {
        let templates = Mesh::cube()
            .vertices()
            .chunks_exact(4)
            .map(|face| gpui_3d::VoxelFaceTemplate {
                vertices: std::array::from_fn(|i| {
                    let mut v = face[i];
                    v.position = v.position + Vec3::new(0.5, 0.5, 0.5);
                    v
                }),
                material_slot: 0,
                neighbour: [
                    face[0].normal.x as i32,
                    face[0].normal.y as i32,
                    face[0].normal.z as i32,
                ],
            })
            .collect::<Vec<_>>();
        let mut classes = vec![0; 27];
        classes[13] = 16;
        let mut band = p3d::GpuCubeBand {
            size: [3; 3],
            origin: Vec3::new(-1., -1., -1.),
            cells: vec![0; 27],
            faces: vec![],
            model_faces: 0,
            template_faces: 6,
            classes,
            neighbours: vec![],
            instances: vec![gpui_3d::VoxelMeshInstance {
                position: [1; 3],
                faces: 0..6,
                height: 1.,
                occlusion_mask: 1,
            }],
            section_y: 0,
        };
        let material = Arc::new(Material::new());
        assert_eq!(gpu_boxes(&band, &templates, &[material.clone()]).len(), 1);
        band.classes[14] = 1;
        assert_eq!(
            gpu_boxes(&band, &templates, &[material.clone()])[0].faces,
            63 & !2
        );
        band.classes[14] = 0;
        band.instances[0].faces = 0..1;
        assert!(gpu_boxes(&band, &templates, &[material.clone()]).is_empty());
        band.instances[0].faces = 0..6;
        let mut masked = Material::new();
        masked.alpha_mode = AlphaMode::Mask;
        assert!(gpu_boxes(&band, &templates, &[Arc::new(masked)]).is_empty());
        band.instances[0].height = 0.5;
        assert!(gpu_boxes(&band, &templates, &[material]).is_empty());
        let merged = boxes([([0, 0, 0], 1 | 8), ([0, 1, 0], 1 | 4)], [0; 3]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].faces, 1);
    }
}

fn full_face(corners: [[f64; 3]; 4], normal: [i32; 3]) -> Option<([i32; 3], u8)> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in corners {
        for a in 0..3 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    let axis = normal.iter().position(|&n| n != 0)?;
    if normal.iter().map(|n| n.abs()).sum::<i32>() != 1
        || (0..3)
            .any(|a| min[a].fract() != 0. || max[a] - min[a] != if a == axis { 0. } else { 1. })
    {
        return None;
    }
    if corners
        .iter()
        .enumerate()
        .any(|(i, p)| corners[..i].contains(p) || (0..3).any(|a| p[a] != min[a] && p[a] != max[a]))
    {
        return None;
    }
    let a = std::array::from_fn::<_, 3, _>(|i| corners[1][i] - corners[0][i]);
    let b = std::array::from_fn::<_, 3, _>(|i| corners[2][i] - corners[0][i]);
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    if (0..3).map(|i| cross[i] * f64::from(normal[i])).sum::<f64>() <= 0.0 {
        return None;
    }
    Some((
        std::array::from_fn(|a| min[a] as i32 - if normal[a] > 0 { 1 } else { 0 }),
        1 << (axis * 2 + usize::from(normal[axis] > 0)),
    ))
}

pub(super) fn boxes(
    cells: impl IntoIterator<Item = ([i32; 3], u8)>,
    origin: [i32; 3],
) -> Arc<[gpui_3d::Occluder]> {
    let mut columns = BTreeMap::<(i32, i32, u8), BTreeMap<i32, u8>>::new();
    for ([x, y, z], faces) in cells {
        columns
            .entry((x, z, faces & 51))
            .or_default()
            .insert(y, faces);
    }
    let mut result = Vec::new();
    for ((x, z, sides), ys) in columns {
        let mut values = ys.into_iter().peekable();
        while let Some((start, bottom)) = values.next() {
            let mut end = start + 1;
            let mut top = bottom;
            while values.peek().is_some_and(|(y, _)| *y == end) {
                if let Some((_, face)) = values.next() {
                    top = face;
                }
                end += 1;
            }
            let min = Vec3::new(
                (x - origin[0]) as f32,
                (start - origin[1]) as f32,
                (z - origin[2]) as f32,
            );
            let max = Vec3::new(min.x + 1., (end - origin[1]) as f32, min.z + 1.);
            let faces = sides | (bottom & 4) | (top & 8);
            if faces != 0
                && let Some(bounds) = gpui_3d::Aabb::new(min, max)
            {
                result.push(gpui_3d::Occluder { bounds, faces });
            }
        }
    }
    result.into()
}

pub(super) fn gpu_boxes(
    band: &p3d::GpuCubeBand,
    templates: &[gpui_3d::VoxelFaceTemplate],
    materials: &[Arc<Material>],
) -> Arc<[gpui_3d::Occluder]> {
    let cells = band.instances.iter().filter_map(|instance| {
        let [x, y, z] = instance.position;
        let index = ((z * band.size[1] + y) * band.size[0] + x) as usize;
        if band.classes[index] & 16 == 0 || instance.height != 1.0 {
            return None;
        }
        let begin = band.model_faces + instance.faces.start as usize;
        let end = band.model_faces + instance.faces.end as usize;
        let mut sides = 0;
        let mut rendered = 0;
        for face in templates.get(begin..end)? {
            if materials.get(face.material_slot as usize)?.alpha_mode != AlphaMode::Opaque {
                return None;
            }
            let (cell, side) = full_face(
                face.vertices.map(|v| {
                    [
                        v.position.x as f64,
                        v.position.y as f64,
                        v.position.z as f64,
                    ]
                }),
                face.neighbour,
            )?;
            if cell != [0; 3] {
                return None;
            }
            sides |= side;
            let neighbor = std::array::from_fn::<_, 3, _>(|axis| {
                instance.position[axis].checked_add_signed(face.neighbour[axis])
            });
            let [Some(nx), Some(ny), Some(nz)] = neighbor else {
                return None;
            };
            if nx >= band.size[0] || ny >= band.size[1] || nz >= band.size[2] {
                return None;
            }
            let class = band.classes[((nz * band.size[1] + ny) * band.size[0] + nx) as usize];
            let height = band.origin.y
                + y as f32
                + face.vertices.iter().map(|v| v.position.y).sum::<f32>() * 0.25;
            if class & instance.occlusion_mask == 0
                && height >= (band.section_y * 64) as f32
                && height < ((band.section_y + 1) * 64) as f32
            {
                rendered |= side;
            }
        }
        if sides != 63 || rendered == 0 {
            return None;
        }
        let p = [
            band.origin.x as i32 + x as i32,
            band.origin.y as i32 + y as i32,
            band.origin.z as i32 + z as i32,
        ];
        ((0..16).contains(&p[0]) && (0..16).contains(&p[2])).then_some((p, rendered))
    });
    boxes(cells, [0; 3])
}
