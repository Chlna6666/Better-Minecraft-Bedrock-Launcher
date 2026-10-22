//! Model interchange export for map conversion and exact-chunk selections.
//! OBJ with MTL/PNG sidecars, self-contained glTF, and binary GLB all use the
//! same already-decoded tobj/glTF raster input; no second file parser.
//! Heavy serialization runs on TaskManager workers, not GPUI Paint.

use std::{collections::HashMap, io::Cursor, path::Path, sync::Arc};

use base64::Engine as _;
use bedrock_voxel::{ObjModel, SourceTexture};
use bedrock_world::surface::CancelFlag;
use serde_json::{Value, json};

use super::model_import::prepare_model_for_export;

/// Exports an existing model source as Wavefront OBJ/MTL, glTF or GLB.
/// The caller must run this off the UI thread.
///
/// # Errors
/// Returns a descriptive error on malformed output formats, failed conversion
/// or cancellation. Writes an atomic single output for glTF / GLB.
pub(super) fn export_model_path(
    source: &Path,
    output: &Path,
    cancel: &CancelFlag,
) -> Result<(), String> {
    let input = prepare_model_for_export(source)?;
    if cancel.is_cancelled() { return Err("模型导出已取消".into()); }
    write_model(&input.model, &input.decoded, output, cancel)
}

/// Write already-imported triangle geometry to a supported model interchange
/// format. This can also be used by world-selection exports after their block
/// mesh is converted to ObjModel; no additional source decode is needed.
pub(super) fn write_model(
    model: &ObjModel,
    decoded: &[Option<Arc<SourceTexture>>],
    output: &Path,
    cancel: &CancelFlag,
) -> Result<(), String> {
    if model.triangles.is_empty() { return Err("没有可导出的三角形".into()); }
    if decoded.len() != model.materials.len() {
        return Err("模型材质和贴图表数量不匹配".into());
    }
    if model.triangles.len() > 2_000_000 {
        return Err("模型超过 200 万三角形导出限制".into());
    }
    if cancel.is_cancelled() { return Err("模型导出已取消".into()); }
    let ext = output.extension().and_then(|s| s.to_str())
        .unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "obj" => write_wavefront(model, decoded, output, cancel),
        "gltf" | "glb" => write_gltf(model, decoded, output, ext == "glb", cancel),
        _ => Err("仅支持导出 .obj、.gltf、.glb；MTL 随 OBJ 输出".into()),
    }
}

fn texture_png(image: &SourceTexture) -> Result<Vec<u8>, String> {
    let rgba = image::RgbaImage::from_raw(
        image.width, image.height, image.pixels.to_vec(),
    ).ok_or_else(|| "模型贴图像素长度不正确".to_owned())?;
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(buf.into_inner())
}

