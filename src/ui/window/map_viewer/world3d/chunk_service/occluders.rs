//! Conservative proxies from certified opaque rectangles, never arbitrary mesh bounds.
use super::*;

#[cfg(test)]
fn cube_cell(face: &p3d::Preview3dStreamFace) -> Option<([i32; 3], u8)> {
    if face.pass != p3d::Preview3dStreamPass::Opaque {
        return None;
    }
    full_face(face.corners, face.normal, gpui_3d::FrontFace::Cw)
}

pub(super) fn face_proxy(
    face: &p3d::Preview3dStreamFace,
    origin: [i32; 3],
) -> Option<gpui_3d::Occluder> {
    if face.pass != p3d::Preview3dStreamPass::Opaque {
        return None;
    }
    let (min, max, axis) = quad_bounds(face.corners, face.normal, gpui_3d::FrontFace::Cw)?;
    let local = |p: [f64; 3]| {
        Vec3::new(
            (p[0] - f64::from(origin[0])) as f32,
            (p[1] - f64::from(origin[1])) as f32,
            (p[2] - f64::from(origin[2])) as f32,
        )
    };
    Some(gpui_3d::Occluder {
        bounds: gpui_3d::Aabb::new(local(min), local(max))?,
        faces: 1 << (axis * 2 + usize::from(face.normal[axis] > 0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streamed_clockwise_cube_faces_certify_all_six_occluder_sides() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut builder = p3d::Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(pos, p3d::Preview3dChunkBlocks::test_cubes(&[[3, 4, 5]]));
        let faces = builder.build_stream_faces(&[pos]).remove(0).faces;
        assert_eq!(faces.len(), 6);
        let cells = faces
            .iter()
            .map(|face| cube_cell(face).expect("real streamed opaque face must certify"))
            .collect::<Vec<_>>();
        assert!(cells.iter().all(|(cell, _)| *cell == [3, 4, 5]));
        assert_eq!(cells.iter().fold(0, |mask, (_, side)| mask | side), 63);
        let proxies = boxes(cells.iter().copied(), [0; 3]);
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].faces, 63);
        let mut bowtie = faces[0].clone();
        bowtie.corners.swap(2, 3);
        assert!(
            cube_cell(&bowtie).is_none(),
            "crossed quads must never certify a solid occluder"
        );
        let mut reverse = faces[0].clone();
        reverse.corners.reverse();
        assert!(
            cube_cell(&reverse).is_none(),
            "certification must agree with material front-face winding"
        );
    }
    #[test]
    fn planar_proxy_merging_preserves_roof_holes_and_outer_face_masks() {
        let roof = || (0..16).flat_map(|x| (0..16).map(move |z| ([x, 0, z], 8)));
        let merged = boxes(roof(), [0; 3]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].bounds.min, Vec3::ZERO);
        assert_eq!(merged[0].bounds.max, Vec3::new(16., 1., 16.));
        assert_eq!(merged[0].faces, 8);
        let hole = boxes(roof().filter(|(cell, _)| *cell != [8, 0, 8]), [0; 3]);
        assert!(hole.iter().all(|b| !(b.bounds.min.x < 8.5
            && b.bounds.max.x > 8.5
            && b.bounds.min.z < 8.5
            && b.bounds.max.z > 8.5)));
        let line = boxes([([0, 0, 0], 1 | 8), ([1, 0, 0], 2 | 8)], [0; 3]);
        assert_eq!(line.len(), 1);
        assert_eq!(line[0].faces, 1 | 2 | 8);
        assert_eq!(line[0].bounds.max.x, 2.);
    }
    #[test]
    fn merged_surface_faces_occlude_without_claiming_hidden_faces() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut builder = p3d::Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        let cells = (3..6)
            .flat_map(|x| (3..6).map(move |z| [x, 4, z]))
            .collect::<Vec<_>>();
        builder.insert_stream_chunk(pos, p3d::Preview3dChunkBlocks::test_cubes(&cells));
        let faces = builder.build_stream_faces(&[pos]).remove(0).faces;
        let proxies = merge_boxes(
            faces
                .iter()
                .filter_map(|face| face_proxy(face, [0; 3]))
                .collect(),
        );
        let proxy = proxies.iter().find(|proxy| proxy.faces == 8).unwrap();
        assert_eq!(proxy.faces, 8);
        assert_eq!(proxy.bounds.min, Vec3::new(3., 5., 3.));
        assert_eq!(proxy.bounds.max, Vec3::new(6., 5., 6.));
    }
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
        let clockwise = templates
            .iter()
            .cloned()
            .map(|mut face| {
                face.vertices.reverse();
                face
            })
            .collect::<Vec<_>>();
        let mut clockwise_material = Material::new();
        clockwise_material.front_face = gpui_3d::FrontFace::Cw;
        assert_eq!(
            gpu_boxes(&band, &clockwise, &[Arc::new(clockwise_material)]).len(),
            1
        );
        assert!(gpu_boxes(&band, &clockwise, &[material.clone()]).is_empty());
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

