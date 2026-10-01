//! Bounded OBJ input with material paths confined to the model directory.

use std::{
    cell::RefCell,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

const MAX_OBJ_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FACE_VERTICES: usize = 4096;
const MAX_TRIANGLES: usize = 2_000_000;

/// One OBJ vertex after `tobj` has resolved position and texture indices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjVertex {
    /// Model-space position. No Minecraft scale or orientation is applied.
    pub position: [f32; 3],
    /// Source texture coordinate when the face supplies one.
    pub uv: Option<[f32; 2]>,
}

/// A validated source triangle, preserving the OBJ material index.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjTriangle {
    /// Three vertices in source winding order.
    pub vertices: [ObjVertex; 3],
    /// Index into [`ObjModel::materials`], when an MTL material is assigned.
    pub material: Option<usize>,
}

/// Diffuse color, opacity, and a validated texture path from an OBJ material.
///
/// Opacity follows MTL `d`; when absent, the inverse of `Tr` is used. Materials
/// without either field are opaque. Diffuse texture alpha remains separate and
/// is multiplied by this opacity during voxelization.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjMaterial {
    /// Diffuse color supplied by MTL, when present.
    pub diffuse: Option<[f32; 3]>,
    /// Resolved MTL opacity in the inclusive range `0.0..=1.0`.
    pub opacity: f32,
    /// Canonical path within the model directory, when present.
    pub diffuse_texture: Option<PathBuf>,
}

/// Source geometry for subsequent preview, sampling, and voxelization.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjModel {
    /// Triangles from all source objects, in source object order.
    pub triangles: Vec<ObjTriangle>,
    /// Materials loaded from MTL files inside the model directory.
    pub materials: Vec<ObjMaterial>,
}

/// Parse a local OBJ with ear clipping for simple concave polygon faces.
///
/// The OBJ, each referenced MTL, and each declared texture must resolve within
/// the canonical model directory. Symlink escapes and malformed, intersecting,
/// non-planar, or degenerate faces are rejected. This only reads model files;
/// it does not access or modify a Minecraft world. Limits bound individual OBJ
/// size, polygon arity, and output triangle count; call this on the CPU worker.
///
/// # Errors
/// Returns an error for I/O failures, escaping paths, unsupported geometry,
/// missing textures, or an input exceeding the stated limits.
pub fn load_obj_model(path: &Path) -> Result<ObjModel, String> {
    let source = path.canonicalize().map_err(|error| error.to_string())?;
    let root = source
        .parent()
        .ok_or_else(|| "OBJ has no model directory".to_owned())?;
    if source.metadata().map_err(|error| error.to_string())?.len() > MAX_OBJ_BYTES {
        return Err("OBJ exceeds 256 MiB input limit".to_owned());
    }

    let reader = File::open(&source).map_err(|error| error.to_string())?;
    let path_error = RefCell::new(None);
    let options = tobj::LoadOptions {
        triangulate: false,
        single_index: true,
        ignore_points: true,
        ignore_lines: true,
    };
    let parsed = tobj::load_obj_buf(&mut BufReader::new(reader), &options, |relative| {
        let material_path = match confined_path(root, relative) {
            Ok(path) => path,
            Err(error) => {
                *path_error.borrow_mut() = Some(error);
                return Err(tobj::LoadError::OpenFileFailed);
            }
        };
        match File::open(material_path) {
            Ok(file) => tobj::load_mtl_buf(&mut BufReader::new(file)),
            Err(error) => {
                *path_error.borrow_mut() = Some(error.to_string());
                Err(tobj::LoadError::OpenFileFailed)
            }
        }
    });
    if let Some(error) = path_error.into_inner() {
        return Err(error);
    }
    let (meshes, materials) = parsed.map_err(|error| error.to_string())?;
    let materials = materials.map_err(|error| error.to_string())?;
    let materials = materials
        .into_iter()
        .map(|material| {
            if material
                .unknown_param
                .keys()
                .any(|key| key.starts_with("map_") || key == "bump" || key == "disp")
            {
                return Err("MTL contains an unsupported texture reference".to_owned());
            }
            for texture in [
                &material.ambient_texture,
                &material.diffuse_texture,
                &material.specular_texture,
                &material.normal_texture,
                &material.shininess_texture,
                &material.dissolve_texture,
            ] {
                if let Some(texture) = texture {
                    confined_path(root, Path::new(texture))?;
                }
            }
            let diffuse_texture = material
                .diffuse_texture
                .as_deref()
                .map(|texture| confined_path(root, Path::new(texture)))
                .transpose()?;
            Ok(ObjMaterial {
                diffuse: material.diffuse,
                opacity: mtl_opacity(&material)?,
                diffuse_texture,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut triangles = Vec::new();
    for model in meshes {
        let mesh = model.mesh;
        let triangle_only = mesh.face_arities.is_empty();
        if triangle_only && mesh.indices.len() % 3 != 0 {
            return Err("OBJ triangle indices are incomplete".to_owned());
        }
        let face_count = if triangle_only {
            mesh.indices.len() / 3
        } else {
            mesh.face_arities.len()
        };
        let mut offset = 0_usize;
        for face_index in 0..face_count {
            let arity = if triangle_only {
                3
            } else {
                mesh.face_arities[face_index] as usize
            };
            if !(3..=MAX_FACE_VERTICES).contains(&arity) {
                return Err("OBJ face has unsupported vertex count".to_owned());
            }
            let end = offset
                .checked_add(arity)
                .ok_or_else(|| "OBJ face index overflow".to_owned())?;
            let indices = mesh
                .indices
                .get(offset..end)
                .ok_or_else(|| "OBJ face indices are incomplete".to_owned())?;
            let face = indices
                .iter()
                .map(|&index| vertex(&mesh, index as usize))
                .collect::<Result<Vec<_>, _>>()?;
            let face_triangles = triangulate(&face)?;
            if triangles.len() + face_triangles.len() > MAX_TRIANGLES {
                return Err("OBJ exceeds two million triangles".to_owned());
            }
            triangles.extend(face_triangles.into_iter().map(|vertices| ObjTriangle {
                vertices,
                material: mesh.material_id,
            }));
            offset = end;
        }
        if offset != mesh.indices.len() {
            return Err("OBJ face arities do not cover all indices".to_owned());
        }
    }
    if triangles.is_empty() {
        return Err("OBJ contains no valid faces".to_owned());
    }
    Ok(ObjModel {
        triangles,
        materials,
    })
}

fn mtl_opacity(material: &tobj::Material) -> Result<f32, String> {
    let dissolve = material.dissolve;
    let transparency = material
        .unknown_param
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("Tr"))
        .map(|(_, value)| {
            value
                .parse::<f32>()
                .map(|transparency| 1.0 - transparency)
                .map_err(|_| "MTL Tr must be a number".to_owned())
        })
        .transpose()?;
    let opacity = match (dissolve, transparency) {
        (Some(dissolve), Some(transparency)) if (dissolve - transparency).abs() > 0.0001 => {
            return Err("MTL d and Tr specify conflicting opacity".to_owned());
        }
        (Some(dissolve), _) => dissolve,
        (None, Some(transparency)) => transparency,
        (None, None) => 1.0,
    };
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
        return Err("MTL opacity must be between 0 and 1".to_owned());
    }
    Ok(opacity)
}

fn confined_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let resolved = root.join(relative).canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "model references a missing material or texture: {}",
                relative.display()
            )
        } else {
            format!("cannot read model asset {}: {error}", relative.display())
        }
    })?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "model asset escapes its directory: {}",
            relative.display()
        ));
    }
    Ok(resolved)
}