fn write_wavefront(
    model: &ObjModel, decoded: &[Option<Arc<SourceTexture>>],
    output: &Path, cancel: &CancelFlag,
) -> Result<(), String> {
    let stem = output.file_stem().and_then(|s| s.to_str())
        .ok_or_else(|| "输出 OBJ 文件名不合法".to_owned())?;
    let root = output.parent().unwrap_or_else(|| Path::new("."));
    let mtl_name = format!("{stem}.mtl");
    let mut obj = String::from("# BMCBL map model export\n");
    obj.push_str(&format!("mtllib {mtl_name}\n"));
    obj.push_str("o bmcbl_export\n");
    let mut mtl = String::from("# BMCBL material library\n");
    for (i, material) in model.materials.iter().enumerate() {
        mtl.push_str(&format!("newmtl material_{i}\n"));
        let rgb = material.diffuse.unwrap_or([1.0; 3]);
        mtl.push_str(&format!("Kd {} {} {}\nd {}\n",rgb[0],rgb[1],rgb[2],material.opacity));
        if decoded[i].is_some() {
            mtl.push_str(&format!("map_Kd {stem}_material_{i}.png\n"));
        }
        mtl.push('\n');
    }
    let fallback = model.materials.len();
    mtl.push_str("newmtl material_default\nKd 1 1 1\nd 1\n");
    let mut grouped = vec![Vec::new(); fallback + 1];
    for (i, tri) in model.triangles.iter().enumerate() {
        let group = tri.material.filter(|&v| v < fallback).unwrap_or(fallback);
        grouped[group].push(i);
    }
    // OBJ geometry is emitted as triangle soup, avoiding per-frame or global
    // dedup caches; coordinate and UV conventions remain unchanged.
    let mut next_vertex = 1usize;
    for (group, triangles) in grouped.iter().enumerate() {
        if cancel.is_cancelled() { return Err("模型导出已取消".into()); }
        if triangles.is_empty() { continue; }
        if group < fallback { obj.push_str(&format!("usemtl material_{group}\n")); }
        else { obj.push_str("usemtl material_default\n"); }
        for &i in triangles {
            let tri = &model.triangles[i];
            for vertex in tri.vertices {
                let [x,y,z] = vertex.position;
                if ![x,y,z].iter().all(|x| x.is_finite()) {
                    return Err("模型包含非法顶点坐标".into());
                }
                obj.push_str(&format!("v {x} {y} {z}\n"));
            }
            for vertex in tri.vertices {
                let uv = vertex.uv.unwrap_or([0.0,0.0]);
                obj.push_str(&format!("vt {} {}\n",uv[0],uv[1]));
            }
            obj.push_str(&format!(
                "f {0}/{0} {1}/{1} {2}/{2}\n",
                next_vertex, next_vertex+1, next_vertex+2,
            ));
            next_vertex += 3;
        }
    }
    // Build all sidecars in a private temp directory, then publish on success.
    // A cancellation cannot expose half-written OBJ/MTL/PNG files.
    let tmp = root.join(format!(".bmcbl-export-{}-{}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos()));
    std::fs::create_dir(&tmp).map_err(|error| error.to_string())?;
    let work = (|| -> Result<(), String> {
        std::fs::write(tmp.join(output.file_name().ok_or("文件名无效")?), &obj)
            .map_err(|error| error.to_string())?;
        std::fs::write(tmp.join(&mtl_name), &mtl).map_err(|error| error.to_string())?;
        for (i, texture) in decoded.iter().enumerate() {
            if cancel.is_cancelled() { return Err("模型导出已取消".into()); }
            if let Some(image) = texture {
                std::fs::write(tmp.join(format!("{stem}_material_{i}.png")), texture_png(image)?)
                    .map_err(|error| error.to_string())?;
            }
        }
        if cancel.is_cancelled() { return Err("模型导出已取消".into()); }
        for entry in std::fs::read_dir(&tmp).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let target = root.join(entry.file_name());
            if target.exists() {
                std::fs::remove_file(&target).map_err(|error| error.to_string())?;
            }
            std::fs::rename(entry.path(), target).map_err(|error| error.to_string())?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(tmp);
    work
}

fn push_aligned_binary(buf: &mut Vec<u8>, data: &[u8]) -> (usize, usize) {
    while buf.len() % 4 != 0 { buf.push(0); }
    let begin = buf.len();
    buf.extend_from_slice(data);
    (begin, data.len())
}
fn push_vec3(buf: &mut Vec<u8>, vec: [f32;3]) {
    for v in vec { buf.extend_from_slice(&v.to_le_bytes()); }
}
fn push_vec2(buf: &mut Vec<u8>, vec: [f32;2]) {
    for v in vec { buf.extend_from_slice(&v.to_le_bytes()); }
}

fn write_gltf(
    model: &ObjModel, decoded: &[Option<Arc<SourceTexture>>],
    output: &Path, binary: bool, cancel: &CancelFlag,
) -> Result<(), String> {
    let mut buffer = Vec::new();
    let mut views = Vec::<Value>::new();
    let mut accessors = Vec::<Value>::new();
    let mut materials = Vec::<Value>::new();
    let mut textures = Vec::<Value>::new();
    let mut images = Vec::<Value>::new();
    let mut samplers = Vec::<Value>::new();
    let mut source_tex_ids = HashMap::<usize, usize>::new();
    let material_count = model.materials.len();
    // Texture assets may be referenced by more than one material.
    for (i, material) in model.materials.iter().enumerate() {
        if cancel.is_cancelled() { return Err("模型导出已取消".into()); }
        let mut pbr = json!({
            "baseColorFactor": [
                material.diffuse.unwrap_or([1.0;3])[0],
                material.diffuse.unwrap_or([1.0;3])[1],
                material.diffuse.unwrap_or([1.0;3])[2],
                material.opacity.clamp(0.0,1.0)
            ],
            "metallicFactor":0.0, "roughnessFactor":1.0
        });
        if let Some(texture) = decoded[i].as_ref() {
            let key = Arc::as_ptr(texture) as usize;
            let idx = if let Some(&idx) = source_tex_ids.get(&key) { idx } else {
                let png = texture_png(texture)?;
                let image_index = images.len();
                if binary {
                    let (offset,len) = push_aligned_binary(&mut buffer, &png);
                    let view = views.len();
                    views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":len}));
                    images.push(json!({"bufferView":view,"mimeType":"image/png"}));
                } else {
                    let uri = format!("data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(&png));
                    images.push(json!({"uri":uri}));
                }
                if samplers.is_empty() {
                    samplers.push(json!({"wrapS":10497,"wrapT":10497,"magFilter":9729,"minFilter":9729}));
                }
                let idx = textures.len();
                textures.push(json!({"sampler":0,"source":image_index}));
                source_tex_ids.insert(key,idx);
                idx
            };
            pbr["baseColorTexture"] = json!({"index":idx});
        }
        materials.push(json!({
            "name":format!("material_{i}"),"pbrMetallicRoughness":pbr,
            "doubleSided":true,"alphaMode":if material.opacity < 1.0
                || decoded[i].as_ref().is_some_and(|t|t.has_alpha) {"BLEND"} else {"OPAQUE"}
        }));
    }
    materials.push(json!({
        "name":"default","pbrMetallicRoughness":{"baseColorFactor":[1,1,1,1],"metallicFactor":0,"roughnessFactor":1},
        "doubleSided":true
    }));
    let mut groups = vec![Vec::new(); material_count+1];
    for (i,tri) in model.triangles.iter().enumerate() {
        groups[tri.material.filter(|&v| v<material_count).unwrap_or(material_count)].push(i);
    }
    let mut primitives = Vec::<Value>::new();
    for (material_index, group) in groups.iter().enumerate() {
        if group.is_empty() {continue;}
        if cancel.is_cancelled() {return Err("模型导出已取消".into());}
        let vertex_count = group.len().checked_mul(3).ok_or("模型顶点溢出")?;
        let mut position = Vec::with_capacity(vertex_count*12);
        let mut normals = Vec::with_capacity(vertex_count*12);
        let mut uvs = Vec::with_capacity(vertex_count*8);
        let mut min = [f32::INFINITY;3];
        let mut max = [f32::NEG_INFINITY;3];
        for &index in group {
            let tri = &model.triangles[index];
            let [a,b,c] = tri.vertices.map(|v|v.position);
            if a.into_iter().chain(b).chain(c).any(|v|!v.is_finite()) {
                return Err("模型包含非法顶点坐标".into());
            }
            let ab=[b[0]-a[0],b[1]-a[1],b[2]-a[2]];
            let ac=[c[0]-a[0],c[1]-a[1],c[2]-a[2]];
            let n=[ab[1]*ac[2]-ab[2]*ac[1],
                   ab[2]*ac[0]-ab[0]*ac[2],
                   ab[0]*ac[1]-ab[1]*ac[0]];
            let mag=(n[0]*n[0]+n[1]*n[1]+n[2]*n[2]).sqrt();
            let normal=if mag>1.0e-10 { n.map(|v|v/mag) } else {[0.,1.,0.]};
            for vert in tri.vertices {
                for c in 0..3 { min[c]=min[c].min(vert.position[c]);max[c]=max[c].max(vert.position[c]); }
                push_vec3(&mut position,vert.position);
                push_vec3(&mut normals,normal);
                // Canonical glTF UV origin is top-left in the renderer; OBJ is bottom-up.
                let uv=vert.uv.unwrap_or([0.,0.]);
                push_vec2(&mut uvs,[uv[0],1.-uv[1]]);
            }
        }
        let mut attr=serde_json::Map::new();
        for (semantic,data,components,minimum,maximum) in [
            ("POSITION",position, "VEC3", Some(min),Some(max)),
            ("NORMAL",normals,"VEC3",None,None),
            ("TEXCOORD_0",uvs,"VEC2",None,None)
        ] {
            let (offset,length)=push_aligned_binary(&mut buffer,&data);
            let view=views.len();
            views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":length,"target":34962}));
            let acc=accessors.len();
            let mut accessor=json!({"bufferView":view,"componentType":5126,"count":vertex_count,"type":components});
            if let (Some(minimum),Some(maximum))=(minimum,maximum) {
                accessor["min"]=json!(minimum); accessor["max"]=json!(maximum);
            }
            accessors.push(accessor);
            attr.insert(semantic.into(),json!(acc));
        }
        primitives.push(json!({"attributes":attr,"mode":4,"material":material_index}));
    }
    if primitives.is_empty() {return Err("模型不包含可输出的三角形".into());}
    let mut root=json!({
        "asset":{"version":"2.0","generator":"BMCBL GPUI-3D"},
        "scene":0, "scenes":[{"nodes":[0]}],
        "nodes":[{"mesh":0,"name":"model"}],
        "meshes":[{"primitives":primitives}],
        "materials":materials,
        "textures":textures, "images":images, "samplers":samplers,
        "accessors":accessors,"bufferViews":views,"buffers":[{"byteLength":buffer.len()}]
    });
    if binary {
        while buffer.len()%4!=0{buffer.push(0);}
        // Buffer byteLength is the number of actual binary bytes, including
        // trailing alignment padding allowed by GLB 2.0.
        root["buffers"][0]["byteLength"]=json!(buffer.len());
        let mut json_bytes=serde_json::to_vec(&root).map_err(|e|e.to_string())?;
        while json_bytes.len()%4!=0 {json_bytes.push(b' ');}
        let whole=12usize.checked_add(8+json_bytes.len())
            .and_then(|n|n.checked_add(8+buffer.len())).ok_or("GLB 文件过大")?;
        let length=u32::try_from(whole).map_err(|_|"GLB 文件超过 4 GiB".to_owned())?;
        let mut glb=Vec::with_capacity(whole);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&length.to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(buffer.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&buffer);
        atomic_write(output,&glb,cancel)
    } else {
        root["buffers"][0]["uri"]=json!(format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buffer),
        ));
        let bytes=serde_json::to_vec(&root).map_err(|e|e.to_string())?;
        atomic_write(output,&bytes,cancel)
    }
}

fn atomic_write(output: &Path, bytes: &[u8], cancel: &CancelFlag) -> Result<(), String> {
    if cancel.is_cancelled() {return Err("模型导出已取消".into());}
    let root=output.parent().unwrap_or_else(|| Path::new("."));
    let tmp=root.join(format!(".bmcbl-model-{}-{}.tmp",std::process::id(),std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos()));
    std::fs::write(&tmp,bytes).map_err(|error|error.to_string())?;
    if cancel.is_cancelled() {
        let _=std::fs::remove_file(&tmp);
        return Err("模型导出已取消".into());
    }
    // Windows rename does not replace an existing destination: remove only
    // after a successful temp write. A rename failure leaves no temp file.
    let outcome=(||{
        if output.exists(){std::fs::remove_file(output).map_err(|e|e.to_string())?;}
        std::fs::rename(&tmp,output).map_err(|e|e.to_string())
    })();
    if outcome.is_err(){let _=std::fs::remove_file(&tmp);}
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exported_gltf_and_glb_import_via_gpui_3d() {
        use bedrock_voxel::{ObjMaterial, ObjTriangle, ObjVertex};
        let triangle = ObjTriangle {
            vertices: [
                ObjVertex { position:[0.0,0.0,0.0], uv:Some([0.0,0.0]) },
                ObjVertex { position:[1.0,0.0,0.0], uv:Some([1.0,0.0]) },
                ObjVertex { position:[0.0,1.0,0.0], uv:Some([0.0,1.0]) },
            ],
            material:Some(0),
        };
        let model=ObjModel {
            triangles:vec![triangle],
            materials:vec![ObjMaterial{
                diffuse:Some([0.8,0.3,0.5]),
                opacity:1.0,
                diffuse_texture:None,
            }],
        };
        let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap().as_nanos();
        for ext in ["gltf","glb"] {
            let path=std::env::temp_dir()
                .join(format!("bmcbl-model-export-{stamp}.{ext}"));
            write_model(&model,&[None],&path,&CancelFlag::new()).unwrap();
            let loaded=gpui_3d::load_imported_path(
                &path,&gpui_3d::GltfLoadOptions::default(),
            ).unwrap();
            assert_eq!(loaded.triangle_count,1);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn wavefront_export_creates_material_and_texture_sidecars() {
        use bedrock_voxel::{ObjMaterial, ObjTriangle, ObjVertex};
        let model=ObjModel {
            triangles:vec![ObjTriangle{
                vertices:[
                    ObjVertex { position:[0.,0.,0.],uv:Some([0.,0.]) },
                    ObjVertex { position:[1.,0.,0.],uv:Some([1.,0.]) },
                    ObjVertex { position:[0.,1.,0.],uv:Some([0.,1.]) },
                ],material:Some(0),
            }],
            materials:vec![ObjMaterial{
                diffuse:Some([1.;3]),opacity:1.0,diffuse_texture:None,
            }],
        };
        let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap().as_nanos();
        let folder=std::env::temp_dir().join(format!("bmcbl-obj-export-{stamp}"));
        std::fs::create_dir(&folder).unwrap();
        let target=folder.join("source.obj");
        let texture=Arc::new(SourceTexture {
            width:1,height:1,has_alpha:false,pixels:Arc::<[u8]>::from(vec![255u8,64,32,255]),
        });
        write_model(&model,&[Some(texture)],&target,&CancelFlag::new()).unwrap();
        let obj=std::fs::read_to_string(&target).unwrap();
        assert!(obj.contains("mtllib source.mtl"));
        assert!(obj.contains("usemtl material_0"));
        let mtl=std::fs::read_to_string(folder.join("source.mtl")).unwrap();
        assert!(mtl.contains("map_Kd source_material_0.png"));
        assert!(folder.join("source_material_0.png").exists());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn unsupported_model_suffix_is_rejected() {
        let model=ObjModel{triangles:Vec::new(),materials:Vec::new()};
        let cancel=CancelFlag::new();
        assert!(write_model(&model,&[],Path::new("empty.fbx"),&cancel).is_err());
    }
}