fn full_face(
    corners: [[f64; 3]; 4],
    normal: [i32; 3],
    front_face: gpui_3d::FrontFace,
) -> Option<([i32; 3], u8)> {
    let (min, max, axis) = quad_bounds(corners, normal, front_face)?;
    if (0..3).any(|a| min[a].fract() != 0. || max[a] - min[a] != if a == axis { 0. } else { 1. }) {
        return None;
    }
    Some((
        std::array::from_fn(|a| min[a] as i32 - if normal[a] > 0 { 1 } else { 0 }),
        1 << (axis * 2 + usize::from(normal[axis] > 0)),
    ))
}

fn quad_bounds(
    corners: [[f64; 3]; 4],
    normal: [i32; 3],
    front_face: gpui_3d::FrontFace,
) -> Option<([f64; 3], [f64; 3], usize)> {
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
        || (0..3).any(|a| {
            !min[a].is_finite()
                || !max[a].is_finite()
                || if a == axis {
                    max[a] != min[a]
                } else {
                    max[a] <= min[a]
                }
        })
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
    let sign = if front_face == gpui_3d::FrontFace::Cw {
        -1.
    } else {
        1.
    };
    for triangle in [(1, 2), (2, 3)] {
        let a = std::array::from_fn::<_, 3, _>(|i| corners[triangle.0][i] - corners[0][i]);
        let b = std::array::from_fn::<_, 3, _>(|i| corners[triangle.1][i] - corners[0][i]);
        let cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        if sign * (0..3).map(|i| cross[i] * f64::from(normal[i])).sum::<f64>() <= 0. {
            return None;
        }
    }
    Some((min, max, axis))
}

pub(super) fn boxes(
    cells: impl IntoIterator<Item = ([i32; 3], u8)>,
    origin: [i32; 3],
) -> Arc<[gpui_3d::Occluder]> {
    let mut columns = BTreeMap::<(i32, i32), BTreeMap<i32, u8>>::new();
    for ([x, y, z], faces) in cells {
        columns
            .entry((x, z))
            .or_default()
            .entry(y)
            .and_modify(|current| *current |= faces)
            .or_insert(faces);
    }
    let mut result = Vec::new();
    for ((x, z), ys) in columns {
        let mut values = ys.into_iter().peekable();
        while let Some((start, bottom)) = values.next() {
            let sides = bottom & 51;
            let mut end = start + 1;
            let mut top = bottom;
            while values
                .peek()
                .is_some_and(|(y, face)| *y == end && face & 51 == sides)
            {
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
    merge_boxes(result)
}

/// Adjacent solid boxes may merge only when their other face coverage agrees. Never bridge gaps.
pub(super) fn merge_boxes(mut proxies: Vec<gpui_3d::Occluder>) -> Arc<[gpui_3d::Occluder]> {
    for axis in [0, 2, 1] {
        let axis_faces = 3 << (axis * 2);
        let components = |v: Vec3| [v.x, v.y, v.z];
        let key = |proxy: &gpui_3d::Occluder| {
            let min = components(proxy.bounds.min);
            let max = components(proxy.bounds.max);
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            [
                min[a].to_bits(),
                max[a].to_bits(),
                min[b].to_bits(),
                max[b].to_bits(),
                u32::from(proxy.faces & !axis_faces),
            ]
        };
        proxies.sort_unstable_by(|a, b| {
            key(a).cmp(&key(b)).then_with(|| {
                components(a.bounds.min)[axis].total_cmp(&components(b.bounds.min)[axis])
            })
        });
        let mut merged: Vec<gpui_3d::Occluder> = Vec::with_capacity(proxies.len());
        for proxy in proxies {
            if let Some(last) = merged.last_mut()
                && key(last) == key(&proxy)
                && components(last.bounds.max)[axis] == components(proxy.bounds.min)[axis]
            {
                let mut max = components(last.bounds.max);
                max[axis] = components(proxy.bounds.max)[axis];
                last.bounds.max = Vec3::new(max[0], max[1], max[2]);
                last.faces = (last.faces & !axis_faces)
                    | (last.faces & (1 << (axis * 2)))
                    | (proxy.faces & (2 << (axis * 2)));
            } else {
                merged.push(proxy);
            }
        }
        proxies = merged;
    }
    proxies.retain(|proxy| proxy.faces != 0);
    proxies.into()
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
            let material = materials.get(face.material_slot as usize)?;
            if material.alpha_mode != AlphaMode::Opaque {
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
                material.front_face,
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