fn vertex(mesh: &tobj::Mesh, index: usize) -> Result<ObjVertex, String> {
    let start = index
        .checked_mul(3)
        .ok_or_else(|| "OBJ vertex index overflow".to_owned())?;
    let position: [f32; 3] = mesh
        .positions
        .get(start..start + 3)
        .ok_or_else(|| "OBJ vertex position is missing".to_owned())?
        .try_into()
        .map_err(|_| "OBJ vertex position is incomplete".to_owned())?;
    if !position.iter().all(|value| value.is_finite()) {
        return Err("OBJ vertex position is not finite".to_owned());
    }
    let uv = if mesh.texcoords.is_empty() {
        None
    } else {
        let start = index
            .checked_mul(2)
            .ok_or_else(|| "OBJ texture index overflow".to_owned())?;
        let uv: [f32; 2] = mesh
            .texcoords
            .get(start..start + 2)
            .ok_or_else(|| "OBJ texture coordinate is missing".to_owned())?
            .try_into()
            .map_err(|_| "OBJ texture coordinate is incomplete".to_owned())?;
        if !uv.iter().all(|value| value.is_finite()) {
            return Err("OBJ texture coordinate is not finite".to_owned());
        }
        Some(uv)
    };
    Ok(ObjVertex { position, uv })
}

fn triangulate(face: &[ObjVertex]) -> Result<Vec<[ObjVertex; 3]>, String> {
    let n = face.len();
    let mut normal = [0.0_f64; 3];
    for (current, next) in face.iter().zip(face.iter().cycle().skip(1)).take(n) {
        let a = current.position.map(f64::from);
        let b = next.position.map(f64::from);
        normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
        normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
        normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let axis = (0..3)
        .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .ok_or_else(|| "OBJ face has no normal".to_owned())?;
    let projected = face
        .iter()
        .map(|vertex| {
            let point = vertex.position;
            [
                f64::from(point[(axis + 1) % 3]),
                f64::from(point[(axis + 2) % 3]),
            ]
        })
        .collect::<Vec<_>>();
    let (minimum, maximum) = projected.iter().fold(
        ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
        |(minimum, maximum), point| {
            (
                [minimum[0].min(point[0]), minimum[1].min(point[1])],
                [maximum[0].max(point[0]), maximum[1].max(point[1])],
            )
        },
    );
    let width = (maximum[0] - minimum[0]).max(maximum[1] - minimum[1]);
    let epsilon = width * width * 1e-12;
    let signed_area = (0..n)
        .map(|i| cross([0.0, 0.0], projected[i], projected[(i + 1) % n]))
        .sum::<f64>();
    if signed_area.abs() <= epsilon {
        return Err("OBJ face is degenerate".to_owned());
    }
    let winding = signed_area.signum();
    let normal_length = normal.iter().map(|value| value * value).sum::<f64>().sqrt();
    let origin = face[0].position.map(f64::from);
    for point in face.iter().skip(1) {
        let distance = (0..3)
            .map(|axis| (f64::from(point.position[axis]) - origin[axis]) * normal[axis])
            .sum::<f64>()
            / normal_length;
        if distance.abs() > width.max(1.0) * 1e-5 {
            return Err("OBJ face is not planar".to_owned());
        }
    }
    for a in 0..n {
        for b in (a + 2)..n {
            if a == 0 && b == n - 1 {
                continue;
            }
            if segments_cross(
                projected[a],
                projected[(a + 1) % n],
                projected[b],
                projected[(b + 1) % n],
                epsilon,
            ) {
                return Err("OBJ face intersects itself".to_owned());
            }
        }
    }
    let mut remaining = (0..n).collect::<Vec<_>>();
    let mut output = Vec::with_capacity(n - 2);
    while remaining.len() > 3 {
        let mut ear = None;
        for i in 0..remaining.len() {
            let prev = remaining[(i + remaining.len() - 1) % remaining.len()];
            let current = remaining[i];
            let next = remaining[(i + 1) % remaining.len()];
            if cross(projected[prev], projected[current], projected[next]) * winding <= epsilon {
                continue;
            }
            if remaining.iter().copied().any(|point| {
                point != prev
                    && point != current
                    && point != next
                    && inside_triangle(
                        projected[point],
                        projected[prev],
                        projected[current],
                        projected[next],
                        winding,
                        epsilon,
                    )
            }) {
                continue;
            }
            ear = Some((i, [face[prev], face[current], face[next]]));
            break;
        }
        let Some((index, triangle)) = ear else {
            return Err("OBJ face cannot be triangulated".to_owned());
        };
        output.push(triangle);
        remaining.remove(index);
    }
    let [a, b, c] = [remaining[0], remaining[1], remaining[2]];
    if cross(projected[a], projected[b], projected[c]) * winding <= epsilon {
        return Err("OBJ face has degenerate final triangle".to_owned());
    }
    output.push([face[a], face[b], face[c]]);
    Ok(output)
}

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn inside_triangle(
    point: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    winding: f64,
    epsilon: f64,
) -> bool {
    cross(a, b, point) * winding >= -epsilon
        && cross(b, c, point) * winding >= -epsilon
        && cross(c, a, point) * winding >= -epsilon
}

fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], eps: f64) -> bool {
    let ac = cross(a, b, c);
    let ad = cross(a, b, d);
    let ca = cross(c, d, a);
    let cb = cross(c, d, b);
    ((ac > eps && ad < -eps) || (ac < -eps && ad > eps))
        && ((ca > eps && cb < -eps) || (ca < -eps && cb > eps))
        || (ac.abs() <= eps && on_segment(a, b, c))
        || (ad.abs() <= eps && on_segment(a, b, d))
        || (ca.abs() <= eps && on_segment(c, d, a))
        || (cb.abs() <= eps && on_segment(c, d, b))
}

fn on_segment(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> bool {
    (0..2).all(|axis| point[axis] >= a[axis].min(b[axis]) && point[axis] <= a[axis].max(b[axis]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    fn fixture_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bmcbl-obj-test-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn vertex_at(x: f32, y: f32) -> ObjVertex {
        ObjVertex {
            position: [x, y, 0.0],
            uv: Some([x, y]),
        }
    }

    #[test]
    fn concave_face_is_triangulated_without_filling_notch() {
        let face = [
            vertex_at(0.0, 0.0),
            vertex_at(3.0, 0.0),
            vertex_at(3.0, 3.0),
            vertex_at(1.5, 1.0),
            vertex_at(0.0, 3.0),
        ];
        let triangles = triangulate(&face).unwrap();
        assert_eq!(triangles.len(), 3);
        let area = triangles
            .iter()
            .map(|triangle| {
                cross(
                    triangle[0].uv.unwrap().map(f64::from),
                    triangle[1].uv.unwrap().map(f64::from),
                    triangle[2].uv.unwrap().map(f64::from),
                )
                .abs()
                    / 2.0
            })
            .sum::<f64>();
        assert!((area - 6.0).abs() < 1e-6);
    }

    #[test]
    fn crossing_face_is_rejected() {
        let face = [
            vertex_at(0.0, 0.0),
            vertex_at(2.0, 2.0),
            vertex_at(0.0, 2.0),
            vertex_at(2.0, 0.0),
        ];
        assert!(triangulate(&face).is_err());
    }

    #[test]
    fn nonplanar_face_is_rejected() {
        let mut face = [
            vertex_at(0.0, 0.0),
            vertex_at(1.0, 0.0),
            vertex_at(1.0, 1.0),
            vertex_at(0.0, 1.0),
        ];
        face[2].position[2] = 0.1;
        assert!(triangulate(&face).is_err());
    }

    #[test]
    fn obj_loader_preserves_uv_and_confines_assets() {
        let fixture = fixture_dir();
        let model_dir = fixture.join("model");
        fs::create_dir(&model_dir).unwrap();
        fs::write(model_dir.join("color.png"), b"texture bytes").unwrap();
        fs::write(
            model_dir.join("material.mtl"),
            "newmtl paint\nKd 1 0 0\nmap_Kd color.png\n",
        )
        .unwrap();
        let obj = model_dir.join("shape.obj");
        fs::write(
            &obj,
            "mtllib material.mtl\nv 0 0 0\nv 3 0 0\nv 3 3 0\nv 1.5 1 0\nv 0 3 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0.5 0.4\nvt 0 1\nusemtl paint\nf 1/1 2/2 3/3 4/4 5/5\n",
        )
        .unwrap();
        let model = load_obj_model(&obj).unwrap();
        assert_eq!(model.triangles.len(), 3);
        assert!(
            model
                .triangles
                .iter()
                .all(|triangle| triangle.material == Some(0))
        );
        assert!(
            model
                .triangles
                .iter()
                .all(|triangle| triangle.vertices.iter().all(|vertex| vertex.uv.is_some()))
        );
        assert_eq!(
            model.materials[0].diffuse_texture,
            Some(model_dir.join("color.png").canonicalize().unwrap())
        );

        fs::write(fixture.join("outside.mtl"), "newmtl paint\n").unwrap();
        fs::write(
            &obj,
            "mtllib ../outside.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        )
        .unwrap();
        assert!(load_obj_model(&obj).unwrap_err().contains("escapes"));

        fs::write(
            model_dir.join("material.mtl"),
            "newmtl paint\nmap_Kd ../outside.png\n",
        )
        .unwrap();
        fs::write(fixture.join("outside.png"), b"texture bytes").unwrap();
        fs::write(
            &obj,
            "mtllib material.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl paint\nf 1 2 3\n",
        )
        .unwrap();
        assert!(load_obj_model(&obj).unwrap_err().contains("escapes"));

        fs::write(
            &obj,
            "mtllib result.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl \nf 1 2 3\n",
        )
        .unwrap();
        assert!(
            load_obj_model(&obj)
                .unwrap_err()
                .contains("missing material or texture: result.mtl")
        );
        fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn triangle_only_obj_without_face_arities_loads_materials() {
        let fixture = fixture_dir();
        fs::write(
            fixture.join("material.mtl"),
            "newmtl paint\nKd 1 0 0\nd 0.75\n",
        )
        .unwrap();
        let obj = fixture.join("triangles.obj");
        fs::write(
            &obj,
            "mtllib material.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl paint\nf 1/1 2/2 3/3\n",
        )
        .unwrap();
        let model = load_obj_model(&obj).expect("triangles");
        assert_eq!(model.triangles.len(), 1);
        assert_eq!(model.triangles[0].material, Some(0));
        assert_eq!(model.materials[0].diffuse, Some([1.0, 0.0, 0.0]));
        assert_eq!(model.materials[0].opacity, 0.75);

        fs::write(fixture.join("material.mtl"), "newmtl paint\nTr 0.25\n").unwrap();
        let model = load_obj_model(&obj).expect("transparency material");
        assert_eq!(model.materials[0].opacity, 0.75);

        fs::write(
            fixture.join("material.mtl"),
            "newmtl paint\nd 0.75\nTr 0.5\n",
        )
        .unwrap();
        assert!(
            load_obj_model(&obj)
                .unwrap_err()
                .contains("conflicting opacity")
        );
        fs::remove_dir_all(fixture).unwrap();
    }
}
