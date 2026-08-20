use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use image::DynamicImage;
use serde_json::Value;

use crate::json::read_json_file;
use crate::{
    BlockFace, BlockModelError, BlockModelRepository, BlockStateQuery, ModelWarning, Result,
};

pub const MATERIAL_SLOT_SEPARATOR: &str = "__mat_";
pub const FALLBACK_MATERIAL_NAME: &str = "minecraft_unknown";

#[derive(Clone, Debug, PartialEq)]
pub struct ObjMaterial {
    pub diffuse_color: [f32; 3],
    pub dissolve: f32,
    pub relative_texture_path: Option<String>,
    pub alpha_texture_path: Option<String>,
    pub texture_tint: Option<[f32; 3]>,
    pub use_texture_alpha: bool,
}

impl ObjMaterial {
    #[must_use]
    pub fn from_preview_color(
        material_name: &str,
        color: [f32; 4],
        relative_texture_path: Option<String>,
    ) -> Self {
        let has_texture = relative_texture_path.is_some();
        let texture_tint = has_texture
            .then(|| obj_material_texture_tint(material_name, color))
            .flatten();
        let use_preview_color = texture_tint.is_none()
            && (!has_texture || obj_material_uses_preview_tint(material_name));
        let diffuse_color = if use_preview_color {
            [
                color[0].clamp(0.0, 1.0),
                color[1].clamp(0.0, 1.0),
                color[2].clamp(0.0, 1.0),
            ]
        } else {
            [1.0, 1.0, 1.0]
        };
        let dissolve = obj_material_preview_dissolve(material_name, color, use_preview_color);
        let use_texture_alpha = has_texture && obj_material_uses_texture_alpha(material_name);
        let alpha_texture_path = if use_texture_alpha {
            relative_texture_path.as_deref().map(obj_alpha_texture_path)
        } else {
            None
        };
        Self {
            diffuse_color,
            dissolve,
            relative_texture_path,
            alpha_texture_path,
            texture_tint,
            use_texture_alpha,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjExportMaterial {
    pub source_texture_path: Option<PathBuf>,
    pub material: ObjMaterial,
}

impl ObjExportMaterial {
    #[must_use]
    pub fn from_preview_color(
        material_name: &str,
        color: [f32; 4],
        source_texture_path: Option<PathBuf>,
        relative_texture_path: Option<String>,
    ) -> Self {
        Self {
            source_texture_path,
            material: ObjMaterial::from_preview_color(material_name, color, relative_texture_path),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjMaterialSample<'a> {
    pub name: Cow<'a, str>,
    pub color: [f32; 4],
    pub normal: [i32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjTextureCopy {
    pub source_path: PathBuf,
    pub relative_path: String,
    pub tint: Option<[f32; 3]>,
    pub alpha_mask: bool,
}

impl ObjTextureCopy {
    #[must_use]
    pub fn needs_png_conversion(&self) -> bool {
        true
    }
}

pub fn write_obj_texture_copy(texture_copy: &ObjTextureCopy, target_path: &Path) -> Result<()> {
    let image = read_obj_texture_copy_image(texture_copy)?;
    let image = if texture_copy.alpha_mask {
        alpha_mask_obj_texture_image(image)
    } else if let Some(tint) = texture_copy.tint {
        let tinted = tint_obj_texture_image(image, tint);
        upscale_and_clean_texture_image(tinted)
    } else {
        upscale_and_clean_texture_image(image)
    };
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|error| {
            BlockModelError::Message(format!(
                "failed to encode OBJ texture {} as PNG: {error}",
                texture_copy.source_path.display()
            ))
        })?;
    std::fs::write(target_path, encoded.into_inner()).map_err(|source| BlockModelError::Write {
        path: target_path.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn write_obj_export_files(
    export: &ObjExport,
    obj_path: &Path,
    material_library_path: &Path,
    export_root: &Path,
) -> Result<ObjExportWriteSummary> {
    create_obj_export_directory(export_root)?;
    create_obj_export_parent_directory(obj_path)?;
    create_obj_export_parent_directory(material_library_path)?;

    std::fs::write(obj_path, &export.obj_text).map_err(|source| BlockModelError::Write {
        path: obj_path.to_path_buf(),
        source,
    })?;
    std::fs::write(material_library_path, &export.material_library_text).map_err(|source| {
        BlockModelError::Write {
            path: material_library_path.to_path_buf(),
            source,
        }
    })?;

    for texture_copy in &export.texture_copies {
        let target_path = obj_export_texture_target_path(export_root, &texture_copy.relative_path)?;
        create_obj_export_parent_directory(&target_path)?;
        write_obj_texture_copy(texture_copy, &target_path)?;
    }

    Ok(ObjExportWriteSummary {
        obj_path: obj_path.to_path_buf(),
        material_library_path: material_library_path.to_path_buf(),
        texture_copy_count: export.texture_copies.len(),
    })
}

fn create_obj_export_parent_directory(path: &Path) -> Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    create_obj_export_directory(parent)
}

fn create_obj_export_directory(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|source| BlockModelError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn obj_export_texture_target_path(export_root: &Path, relative_path: &str) -> Result<PathBuf> {
    let relative_path = Path::new(relative_path);
    let leaves_export_root = relative_path.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    });
    if leaves_export_root {
        return Err(BlockModelError::Message(format!(
            "OBJ texture target path must stay inside export root: {}",
            relative_path.display()
        )));
    }
    Ok(export_root.join(relative_path))
}

pub fn read_obj_texture_copy_image(texture_copy: &ObjTextureCopy) -> Result<DynamicImage> {
    let bytes =
        std::fs::read(&texture_copy.source_path).map_err(|source| BlockModelError::Read {
            path: texture_copy.source_path.clone(),
            source,
        })?;
    if path_extension_eq(&texture_copy.source_path, "tga") {
        return image::load_from_memory_with_format(&bytes, image::ImageFormat::Tga).map_err(
            |error| {
                BlockModelError::Message(format!(
                    "failed to decode OBJ TGA texture {}: {error}",
                    texture_copy.source_path.display()
                ))
            },
        );
    }
    image::load_from_memory(&bytes).map_err(|error| {
        BlockModelError::Message(format!(
            "failed to decode OBJ texture {}: {error}",
            texture_copy.source_path.display()
        ))
    })
}

#[must_use]
pub fn tint_obj_texture_image(image: DynamicImage, tint: [f32; 3]) -> DynamicImage {
    let mut image = image.to_rgba8();
    for pixel in image.pixels_mut() {
        pixel[0] = ((f32::from(pixel[0]) * tint[0]).round()).clamp(0.0, 255.0) as u8;
        pixel[1] = ((f32::from(pixel[1]) * tint[1]).round()).clamp(0.0, 255.0) as u8;
        pixel[2] = ((f32::from(pixel[2]) * tint[2]).round()).clamp(0.0, 255.0) as u8;
    }
    DynamicImage::ImageRgba8(image)
}

#[must_use]
pub fn upscale_and_clean_texture_image(image: DynamicImage) -> DynamicImage {
    let (width, height) = (image.width(), image.height());
    if width <= 64 && height <= 64 && width > 0 && height > 0 {
        let scale = (128 / width.max(1)).max(1);
        if scale > 1 {
            return image.resize_exact(
                width * scale,
                height * scale,
                image::imageops::FilterType::Nearest,
            );
        }
    }
    image
}

#[must_use]
pub fn alpha_mask_obj_texture_image(image: DynamicImage) -> DynamicImage {
    let (width, height) = (image.width(), image.height());
    let rgba = image.to_rgba8();
    let mut mask = image::RgbImage::new(width, height);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = if pixel[3] > 10 { 255 } else { 0 };
        mask.put_pixel(x, y, image::Rgb([alpha, alpha, alpha]));
    }
    let mask_image = DynamicImage::ImageRgb8(mask);
    if width <= 64 && height <= 64 && width > 0 && height > 0 {
        let scale = (128 / width.max(1)).max(1);
        if scale > 1 {
            return mask_image.resize_exact(
                width * scale,
                height * scale,
                image::imageops::FilterType::Nearest,
            );
        }
    }
    mask_image
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjExport {
    pub obj_text: String,
    pub material_library_text: String,
    pub texture_copies: Vec<ObjTextureCopy>,
    pub material_count: usize,
    pub textured_material_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjExportTarget {
    pub export_root: PathBuf,
    pub obj_path: PathBuf,
    pub material_library_path: PathBuf,
    pub material_library_name: String,
}

impl ObjExportTarget {
    pub fn from_obj_path(path: &Path) -> Result<Self> {
        let output_stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                BlockModelError::Message(format!(
                    "failed to derive OBJ export directory name from {}",
                    path.display()
                ))
            })?;
        let export_root = path.with_file_name(output_stem);
        let obj_path = export_root.join(format!("{output_stem}.obj"));
        let material_library_path = export_root.join(format!("{output_stem}.mtl"));
        let material_library_name = material_library_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                BlockModelError::Message(format!(
                    "failed to derive OBJ material library name from {}",
                    material_library_path.display()
                ))
            })?
            .to_owned();
        Ok(Self {
            export_root,
            obj_path,
            material_library_path,
            material_library_name,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjExportWriteSummary {
    pub obj_path: PathBuf,
    pub material_library_path: PathBuf,
    pub texture_copy_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedObjMaterial<'a> {
    pub name: Cow<'a, str>,
    pub material: ObjMaterial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjResolvedTexture {
    pub source_path: PathBuf,
    pub relative_path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjFace<'a> {
    pub material: Cow<'a, str>,
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjMeshFace<'a> {
    pub material: Cow<'a, str>,
    pub color: [f32; 4],
    pub triangle_positions: [[f32; 3]; 6],
    pub uv: [[f32; 2]; 4],
}

pub trait ObjMeshFaceSource {
    fn obj_face_count(&self) -> usize;

    fn obj_face_material(&self, face_index: usize) -> Option<&str>;

    fn obj_face_color(&self, face_index: usize) -> Option<[f32; 4]>;

    fn obj_face_triangle_positions(&self, face_index: usize) -> Option<[[f32; 3]; 6]>;

    fn obj_face_uv(&self, face_index: usize) -> Option<[[f32; 2]; 4]>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjTextureResolver {
    packs: Vec<ObjResourcePack>,
    models: BlockModelRepository,
    texture_directory_name: String,
    model_errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjResourcePack {
    root: PathBuf,
    texture_root: PathBuf,
    blocks: Value,
    texture_data: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObjTextureSlot {
    Up,
    Down,
    Side,
}

impl ObjTextureResolver {
    #[must_use]
    pub fn with_pack_roots<I, P>(pack_roots: I, texture_directory_name: &str) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut unique_pack_roots = Vec::new();
        for pack_root in pack_roots {
            let pack_root = pack_root.as_ref().to_path_buf();
            if !unique_pack_roots
                .iter()
                .any(|existing| existing == &pack_root)
            {
                unique_pack_roots.push(pack_root);
            }
        }

        let packs = unique_pack_roots
            .iter()
            .filter_map(|root| ObjResourcePack::load(root.clone()))
            .collect();
        let mut models = BlockModelRepository::new();
        let mut model_errors = Vec::new();
        for root in &unique_pack_roots {
            if let Err(error) = models.merge_pack(root) {
                model_errors.push(error.to_string());
            }
        }

        Self {
            packs,
            models,
            texture_directory_name: texture_directory_name.to_owned(),
            model_errors,
        }
    }

    #[must_use]
    pub fn with_package_roots<I, P>(package_roots: I, texture_directory_name: &str) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        Self::with_pack_roots(
            vanilla_resource_pack_roots_from_packages(package_roots),
            texture_directory_name,
        )
    }

    pub fn try_with_pack_roots<I, P>(pack_roots: I, texture_directory_name: &str) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut resolver = Self::with_pack_roots(pack_roots, texture_directory_name);
        if let Some(error) = resolver.model_errors.pop() {
            return Err(BlockModelError::Message(error));
        }
        Ok(resolver)
    }

    pub fn try_with_package_roots<I, P>(
        package_roots: I,
        texture_directory_name: &str,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut resolver = Self::with_package_roots(package_roots, texture_directory_name);
        if let Some(error) = resolver.model_errors.pop() {
            return Err(BlockModelError::Message(error));
        }
        Ok(resolver)
    }

    #[must_use]
    pub fn texture_for(&self, material: &str, normal: [i32; 3]) -> Option<ObjResolvedTexture> {
        let block = obj_block_texture_name(material);
        let lookup_block = obj_canonical_block_lookup_name(&block);
        let material_texture = obj_material_texture_name(material);
        let material_slot = obj_material_instance_slot(material);
        let slot_face = material_slot
            .as_deref()
            .map(BlockFace::parse)
            .filter(|face| !matches!(face, BlockFace::Default | BlockFace::All));
        let block_face = slot_face
            .or_else(|| obj_material_block_face(material))
            .unwrap_or_else(|| obj_block_face_for_normal(normal));
        let lookup_normal = obj_normal_for_block_face(block_face);

        if is_full_grass_block(&block)
            && let Some(texture_key) = grass_block_texture_key_for_face(block_face)
            && let Some((pack, source_path)) = self.texture_path_for_key(&texture_key)
        {
            return Some(pack.resolved_texture(&self.texture_directory_name, source_path));
        }

        if material_slot.as_deref() == Some("candle")
            && let Some(candle_block) = candle_block_for_cake(&lookup_block)
            && let Some(texture_key) = self.resolved_texture_key_for(&candle_block, block_face)
            && let Some((pack, source_path)) = self.texture_path_for_key(&texture_key)
        {
            return Some(pack.resolved_texture(&self.texture_directory_name, source_path));
        }

        if let Some(material_slot) = material_slot.as_deref()
            && let Some(texture_key) =
                self.resolved_texture_key_for_material_slot(&lookup_block, material_slot)
            && let Some((pack, source_path)) = self.texture_path_for_key(&texture_key)
        {
            return Some(pack.resolved_texture(&self.texture_directory_name, source_path));
        }

        if let Some(texture_key) = self.resolved_texture_key_for(&lookup_block, block_face)
            && let Some((pack, source_path)) = self.texture_path_for_key(&texture_key)
        {
            return Some(pack.resolved_texture(&self.texture_directory_name, source_path));
        }

        for pack in self.packs.iter().rev() {
            if let Some(texture_key) = pack.block_texture_key_for(&lookup_block, lookup_normal)
                && let Some(source_path) = pack.texture_path_for_key(&texture_key)
            {
                return Some(pack.resolved_texture(&self.texture_directory_name, source_path));
            }
            for texture_key in
                obj_fallback_texture_keys(&material_texture, &lookup_block, lookup_normal)
            {
                if let Some(source_path) = pack.texture_path_for_key(&texture_key) {
                    return Some(pack.resolved_texture(&self.texture_directory_name, source_path));
                }
            }
        }

        None
    }

    #[must_use]
    pub fn model_errors(&self) -> &[String] {
        &self.model_errors
    }

    #[must_use]
    pub fn models(&self) -> &BlockModelRepository {
        &self.models
    }

    fn resolved_texture_key_for(&self, block: &str, face: BlockFace) -> Option<String> {
        let state = BlockStateQuery::new(obj_block_identifier(block));
        let resolved = self.models.resolve_block(&state);
        if resolved
            .warnings
            .iter()
            .any(|warning| matches!(warning, ModelWarning::MissingBlockDefinition(_)))
        {
            return None;
        }
        obj_texture_faces_for_block_face(face)
            .iter()
            .find_map(|face| resolved.face_textures.get(face))
            .map(|texture| texture.key.clone())
            .or_else(|| {
                obj_material_slots_for_block_face(face)
                    .iter()
                    .find_map(|slot| resolved.materials.get(*slot))
                    .and_then(|material| material.texture_key.clone())
            })
    }

    fn resolved_texture_key_for_material_slot(&self, block: &str, slot: &str) -> Option<String> {
        let state = BlockStateQuery::new(obj_block_identifier(block));
        let resolved = self.models.resolve_block(&state);
        if resolved
            .warnings
            .iter()
            .any(|warning| matches!(warning, ModelWarning::MissingBlockDefinition(_)))
        {
            return None;
        }
        if let Some(texture_key) = resolved
            .materials
            .get(slot)
            .and_then(|material| material.texture_key.clone())
        {
            return Some(texture_key);
        }
        if let Some(face) = material_slot_block_face(slot)
            && let Some(texture_key) = obj_texture_faces_for_block_face(face)
                .iter()
                .find_map(|face| resolved.face_textures.get(face))
                .map(|texture| texture.key.clone())
        {
            return Some(texture_key);
        }
        obj_material_slot_candidate_list(slot)
            .iter()
            .find_map(|slot| resolved.materials.get(*slot))
            .and_then(|material| material.texture_key.clone())
    }

    fn texture_path_for_key(&self, texture_key: &str) -> Option<(&ObjResourcePack, PathBuf)> {
        for pack in self.packs.iter().rev() {
            if let Some(source_path) = pack.texture_path_for_key(texture_key) {
                return Some((pack, source_path));
            }
        }
        None
    }
}

impl ObjResourcePack {
    #[must_use]
    pub fn load(root: PathBuf) -> Option<Self> {
        let texture_root = root.join("textures");
        if !texture_root.is_dir() {
            return None;
        }
        let blocks = read_optional_json(&root.join("blocks.json")).unwrap_or(Value::Null);
        let texture_data = read_texture_data_maps(&root, &texture_root);
        Some(Self {
            root,
            texture_root,
            blocks,
            texture_data,
        })
    }

    #[must_use]
    pub fn block_texture_key_for(&self, block: &str, normal: [i32; 3]) -> Option<String> {
        let object = block_definition(&self.blocks, block)?;
        let textures = object
            .get("textures")
            .or_else(|| object.get("texture"))
            .or_else(|| object.get("carried_textures"))?;
        obj_texture_key_from_value(textures, normal)
    }

    #[must_use]
    pub fn texture_path_for_key(&self, texture_key: &str) -> Option<PathBuf> {
        let normalized = self
            .texture_data
            .get(texture_key)
            .map(String::as_str)
            .unwrap_or(texture_key);
        let normalized = obj_normalize_texture_key(normalized);
        for relative_path in texture_candidate_relatives(&normalized) {
            if let Some(path) = find_texture_file(&self.texture_root, &relative_path) {
                return Some(path);
            }
            if let Some(path) = find_texture_file(&self.root, &relative_path) {
                return Some(path);
            }
        }
        None
    }

    #[must_use]
    pub fn packaged_texture_path(
        &self,
        texture_directory_name: &str,
        source_path: &Path,
    ) -> String {
        if let Ok(relative_path) = source_path.strip_prefix(&self.texture_root) {
            return prefixed_relative_path(texture_directory_name, relative_path);
        }
        if let Ok(relative_path) = source_path.strip_prefix(&self.root)
            && let Some(path) = relative_path_string(relative_path)
        {
            if path_starts_with_directory(&path, texture_directory_name) {
                return path;
            }
            return format!("{texture_directory_name}/{path}");
        }
        source_path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .map(|file_name| format!("{texture_directory_name}/{file_name}"))
            .unwrap_or_else(|| format!("{texture_directory_name}/texture.png"))
    }

    #[must_use]
    pub fn export_texture_path(&self, texture_directory_name: &str, source_path: &Path) -> String {
        let relative_path = self.packaged_texture_path(texture_directory_name, source_path);
        if path_extension_eq(source_path, "tga") {
            replace_path_extension(&relative_path, "png")
        } else {
            relative_path
        }
    }

    fn resolved_texture(
        &self,
        texture_directory_name: &str,
        source_path: PathBuf,
    ) -> ObjResolvedTexture {
        ObjResolvedTexture {
            relative_path: self.export_texture_path(texture_directory_name, &source_path),
            source_path,
        }
    }
}

#[must_use]
pub fn obj_material_library_string<'a>(
    materials: impl IntoIterator<Item = NamedObjMaterial<'a>>,
) -> String {
    let materials = materials.into_iter();
    let (lower_bound, _) = materials.size_hint();
    let mut output = String::with_capacity(lower_bound.saturating_mul(160).saturating_add(64));
    output.push_str("# bedrock-block-model OBJ MTL export\n");
    for NamedObjMaterial { name, material } in materials {
        let _ = writeln!(output, "newmtl {name}");
        let _ = writeln!(
            output,
            "Kd {:.6} {:.6} {:.6}",
            material.diffuse_color[0], material.diffuse_color[1], material.diffuse_color[2]
        );
        output.push_str("Ka 0.000000 0.000000 0.000000\n");
        output.push_str("Ks 0.000000 0.000000 0.000000\n");
        let dissolve = material.dissolve.clamp(0.0, 1.0);
        let _ = writeln!(output, "d {dissolve:.6}");
        if material.use_texture_alpha || dissolve < 1.0 {
            output.push_str("illum 4\n");
            let _ = writeln!(output, "Tf 1.000000 1.000000 1.000000");
        }
        if let Some(texture_path) = &material.relative_texture_path {
            let _ = writeln!(output, "map_Kd {texture_path}");
            if material.use_texture_alpha {
                let alpha_texture_path = material
                    .alpha_texture_path
                    .as_deref()
                    .unwrap_or(texture_path);
                let _ = writeln!(output, "map_d -imfchan a {alpha_texture_path}");
                let _ = writeln!(output, "map_tr -imfchan a {alpha_texture_path}");
            }
        }
        output.push('\n');
    }
    output
}

#[must_use]
pub fn obj_material_library_from_export_materials<K: AsRef<str>>(
    materials: &BTreeMap<K, ObjExportMaterial>,
) -> String {
    obj_material_library_string(materials.iter().map(|(name, material)| NamedObjMaterial {
        name: Cow::Borrowed(name.as_ref()),
        material: material.material.clone(),
    }))
}

#[must_use]
pub fn obj_document_string<'a>(
    comment: &str,
    material_library_name: &str,
    object_name: &str,
    parts: impl IntoIterator<Item = &'a str>,
) -> String {
    let parts = parts.into_iter();
    let (lower_bound, _) = parts.size_hint();
    let mut output = String::with_capacity(
        lower_bound
            .saturating_mul(192)
            .saturating_add(comment.len())
            .saturating_add(material_library_name.len())
            .saturating_add(object_name.len())
            .saturating_add(64),
    );
    output.push_str("# ");
    output.push_str(comment);
    output.push('\n');
    output.push_str("mtllib ");
    output.push_str(material_library_name);
    output.push('\n');
    output.push_str("o ");
    output.push_str(object_name);
    output.push('\n');
    for part in parts {
        output.push_str(part);
    }
    output
}

#[must_use]
pub fn obj_export_from_parts<'a, K: AsRef<str>>(
    comment: &str,
    material_library_name: &str,
    object_name: &str,
    materials: &BTreeMap<K, ObjExportMaterial>,
    parts: impl IntoIterator<Item = &'a str>,
) -> ObjExport {
    let material_library_text = obj_material_library_from_export_materials(materials);
    let texture_copies = obj_texture_copies(materials.values());
    let textured_material_count = materials
        .values()
        .filter(|material| material.source_texture_path.is_some())
        .count();
    let obj_text = obj_document_string(comment, material_library_name, object_name, parts);

    ObjExport {
        obj_text,
        material_library_text,
        texture_copies,
        material_count: materials.len(),
        textured_material_count,
    }
}

#[must_use]
pub fn obj_export_from_mesh_face_groups<'a, G>(
    comment: &str,
    material_library_name: &str,
    object_name: &str,
    face_groups: G,
    resolver: &ObjTextureResolver,
) -> ObjExport
where
    G: IntoIterator<Item = Vec<ObjMeshFace<'a>>>,
{
    obj_export_from_mesh_face_groups_with_progress(
        comment,
        material_library_name,
        object_name,
        face_groups,
        resolver,
        |_, _| {},
    )
}

#[must_use]
pub fn obj_export_from_mesh_face_groups_with_progress<'a, G>(
    comment: &str,
    material_library_name: &str,
    object_name: &str,
    face_groups: G,
    resolver: &ObjTextureResolver,
    mut progress: impl FnMut(usize, usize),
) -> ObjExport
where
    G: IntoIterator<Item = Vec<ObjMeshFace<'a>>>,
{
    let mut face_groups = face_groups.into_iter().collect::<Vec<_>>();
    obj_cull_hidden_mesh_faces(&mut face_groups);
    let materials = obj_mesh_face_materials(
        face_groups.iter().flat_map(|faces| faces.iter().cloned()),
        resolver,
    );
    let offsets = obj_vertex_offsets(face_groups.iter().map(Vec::len));
    let total_parts = face_groups.len().max(1);
    progress(0, total_parts);

    let mut parts = Vec::with_capacity(face_groups.len());
    for (index, (faces, vertex_offset)) in face_groups.into_iter().zip(offsets).enumerate() {
        parts.push(obj_mesh_faces_string(faces, vertex_offset));
        progress(index + 1, total_parts);
    }

    obj_export_from_parts(
        comment,
        material_library_name,
        object_name,
        &materials,
        parts.iter().map(String::as_str),
    )
}

#[must_use]
pub fn obj_export_from_face_sources_with_package_roots<'a, S, F, R, P>(
    comment: &str,
    material_library_name: &str,
    object_name: &str,
    face_sources: F,
    package_roots: R,
    texture_directory_name: &str,
    progress: impl FnMut(usize, usize),
) -> ObjExport
where
    S: ObjMeshFaceSource + ?Sized + 'a,
    F: IntoIterator<Item = &'a S>,
    R: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let resolver = ObjTextureResolver::with_package_roots(package_roots, texture_directory_name);
    let face_groups = face_sources
        .into_iter()
        .map(obj_mesh_faces_from_source)
        .collect::<Vec<_>>();
    obj_export_from_mesh_face_groups_with_progress(
        comment,
        material_library_name,
        object_name,
        face_groups,
        &resolver,
        progress,
    )
}

#[must_use]
pub fn export_obj_from_face_sources_with_package_roots<'a, S, F, R, P>(
    comment: &str,
    material_library_name: &str,
    object_name: &str,
    face_sources: F,
    package_roots: R,
    texture_directory_name: &str,
    progress: impl FnMut(usize, usize),
) -> ObjExport
where
    S: ObjMeshFaceSource + ?Sized + 'a,
    F: IntoIterator<Item = &'a S>,
    R: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    obj_export_from_face_sources_with_package_roots(
        comment,
        material_library_name,
        object_name,
        face_sources,
        package_roots,
        texture_directory_name,
        progress,
    )
}

pub fn obj_cull_hidden_mesh_faces(face_groups: &mut [Vec<ObjMeshFace<'_>>]) {
    let mut hidden = face_groups
        .iter()
        .map(|faces| vec![false; faces.len()])
        .collect::<Vec<_>>();
    let mut candidates: BTreeMap<ObjHiddenFaceKey, Vec<ObjHiddenFaceRef>> = BTreeMap::new();

    for (group_index, faces) in face_groups.iter().enumerate() {
        for (face_index, face) in faces.iter().enumerate() {
            if !obj_mesh_face_can_be_hidden(face) {
                continue;
            }
            let Some(normal) = obj_mesh_face_normal(face) else {
                continue;
            };
            let key = obj_hidden_face_key(face);
            candidates.entry(key).or_default().push(ObjHiddenFaceRef {
                group_index,
                face_index,
                normal,
            });
        }
    }

    for refs in candidates.values() {
        for (left_index, left) in refs.iter().enumerate() {
            if hidden[left.group_index][left.face_index] {
                continue;
            }
            for right in refs.iter().skip(left_index + 1) {
                if hidden[right.group_index][right.face_index] {
                    continue;
                }
                if obj_normals_are_opposite(left.normal, right.normal) {
                    hidden[left.group_index][left.face_index] = true;
                    hidden[right.group_index][right.face_index] = true;
                    break;
                }
            }
        }
    }

    for (faces, hidden_faces) in face_groups.iter_mut().zip(hidden) {
        let mut index = 0usize;
        faces.retain(|_| {
            let keep = !hidden_faces[index];
            index += 1;
            keep
        });
    }
}

#[must_use]
pub fn obj_mesh_faces_from_source(
    source: &(impl ObjMeshFaceSource + ?Sized),
) -> Vec<ObjMeshFace<'_>> {
    let mut faces = Vec::with_capacity(source.obj_face_count());
    for face_index in 0..source.obj_face_count() {
        let Some(material) = source.obj_face_material(face_index) else {
            continue;
        };
        let Some(color) = source.obj_face_color(face_index) else {
            continue;
        };
        let Some(triangle_positions) = source.obj_face_triangle_positions(face_index) else {
            continue;
        };
        let Some(uv) = source.obj_face_uv(face_index) else {
            continue;
        };
        faces.push(ObjMeshFace {
            material: Cow::Borrowed(material),
            color,
            triangle_positions,
            uv,
        });
    }
    faces
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ObjHiddenFaceKey {
    vertices: [[i64; 3]; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObjHiddenFaceRef {
    group_index: usize,
    face_index: usize,
    normal: [i32; 3],
}

fn obj_mesh_face_can_be_hidden(face: &ObjMeshFace<'_>) -> bool {
    face.color[3] >= 0.999 && !obj_material_uses_texture_alpha(face.material.as_ref())
}

fn obj_mesh_face_normal(face: &ObjMeshFace<'_>) -> Option<[i32; 3]> {
    obj_face_normal_from_triangle(
        face.triangle_positions[0],
        face.triangle_positions[1],
        face.triangle_positions[2],
    )
}

fn obj_hidden_face_key(face: &ObjMeshFace<'_>) -> ObjHiddenFaceKey {
    let mut vertices = [
        obj_quantized_position(face.triangle_positions[0]),
        obj_quantized_position(face.triangle_positions[1]),
        obj_quantized_position(face.triangle_positions[2]),
        obj_quantized_position(face.triangle_positions[5]),
    ];
    vertices.sort_unstable();
    ObjHiddenFaceKey { vertices }
}

#[allow(clippy::cast_possible_truncation)]
fn obj_quantized_position(position: [f32; 3]) -> [i64; 3] {
    const SCALE: f32 = 1_000_000.0;
    [
        (position[0] * SCALE).round() as i64,
        (position[1] * SCALE).round() as i64,
        (position[2] * SCALE).round() as i64,
    ]
}

fn obj_normals_are_opposite(left: [i32; 3], right: [i32; 3]) -> bool {
    left[0] == -right[0] && left[1] == -right[1] && left[2] == -right[2]
}

#[must_use]
pub fn obj_vertex_offsets(face_counts: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut next_offset = 1usize;
    face_counts
        .into_iter()
        .map(|face_count| {
            let offset = next_offset;
            next_offset = next_offset.saturating_add(face_count.saturating_mul(4));
            offset
        })
        .collect()
}

#[must_use]
pub fn obj_faces_string<'a>(
    faces: impl IntoIterator<Item = ObjFace<'a>>,
    vertex_offset: usize,
) -> String {
    let faces = faces.into_iter();
    let (lower_bound, _) = faces.size_hint();
    let mut output = String::with_capacity(lower_bound.saturating_mul(192));
    let mut current_material: Option<String> = None;
    for (face_index, face) in faces.enumerate() {
        if current_material.as_deref() != Some(face.material.as_ref()) {
            output.push_str("usemtl ");
            output.push_str(face.material.as_ref());
            output.push('\n');
            current_material = Some(face.material.into_owned());
        }
        for position in face.positions {
            push_obj_vertex(&mut output, position);
        }
        push_obj_texcoords(&mut output, &face.uv);
        let a = vertex_offset.saturating_add(face_index.saturating_mul(4));
        let b = a.saturating_add(1);
        let c = a.saturating_add(2);
        let d = a.saturating_add(3);
        push_obj_quad_face(&mut output, a, b, c, d);
    }
    output
}

#[must_use]
pub fn obj_mesh_faces_string<'a>(
    faces: impl IntoIterator<Item = ObjMeshFace<'a>>,
    vertex_offset: usize,
) -> String {
    obj_faces_string(
        faces.into_iter().map(obj_face_from_mesh_face),
        vertex_offset,
    )
}

#[must_use]
pub fn obj_default_face_uvs_from_corners(corners: &[[f32; 3]; 4]) -> [[f32; 2]; 4] {
    let u_span = obj_vec3_distance(corners[1], corners[0]).max(1.0);
    let v_span = obj_vec3_distance(corners[3], corners[0]).max(1.0);
    [[0.0, 0.0], [u_span, 0.0], [u_span, v_span], [0.0, v_span]]
}

#[must_use]
pub fn default_block_face_uvs_from_corners(corners: &[[f32; 3]; 4]) -> [[f32; 2]; 4] {
    obj_default_face_uvs_from_corners(corners)
}

#[must_use]
pub fn obj_face_normal_from_triangle(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<[i32; 3]> {
    let normal = obj_vec3_normalize(obj_vec3_cross(obj_vec3_sub(b, a), obj_vec3_sub(c, a)));
    let axis = (0..3).max_by(|left, right| {
        normal[*left]
            .abs()
            .partial_cmp(&normal[*right].abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let mut result = [0, 0, 0];
    result[axis] = if normal[axis].is_sign_negative() {
        -1
    } else {
        1
    };
    Some(result)
}

#[must_use]
pub fn obj_export_materials<'a>(
    samples: impl IntoIterator<Item = ObjMaterialSample<'a>>,
    resolver: &ObjTextureResolver,
) -> BTreeMap<String, ObjExportMaterial> {
    let mut materials = BTreeMap::new();
    for sample in samples {
        let material_name = sample.name.into_owned();
        materials.entry(material_name).or_insert_with_key(|name| {
            let resolved_texture = resolver.texture_for(name, sample.normal);
            ObjExportMaterial::from_preview_color(
                name,
                sample.color,
                resolved_texture
                    .as_ref()
                    .map(|texture| texture.source_path.clone()),
                resolved_texture.map(|texture| texture.relative_path),
            )
        });
    }
    materials
}

#[must_use]
pub fn obj_mesh_face_materials<'a>(
    faces: impl IntoIterator<Item = ObjMeshFace<'a>>,
    resolver: &ObjTextureResolver,
) -> BTreeMap<String, ObjExportMaterial> {
    obj_export_materials(
        faces.into_iter().map(obj_material_sample_from_mesh_face),
        resolver,
    )
}

#[must_use]
pub fn obj_texture_copies<'a>(
    materials: impl IntoIterator<Item = &'a ObjExportMaterial>,
) -> Vec<ObjTextureCopy> {
    let mut texture_copies = Vec::new();
    for material in materials {
        let Some(source_path) = material.source_texture_path.clone() else {
            continue;
        };
        if let Some(relative_path) = material.material.relative_texture_path.clone() {
            texture_copies.push(ObjTextureCopy {
                source_path: source_path.clone(),
                relative_path,
                tint: material.material.texture_tint,
                alpha_mask: false,
            });
        }
        if let Some(relative_path) = material.material.alpha_texture_path.clone() {
            texture_copies.push(ObjTextureCopy {
                source_path,
                relative_path,
                tint: None,
                alpha_mask: true,
            });
        }
    }
    texture_copies
}

#[must_use]
pub fn obj_face_texture_slot_suffix(normal: [i32; 3]) -> &'static str {
    match obj_face_texture_slot(normal) {
        ObjTextureSlot::Up => "up",
        ObjTextureSlot::Down => "down",
        ObjTextureSlot::Side => "side",
    }
}

#[must_use]
pub fn obj_material_name_for_block(block: &str) -> String {
    let mut material = String::with_capacity(block.len().saturating_add(8));
    for character in block.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
            material.push(character);
        } else {
            material.push('_');
        }
    }
    let material = material.trim_matches('_');
    if material.is_empty() {
        FALLBACK_MATERIAL_NAME.to_owned()
    } else {
        material.to_owned()
    }
}

#[must_use]
pub fn block_export_material_name_for_block(block: &str) -> String {
    obj_material_name_for_block(block)
}

#[must_use]
pub fn obj_material_name_for_face(base: &str, normal: [i32; 3]) -> String {
    format!("{base}_{}", obj_face_texture_slot_suffix(normal))
}

#[must_use]
pub fn block_export_material_name_for_face(base: &str, normal: [i32; 3]) -> String {
    obj_material_name_for_face(base, normal)
}

#[must_use]
pub fn obj_material_name_for_slot(base: &str, slot: &str) -> String {
    format!(
        "{base}{MATERIAL_SLOT_SEPARATOR}{}",
        obj_material_slot_component(slot)
    )
}

#[must_use]
pub fn block_export_material_name_for_slot(base: &str, slot: &str) -> String {
    obj_material_name_for_slot(base, slot)
}

#[must_use]
pub fn block_export_material_name_for_plane(
    base: &str,
    normal: [i32; 3],
    material_slot: Option<&str>,
) -> String {
    if let Some(slot) = material_slot {
        block_export_material_name_for_slot(base, slot)
    } else {
        block_export_material_name_for_face(base, normal)
    }
}

#[must_use]
pub fn obj_material_slot_component(slot: &str) -> String {
    let slot = slot.trim();
    if slot == "*" {
        return "default".to_string();
    }
    let mut component = String::with_capacity(slot.len());
    for character in slot.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
            component.push(character);
        } else {
            component.push('_');
        }
    }
    let component = component.trim_matches('_');
    if component.is_empty() {
        "default".to_string()
    } else {
        component.to_string()
    }
}

fn obj_face_from_mesh_face<'a>(face: ObjMeshFace<'a>) -> ObjFace<'a> {
    ObjFace {
        material: face.material,
        positions: [
            face.triangle_positions[0],
            face.triangle_positions[1],
            face.triangle_positions[2],
            face.triangle_positions[5],
        ],
        uv: face.uv,
    }
}

fn obj_material_sample_from_mesh_face(face: ObjMeshFace<'_>) -> ObjMaterialSample<'static> {
    let normal = obj_face_normal_from_triangle(
        face.triangle_positions[0],
        face.triangle_positions[1],
        face.triangle_positions[2],
    )
    .unwrap_or([0, 1, 0]);
    ObjMaterialSample {
        name: Cow::Owned(face.material.into_owned()),
        color: face.color,
        normal,
    }
}

fn push_obj_vertex(output: &mut String, position: [f32; 3]) {
    let _ = writeln!(
        output,
        "v {:.6} {:.6} {:.6}",
        position[0], position[1], position[2]
    );
}

fn push_obj_texcoords(output: &mut String, uv: &[[f32; 2]; 4]) {
    for [u, v] in uv {
        let _ = writeln!(output, "vt {:.6} {:.6}", u.max(0.0), v.max(0.0));
    }
}

fn push_obj_quad_face(output: &mut String, a: usize, b: usize, c: usize, d: usize) {
    let _ = writeln!(output, "f {a}/{a} {b}/{b} {c}/{c} {d}/{d}");
}

fn obj_vec3_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    obj_vec3_length_squared(obj_vec3_sub(a, b)).sqrt()
}

fn obj_vec3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn obj_vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn obj_vec3_length_squared(value: [f32; 3]) -> f32 {
    value[0].mul_add(value[0], value[1].mul_add(value[1], value[2] * value[2]))
}

fn obj_vec3_normalize(value: [f32; 3]) -> [f32; 3] {
    let length = obj_vec3_length_squared(value).sqrt().max(0.0001);
    [value[0] / length, value[1] / length, value[2] / length]
}

#[must_use]
pub fn obj_alpha_texture_path(relative_texture_path: &str) -> String {
    let slash_index = relative_texture_path
        .rfind('/')
        .map_or(0, |index| index + 1);
    let file_name = &relative_texture_path[slash_index..];
    let extension_index = file_name
        .rfind('.')
        .map(|index| slash_index + index)
        .unwrap_or(relative_texture_path.len());
    let stem = &relative_texture_path[..extension_index];
    let extension = &relative_texture_path[extension_index..];
    if stem.ends_with("_alpha") {
        relative_texture_path.to_owned()
    } else {
        format!("{stem}_alpha{extension}")
    }
}

#[must_use]
pub fn obj_material_uses_preview_tint(material_name: &str) -> bool {
    let block = obj_block_texture_name(material_name);
    is_water_block(&block)
        || is_lava_block(&block)
        || block == "redstone_wire"
        || block == "portal"
        || block == "nether_portal"
        || block == "end_portal"
        || block == "glass"
        || block.contains("stained_glass")
        || block.contains("glass_pane")
        || block.contains("grate")
        || block.contains("grass")
        || block.contains("leaves")
        || block.contains("leaf")
        || block.contains("foliage")
}

#[must_use]
pub fn obj_material_texture_tint(material_name: &str, color: [f32; 4]) -> Option<[f32; 3]> {
    let block = obj_block_texture_name(material_name);
    if is_water_block(&block) {
        return Some([0.267, 0.686, 0.961]);
    }
    if !obj_material_needs_biome_tinted_texture(material_name) {
        return None;
    }
    let tint = [
        color[0].clamp(0.0, 1.0),
        color[1].clamp(0.0, 1.0),
        color[2].clamp(0.0, 1.0),
    ];
    if biome_tint_color_is_visible(tint) {
        Some(tint)
    } else if block.contains("leaves") || block.contains("leaf") || block.contains("foliage") {
        Some([0.29, 0.61, 0.20])
    } else {
        Some([0.48, 0.74, 0.32])
    }
}

fn obj_material_preview_dissolve(
    material_name: &str,
    color: [f32; 4],
    use_preview_color: bool,
) -> f32 {
    let block = obj_block_texture_name(material_name);
    if is_water_block(&block) {
        return 0.65;
    }
    if use_preview_color {
        color[3].clamp(0.0, 1.0)
    } else {
        1.0
    }
}

#[must_use]
pub fn obj_material_needs_biome_tinted_texture(material_name: &str) -> bool {
    let block = obj_block_texture_name(material_name);
    let texture_name = obj_material_texture_name(material_name);
    if is_full_grass_block(&block) {
        // Only the top face (grass_up) gets biome tint.
        // Side faces use grass_side_carried which already has blended color baked in.
        // Bottom face (dirt) is never tinted.
        return texture_name.ends_with("_up")
            || obj_material_instance_slot(material_name)
                .as_deref()
                .is_some_and(is_top_material_slot);
    }
    is_biome_tinted_plant_or_foliage(&block)
}

#[must_use]
pub fn obj_material_uses_texture_alpha(material_name: &str) -> bool {
    let block = obj_block_texture_name(material_name);
    let texture_name = obj_material_texture_name(material_name);
    if is_full_grass_block(&block) {
        return texture_name.ends_with("_side")
            || texture_name.ends_with("_side_carried")
            || texture_name.ends_with("_snowed")
            || obj_material_instance_slot(material_name)
                .as_deref()
                .is_some_and(is_side_material_slot);
    }
    if is_foliage_block(&block) {
        return !texture_name.ends_with("_opaque");
    }
    if is_cross_plant(&block) || is_transparent_detail_block(&block) {
        return true;
    }
    if block == "web" || block == "cobweb" || block == "redstone_wire" {
        return true;
    }
    block == "iron_bars"
        || block == "glass"
        || block == "glass_pane"
        || block.contains("stained_glass")
        || block.contains("glass_pane")
        || block == "portal"
        || block == "nether_portal"
        || block == "end_portal"
        || block.contains("grass")
        || block.contains("flower")
        || block.contains("sapling")
        || block.contains("fern")
        || block.contains("vine")
        || block.contains("coral")
        || block.contains("kelp")
        || block.contains("seagrass")
        || block.contains("mushroom")
        || block.contains("torch")
        || block.contains("lantern")
        || block.contains("candle")
        || block.contains("ladder")
        || block.contains("door")
        || block.contains("trapdoor")
        || block.contains("rail")
        || block.contains("chain")
        || block.contains("bars")
        || block.contains("pane")
        || block.contains("grate")
        || block.contains("fence")
        || block.contains("gate")
        || block.contains("scaffold")
        || is_water_block(&block)
        || is_lava_block(&block)
        || block.contains("banner")
        || block.contains("sign")
}

#[must_use]
pub fn obj_block_texture_name(material: &str) -> String {
    let material_base = obj_material_base(material);
    let material = material_base
        .strip_prefix("minecraft_")
        .unwrap_or(material_base)
        .trim_end_matches("_up")
        .trim_end_matches("_down")
        .trim_end_matches("_side")
        .trim_end_matches("_north")
        .trim_end_matches("_south")
        .trim_end_matches("_east")
        .trim_end_matches("_west")
        .replace('-', "_");
    if let Some(stripped) = material.strip_prefix("minecraft:") {
        stripped.to_owned()
    } else {
        material
    }
}

#[must_use]
pub fn obj_material_texture_name(material: &str) -> String {
    let material_base = obj_material_base(material);
    material_base
        .strip_prefix("minecraft_")
        .unwrap_or(material_base)
        .replace('-', "_")
}

#[must_use]
pub fn obj_material_base(material: &str) -> &str {
    material
        .split_once(MATERIAL_SLOT_SEPARATOR)
        .map_or(material, |(base, _)| base)
}

#[must_use]
pub fn obj_block_identifier(block: &str) -> String {
    if block.contains(':') {
        block.to_owned()
    } else {
        format!("minecraft:{block}")
    }
}

#[must_use]
pub fn obj_canonical_block_lookup_name(block: &str) -> Cow<'_, str> {
    match block {
        "grass_block" => Cow::Borrowed("grass"),
        "flowing_water" | "water_still" | "water_flow" | "still_water" | "flowing_water_grey"
        | "still_water_grey" => Cow::Borrowed("water"),
        "flowing_lava" | "lava_still" | "lava_flow" | "still_lava" => Cow::Borrowed("lava"),
        "short_grass" => Cow::Borrowed("tallgrass"),
        _ => Cow::Borrowed(block),
    }
}

#[must_use]
pub fn obj_material_instance_slot(material: &str) -> Option<Cow<'_, str>> {
    let (_, slot) = material.split_once(MATERIAL_SLOT_SEPARATOR)?;
    let slot = slot.trim();
    if slot.is_empty() {
        None
    } else if slot == "default" {
        Some(Cow::Borrowed("*"))
    } else {
        Some(Cow::Borrowed(slot))
    }
}

#[must_use]
pub fn obj_material_block_face(material: &str) -> Option<BlockFace> {
    let normalized = obj_material_texture_name(material);
    if normalized.ends_with("_up") {
        Some(BlockFace::Up)
    } else if normalized.ends_with("_down") {
        Some(BlockFace::Down)
    } else if normalized.ends_with("_north") {
        Some(BlockFace::North)
    } else if normalized.ends_with("_south") {
        Some(BlockFace::South)
    } else if normalized.ends_with("_east") {
        Some(BlockFace::East)
    } else if normalized.ends_with("_west") {
        Some(BlockFace::West)
    } else if normalized.ends_with("_side") {
        Some(BlockFace::Side)
    } else {
        None
    }
}

#[must_use]
pub fn obj_block_face_for_normal(normal: [i32; 3]) -> BlockFace {
    match normal {
        [0, 1, 0] => BlockFace::Up,
        [0, -1, 0] => BlockFace::Down,
        [1, 0, 0] => BlockFace::East,
        [-1, 0, 0] => BlockFace::West,
        [0, 0, 1] => BlockFace::South,
        [0, 0, -1] => BlockFace::North,
        _ => BlockFace::Default,
    }
}

#[must_use]
pub fn block_face_for_normal(normal: [i32; 3]) -> BlockFace {
    obj_block_face_for_normal(normal)
}

#[must_use]
pub fn obj_normal_for_block_face(face: BlockFace) -> [i32; 3] {
    match face {
        BlockFace::Up => [0, 1, 0],
        BlockFace::Down => [0, -1, 0],
        BlockFace::East => [1, 0, 0],
        BlockFace::West => [-1, 0, 0],
        BlockFace::South | BlockFace::Side | BlockFace::All | BlockFace::Default => [0, 0, 1],
        BlockFace::North => [0, 0, -1],
    }
}

#[must_use]
pub fn obj_texture_faces_for_block_face(face: BlockFace) -> &'static [BlockFace] {
    match face {
        BlockFace::Up => &[BlockFace::Up, BlockFace::All, BlockFace::Default],
        BlockFace::Down => &[BlockFace::Down, BlockFace::All, BlockFace::Default],
        BlockFace::North => &[
            BlockFace::North,
            BlockFace::Side,
            BlockFace::All,
            BlockFace::Default,
        ],
        BlockFace::South => &[
            BlockFace::South,
            BlockFace::Side,
            BlockFace::All,
            BlockFace::Default,
        ],
        BlockFace::East => &[
            BlockFace::East,
            BlockFace::Side,
            BlockFace::All,
            BlockFace::Default,
        ],
        BlockFace::West => &[
            BlockFace::West,
            BlockFace::Side,
            BlockFace::All,
            BlockFace::Default,
        ],
        BlockFace::Side => &[BlockFace::Side, BlockFace::All, BlockFace::Default],
        BlockFace::All => &[BlockFace::All, BlockFace::Default],
        BlockFace::Default => &[BlockFace::Default, BlockFace::All],
    }
}

#[must_use]
pub fn obj_material_slots_for_block_face(face: BlockFace) -> &'static [&'static str] {
    match face {
        BlockFace::Up => &["up", "*"],
        BlockFace::Down => &["down", "*"],
        BlockFace::North => &["north", "side", "*"],
        BlockFace::South => &["south", "side", "*"],
        BlockFace::East => &["east", "side", "*"],
        BlockFace::West => &["west", "side", "*"],
        BlockFace::Side => &["side", "*"],
        BlockFace::All | BlockFace::Default => &["*"],
    }
}

#[must_use]
pub fn obj_material_slot_candidates(slot: &str) -> [&str; 2] {
    if slot == "default" {
        ["*", "default"]
    } else {
        [slot, "*"]
    }
}

fn obj_material_slot_candidate_list(slot: &str) -> Vec<&str> {
    match slot {
        "default" => vec!["*", "default"],
        "front" => vec!["front", "south", "north", "side", "*"],
        "back" => vec!["back", "north", "south", "side", "*"],
        "top" => vec!["top", "up", "*"],
        "bottom" => vec!["bottom", "down", "*"],
        "up" => vec!["up", "top", "*"],
        "down" => vec!["down", "bottom", "*"],
        "north" | "south" | "east" | "west" => vec![slot, "side", "*"],
        "side" => vec!["side", "north", "south", "east", "west", "*"],
        _ => vec![slot, "*"],
    }
}

fn material_slot_block_face(slot: &str) -> Option<BlockFace> {
    let face = BlockFace::parse(slot);
    if matches!(face, BlockFace::Default | BlockFace::All) {
        None
    } else {
        Some(face)
    }
}

#[must_use]
pub fn candle_block_for_cake(block: &str) -> Option<String> {
    if block == "candle_cake" {
        return Some("candle".to_owned());
    }
    block
        .strip_suffix("_candle_cake")
        .map(|color| format!("{color}_candle"))
}

#[must_use]
pub fn obj_texture_key_from_value(value: &Value, normal: [i32; 3]) -> Option<String> {
    match value {
        Value::String(text) => Some(obj_normalize_texture_key(text)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| obj_texture_key_from_value(value, normal)),
        Value::Object(object) => {
            let keys = obj_texture_value_keys_for_normal(normal);
            keys.iter()
                .find_map(|key| object.get(*key))
                .and_then(|value| obj_texture_key_from_value(value, normal))
                .or_else(|| {
                    object
                        .values()
                        .find_map(|value| obj_texture_key_from_value(value, normal))
                })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn obj_texture_value_keys_for_normal(normal: [i32; 3]) -> &'static [&'static str] {
    match obj_block_face_for_normal(normal) {
        BlockFace::Up => &["up", "top", "all", "side", "default"],
        BlockFace::Down => &["down", "bottom", "all", "side", "default"],
        BlockFace::North => &["north", "side", "all", "default"],
        BlockFace::South => &["south", "side", "all", "default"],
        BlockFace::East => &["east", "side", "all", "default"],
        BlockFace::West => &["west", "side", "all", "default"],
        BlockFace::Side => &["side", "north", "south", "east", "west", "all", "default"],
        BlockFace::All | BlockFace::Default => {
            &["all", "default", "side", "north", "south", "east", "west"]
        }
    }
}

#[must_use]
pub fn obj_fallback_texture_keys(
    material_texture: &str,
    block: &str,
    normal: [i32; 3],
) -> Vec<String> {
    let mut keys = Vec::with_capacity(24);
    if is_water_block(block) {
        for key in [
            "still_water",
            "flowing_water",
            "water_still",
            "water_flow",
            "still_water_grey",
            "flowing_water_grey",
        ] {
            push_texture_key_aliases(&mut keys, key);
        }
    } else if is_lava_block(block) {
        for key in ["still_lava", "flowing_lava", "lava_still", "lava_flow"] {
            push_texture_key_aliases(&mut keys, key);
        }
    } else if is_full_grass_block(block) {
        match obj_face_texture_slot(normal) {
            ObjTextureSlot::Up => {
                for key in ["grass_top", "grass"] {
                    push_texture_key_aliases(&mut keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["grass_bottom", "dirt", "grass"] {
                    push_texture_key_aliases(&mut keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["grass_side_carried", "grass_side", "grass_carried", "grass"] {
                    push_texture_key_aliases(&mut keys, key);
                }
            }
        }
    }
    push_block_specific_texture_aliases(&mut keys, block, normal);
    push_texture_key_aliases(&mut keys, block);
    match obj_face_texture_slot(normal) {
        ObjTextureSlot::Up => {
            push_texture_key_aliases(&mut keys, format!("{block}_top"));
            push_texture_key_aliases(&mut keys, format!("{block}_up"));
        }
        ObjTextureSlot::Down => {
            push_texture_key_aliases(&mut keys, format!("{block}_bottom"));
            push_texture_key_aliases(&mut keys, format!("{block}_down"));
        }
        ObjTextureSlot::Side => {
            push_texture_key_aliases(&mut keys, format!("{block}_side"));
        }
    }
    push_texture_key_aliases(&mut keys, material_texture);
    keys
}

#[must_use]
pub fn path_starts_with_directory(path: &str, directory: &str) -> bool {
    let directory = directory.trim_matches('/');
    if directory.is_empty() {
        return false;
    }
    path.eq_ignore_ascii_case(directory)
        || path
            .get(..directory.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(directory))
            && path.as_bytes().get(directory.len()) == Some(&b'/')
}

#[must_use]
pub fn prefixed_relative_path(texture_directory_name: &str, path: &Path) -> String {
    relative_path_string(path)
        .map(|path| format!("{texture_directory_name}/{path}"))
        .unwrap_or_else(|| format!("{texture_directory_name}/texture.png"))
}

#[must_use]
pub fn relative_path_string(path: &Path) -> Option<String> {
    let path = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    (!path.is_empty()).then_some(path)
}

#[must_use]
pub fn path_extension_eq(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

#[must_use]
pub fn obj_path_extension_eq(path: &str, extension: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, value)| value.eq_ignore_ascii_case(extension))
}

#[must_use]
pub fn vanilla_resource_pack_roots(package_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique_path(&mut roots, package_path.to_path_buf());
    if path_file_name_eq(package_path, "textures")
        && let Some(parent) = package_path.parent()
    {
        push_unique_path(&mut roots, parent.to_path_buf());
    }
    push_vanilla_resource_pack_roots(
        &mut roots,
        &package_path.join("data").join("resource_packs"),
    );
    push_vanilla_resource_pack_roots(&mut roots, &package_path.join("data").join("resourcepacks"));
    roots
}

#[must_use]
pub fn vanilla_resource_pack_roots_from_packages<I, P>(package_roots: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut roots = Vec::new();
    for package_root in package_roots {
        for root in vanilla_resource_pack_roots(package_root.as_ref()) {
            push_unique_path(&mut roots, root);
        }
    }
    roots
}

#[must_use]
pub fn world_resource_pack_paths(world_path: &Path) -> Vec<PathBuf> {
    let pack_ids = world_resource_pack_ids(world_path);
    if pack_ids.is_empty() {
        return Vec::new();
    }

    let resource_pack_roots = resource_pack_roots_for_world(world_path);
    let mut package_paths = Vec::new();
    for pack_id in pack_ids {
        for root in &resource_pack_roots {
            if let Some(package_path) = find_resource_pack_path(root, &pack_id) {
                push_unique_resource_pack_path(&mut package_paths, package_path);
                break;
            }
        }
    }
    package_paths
}

#[must_use]
pub fn world_resource_pack_ids(world_path: &Path) -> Vec<String> {
    let Some(value) = read_optional_json(&world_path.join("world_resource_packs.json")) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    let mut pack_ids = Vec::new();
    for item in items {
        let Some(pack_id) = item
            .get("pack_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("uuid").and_then(Value::as_str))
        else {
            continue;
        };
        let normalized = normalize_pack_uuid(pack_id);
        if !normalized.is_empty() && !pack_ids.iter().any(|existing| existing == &normalized) {
            pack_ids.push(normalized);
        }
    }
    pack_ids
}

#[must_use]
pub fn resource_pack_roots_for_world(world_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique_resource_pack_path(&mut roots, world_path.join("resource_packs"));
    for ancestor in world_path.ancestors() {
        if !path_file_name_eq(ancestor, "minecraftWorlds") {
            continue;
        }
        let Some(com_mojang_dir) = ancestor.parent() else {
            continue;
        };
        if !path_file_name_eq(com_mojang_dir, "com.mojang") {
            continue;
        }
        push_unique_resource_pack_path(&mut roots, com_mojang_dir.join("resource_packs"));

        let Some(games_dir) = com_mojang_dir.parent() else {
            continue;
        };
        if !path_file_name_eq(games_dir, "games") {
            continue;
        }
        let Some(user_dir) = games_dir.parent() else {
            continue;
        };
        let Some(users_dir) = user_dir
            .parent()
            .filter(|path| path_file_name_eq(path, "Users"))
        else {
            continue;
        };
        push_unique_resource_pack_path(
            &mut roots,
            users_dir
                .join("Shared")
                .join("games")
                .join("com.mojang")
                .join("resource_packs"),
        );
    }
    roots
}

#[must_use]
pub fn find_resource_pack_path(root: &Path, pack_id: &str) -> Option<PathBuf> {
    let pack_id = normalize_pack_uuid(pack_id);
    if pack_id.is_empty() || !root.is_dir() {
        return None;
    }

    for candidate_name in [
        format!("pack_{pack_id}"),
        pack_id.clone(),
        format!("pack_{}", pack_id.to_ascii_lowercase()),
        pack_id.to_ascii_lowercase(),
    ] {
        let candidate = root.join(candidate_name);
        if resource_pack_candidate_matches(&candidate, &pack_id, true) {
            return Some(candidate);
        }
    }

    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if resource_pack_candidate_matches(&path, &pack_id, false) {
            return Some(path);
        }
    }
    None
}

#[must_use]
pub fn resource_pack_manifest_uuid(path: &Path) -> Option<String> {
    let value = read_optional_json(&path.join("manifest.json"))?;
    value
        .get("header")
        .and_then(|header| header.get("uuid"))
        .and_then(Value::as_str)
        .map(normalize_pack_uuid)
}

#[must_use]
pub fn normalize_pack_uuid(value: &str) -> String {
    value
        .trim()
        .trim_matches('{')
        .trim_matches('}')
        .to_ascii_lowercase()
}

pub fn push_unique_resource_pack_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }
    push_unique_path(paths, path);
}

#[must_use]
pub fn replace_path_extension(path: &str, extension: &str) -> String {
    let dot_index = path.rfind('.');
    let slash_index = path.rfind('/');
    if let Some(dot_index) = dot_index
        && slash_index.is_none_or(|slash_index| dot_index > slash_index)
    {
        return format!("{}.{extension}", &path[..dot_index]);
    }
    format!("{path}.{extension}")
}

#[must_use]
pub fn obj_normalize_texture_key(value: &str) -> String {
    let mut value = value.trim().replace('\\', "/");
    if value
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("textures/"))
    {
        value.drain(..9);
    }
    for extension in [".png", ".tga", ".jpg", ".jpeg"] {
        if value
            .get(value.len().saturating_sub(extension.len())..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(extension))
        {
            value.truncate(value.len().saturating_sub(extension.len()));
            break;
        }
    }
    value
}

#[must_use]
pub fn texture_candidate_relatives(texture_path: &str) -> Vec<String> {
    let mut paths = Vec::with_capacity(3);
    push_unique_string(&mut paths, texture_path.to_owned());
    if !texture_path.starts_with("blocks/") {
        push_unique_string(&mut paths, format!("blocks/{texture_path}"));
    }
    if !texture_path.starts_with("textures/") {
        push_unique_string(&mut paths, format!("textures/{texture_path}"));
    }
    paths
}

#[must_use]
pub fn find_texture_file(root: &Path, relative_path: &str) -> Option<PathBuf> {
    let path = root.join(relative_path);
    if path.is_file() {
        return Some(path);
    }
    for extension in ["png", "tga", "jpg", "jpeg"] {
        let path = root.join(format!("{relative_path}.{extension}"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn is_biome_tinted_plant_or_foliage(normalized: &str) -> bool {
    is_grass_tinted_detail_block(normalized) || is_foliage_block(normalized)
}

fn is_grass_tinted_detail_block(normalized: &str) -> bool {
    matches!(
        normalized,
        "short_grass"
            | "tall_grass"
            | "tallgrass"
            | "fern"
            | "large_fern"
            | "vine"
            | "twisting_vines"
            | "weeping_vines"
            | "seagrass"
            | "tall_seagrass"
            | "kelp"
            | "kelp_plant"
    ) || normalized.contains("grass")
        || normalized.contains("fern")
        || normalized.contains("vine")
}

fn biome_tint_color_is_visible(color: [f32; 3]) -> bool {
    let luminance = color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722;
    color[1] > color[0] * 0.85 && color[1] > color[2] * 0.75 && luminance < 0.92
}

fn is_transparent_detail_block(normalized: &str) -> bool {
    let transparent_exact = [
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "deadbush",
        "vine",
        "twisting_vines",
        "weeping_vines",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "snow_layer",
        "tripwire",
        "chain",
        "redstone_wire",
        "web",
        "cobweb",
        "copper_grate",
        "exposed_copper_grate",
        "weathered_copper_grate",
        "oxidized_copper_grate",
        "waxed_copper_grate",
        "waxed_exposed_copper_grate",
        "waxed_weathered_copper_grate",
        "waxed_oxidized_copper_grate",
    ];
    if transparent_exact.contains(&normalized) {
        return true;
    }

    let transparent_suffixes = [
        "sapling",
        "flower",
        "mushroom",
        "torch",
        "rail",
        "button",
        "pressure_plate",
        "carpet",
        "ladder",
        "door",
        "trapdoor",
        "fence",
        "wall",
        "pane",
        "grate",
        "sign",
        "banner",
        "coral",
        "candle",
        "lantern",
    ];

    transparent_suffixes
        .iter()
        .any(|suffix| normalized == *suffix || normalized.ends_with(&format!("_{suffix}")))
}

fn is_cross_plant(normalized: &str) -> bool {
    let plant_exact = [
        "yellow_flower",
        "red_flower",
        "double_plant",
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "deadbush",
        "vine",
        "twisting_vines",
        "weeping_vines",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "dandelion",
        "poppy",
        "blue_orchid",
        "allium",
        "azure_bluet",
        "red_tulip",
        "orange_tulip",
        "white_tulip",
        "pink_tulip",
        "oxeye_daisy",
        "cornflower",
        "lily_of_the_valley",
        "wither_rose",
        "sunflower",
        "lilac",
        "rose_bush",
        "peony",
        "torchflower",
        "pitcher_plant",
        "web",
        "cobweb",
        "wheat",
        "carrots",
        "potatoes",
        "beetroot",
        "beetroots",
        "nether_wart",
        "sweet_berry_bush",
    ];
    plant_exact.contains(&normalized)
        || normalized.ends_with("_sapling")
        || normalized.ends_with("_flower")
        || normalized.ends_with("_crop")
        || normalized.ends_with("_stem")
        || normalized == "flower"
        || normalized.ends_with("_mushroom")
        || normalized == "mushroom"
        || normalized.ends_with("_coral")
        || normalized == "coral"
}

fn is_full_grass_block(normalized: &str) -> bool {
    normalized == "grass" || normalized == "grass_block" || normalized.ends_with("_grass_block")
}

fn is_top_material_slot(slot: &str) -> bool {
    matches!(slot, "up" | "top")
}

fn is_side_material_slot(slot: &str) -> bool {
    matches!(slot, "north" | "south" | "east" | "west" | "side")
}

fn is_water_block(normalized: &str) -> bool {
    matches!(
        normalized,
        "water"
            | "flowing_water"
            | "water_still"
            | "water_flow"
            | "still_water"
            | "flowing_water_grey"
            | "still_water_grey"
    )
}

fn is_lava_block(normalized: &str) -> bool {
    matches!(
        normalized,
        "lava" | "flowing_lava" | "lava_still" | "lava_flow" | "still_lava"
    )
}

fn is_foliage_block(normalized: &str) -> bool {
    if normalized.contains("leaf_litter") {
        return false;
    }
    normalized == "leaves"
        || normalized.ends_with("_leaves")
        || normalized.contains("leaves")
        || normalized.ends_with("_leaf")
        || normalized.contains("foliage")
}

fn grass_block_texture_key_for_face(face: BlockFace) -> Option<String> {
    match face {
        BlockFace::Up => Some("grass_top".to_owned()),
        BlockFace::Down => Some("dirt".to_owned()),
        BlockFace::North
        | BlockFace::South
        | BlockFace::East
        | BlockFace::West
        | BlockFace::Side => Some("grass_side_carried".to_owned()),
        BlockFace::All | BlockFace::Default => None,
    }
}

fn obj_face_texture_slot(normal: [i32; 3]) -> ObjTextureSlot {
    if normal[1] > 0 {
        ObjTextureSlot::Up
    } else if normal[1] < 0 {
        ObjTextureSlot::Down
    } else {
        ObjTextureSlot::Side
    }
}

fn block_definition<'a>(blocks: &'a Value, block: &str) -> Option<&'a Value> {
    let mut keys = Vec::with_capacity(6);
    push_unique_string(&mut keys, block.to_owned());
    if !block.contains(':') {
        push_unique_string(&mut keys, format!("minecraft:{block}"));
    }
    if let Some(stripped) = block.strip_suffix("_block") {
        push_unique_string(&mut keys, stripped.to_owned());
        push_unique_string(&mut keys, format!("minecraft:{stripped}"));
    }
    if block == "grass_block" {
        push_unique_string(&mut keys, "grass".to_owned());
        push_unique_string(&mut keys, "minecraft:grass".to_owned());
    }

    for key in &keys {
        if let Some(value) = blocks.get(key).or_else(|| {
            blocks
                .get("blocks")
                .and_then(|blocks| blocks.get(key.as_str()))
        }) {
            return Some(value);
        }
    }
    None
}

fn push_vanilla_resource_pack_roots(roots: &mut Vec<PathBuf>, resource_packs_dir: &Path) {
    let vanilla = resource_packs_dir.join("vanilla");
    push_unique_path(roots, vanilla.join("client"));
    push_unique_path(roots, vanilla);

    let Ok(entries) = std::fs::read_dir(resource_packs_dir) else {
        return;
    };
    let mut vanilla_overlay_roots = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_vanilla_resource_pack_overlay_name)
        })
        .collect::<Vec<_>>();
    vanilla_overlay_roots.sort_by(|left, right| {
        let left_version = left
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(vanilla_resource_pack_overlay_version);
        let right_version = right
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(vanilla_resource_pack_overlay_version);
        right_version
            .cmp(&left_version)
            .then_with(|| left.cmp(right))
    });

    for root in vanilla_overlay_roots {
        push_unique_path(roots, root.join("client"));
        push_unique_path(roots, root);
    }
}

fn resource_pack_candidate_matches(path: &Path, pack_id: &str, allow_name_fallback: bool) -> bool {
    if !path.join("textures").is_dir() {
        return false;
    }
    if let Some(manifest_uuid) = resource_pack_manifest_uuid(path) {
        return manifest_uuid == pack_id;
    }
    allow_name_fallback
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn path_file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn is_vanilla_resource_pack_overlay_name(name: &str) -> bool {
    vanilla_resource_pack_overlay_version(name).is_some()
}

fn vanilla_resource_pack_overlay_version(name: &str) -> Option<Vec<u32>> {
    let version = name.strip_prefix("vanilla_").or_else(|| {
        name.get(..8)
            .filter(|prefix| prefix.eq_ignore_ascii_case("vanilla_"))
            .and_then(|_| name.get(8..))
    })?;
    let mut parts = Vec::new();
    for part in version.split('.') {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        parts.push(part.parse().ok()?);
    }
    (!parts.is_empty()).then_some(parts)
}

fn read_optional_json(path: &Path) -> Option<Value> {
    read_json_file(path).ok()
}

fn read_texture_data_maps(root: &Path, texture_root: &Path) -> HashMap<String, String> {
    let mut textures = HashMap::new();
    for path in texture_data_json_paths(root, texture_root) {
        merge_texture_data_map(&mut textures, &path);
    }
    textures
}

fn texture_data_json_paths(root: &Path, texture_root: &Path) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    paths.insert(root.join("terrain_texture.json"));
    paths.insert(texture_root.join("terrain_texture.json"));
    collect_direct_json_paths(root, &mut paths);
    collect_direct_json_paths(texture_root, &mut paths);
    paths
}

fn collect_direct_json_paths(root: &Path, paths: &mut BTreeSet<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                && path
                    .file_name()
                    .and_then(|file_name| file_name.to_str())
                    .is_some_and(|file_name| {
                        file_name.to_ascii_lowercase().contains("terrain_texture")
                    })
            {
                paths.insert(path);
            }
        }
    }
}

fn merge_texture_data_map(textures: &mut HashMap<String, String>, path: &Path) {
    let Some(value) = read_optional_json(path) else {
        return;
    };
    let Some(texture_data) = value.get("texture_data").and_then(Value::as_object) else {
        return;
    };
    for (key, value) in texture_data {
        if let Some(path) = terrain_texture_path(value) {
            let path = obj_normalize_texture_key(&path);
            textures.entry(key.clone()).or_insert(path);
        }
    }
}

fn terrain_texture_path(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    if let Some(text) = value.get("path").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    let textures = value.get("textures").or_else(|| value.get("texture"))?;
    match textures {
        Value::String(text) => Some(text.to_owned()),
        Value::Array(values) => values.iter().find_map(terrain_texture_path),
        Value::Object(object) => object
            .get("path")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| object.values().find_map(terrain_texture_path)),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn push_block_specific_texture_aliases(keys: &mut Vec<String>, block: &str, normal: [i32; 3]) {
    if let Some(color) = block.strip_suffix("_wool") {
        for key in [
            format!("wool_colored_{color}"),
            format!("{color}_wool"),
            format!("wool_{color}"),
            format!("blocks/wool_colored_{color}"),
        ] {
            push_texture_key_aliases(keys, key);
        }
        return;
    }
    if block == "wool" {
        for key in ["wool_colored_white", "white_wool", "wool"] {
            push_texture_key_aliases(keys, key);
        }
        return;
    }

    let texture_slot = obj_face_texture_slot(normal);
    if block == "bricks" {
        for key in ["brick_block", "bricks", "brick"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "stone_bricks" {
        for key in ["stonebrick", "stone_brick", "stone_bricks"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "mossy_stone_bricks" {
        for key in [
            "stonebrick_mossy",
            "mossy_stone_brick",
            "mossy_stone_bricks",
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "nether_bricks" {
        for key in ["nether_brick", "nether_bricks"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "red_nether_bricks" {
        for key in ["red_nether_brick", "red_nether_bricks"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "end_bricks" || block == "end_stone_bricks" || block == "end_stone_brick" {
        for key in [
            "end_bricks",
            "end_brick",
            "end_stone_bricks",
            "end_stone_brick",
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if let Some(wood) = block.strip_suffix("_trapdoor") {
        for key in [format!("{wood}_trapdoor"), format!("trapdoor_{wood}")] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "fence" || block == "fence_gate" {
        push_wood_planks_texture_aliases(keys, "oak");
    }
    if let Some(wood) = block.strip_suffix("_fence_gate") {
        push_wood_planks_texture_aliases(keys, wood);
    }
    if block == "nether_brick_fence" {
        for key in ["nether_brick", "nether_bricks"] {
            push_texture_key_aliases(keys, key);
        }
    } else if let Some(wood) = block.strip_suffix("_fence") {
        push_wood_planks_texture_aliases(keys, wood);
    }
    if let Some(wood) = block.strip_suffix("_planks") {
        push_wood_planks_texture_aliases(keys, wood);
    }
    if let Some(color) = block.strip_suffix("_stained_glass") {
        for key in [format!("glass_{color}"), format!("stained_glass_{color}")] {
            push_texture_key_aliases(keys, key);
        }
    }
    if let Some(color) = block.strip_suffix("_stained_glass_pane") {
        for key in [
            format!("glass_pane_top_{color}"),
            format!("glass_{color}"),
            format!("stained_glass_{color}"),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "iron_bars" {
        for key in ["iron_bars", "ironbars"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "web" || block == "cobweb" {
        for key in ["web", "cobweb"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "redstone_wire" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["redstone_dust_cross", "redstone_dust_dot", "redstone_wire"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down | ObjTextureSlot::Side => {
                for key in ["redstone_dust_line", "redstone_dust_line0", "redstone_wire"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "hopper" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["hopper_top", "hopper_inside", "hopper"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["hopper_inside", "hopper_top", "hopper"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["hopper_outside", "hopper_side", "hopper"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "chest" || block == "trapped_chest" || block.ends_with("_chest") {
        for key in chest_texture_aliases(block, texture_slot) {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "standing_sign"
        || block == "wall_sign"
        || block == "hanging_sign"
        || block.ends_with("_standing_sign")
        || block.ends_with("_wall_sign")
        || block.ends_with("_hanging_sign")
    {
        for key in sign_texture_aliases(block) {
            push_texture_key_aliases(keys, key);
        }
    }
    if let Some(texture_key) = copper_golem_texture_key(block) {
        for key in [
            format!("entity/copper_golem/{texture_key}"),
            format!("copper_golem/{texture_key}"),
            texture_key.to_owned(),
            block.to_owned(),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "torch" || block == "wall_torch" {
        for key in ["torch_on", "torch"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "soul_torch" || block == "soul_wall_torch" {
        for key in ["soul_torch", "torch_on", "torch"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "redstone_torch" || block == "redstone_wall_torch" {
        for key in ["redstone_torch_on", "redstone_torch"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "unlit_redstone_torch" || block == "unlit_redstone_wall_torch" {
        for key in ["redstone_torch_off", "redstone_torch"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "anvil" || block == "chipped_anvil" || block == "damaged_anvil" {
        let (damage, top_key) = match block {
            "chipped_anvil" => ("1", "chipped_anvil_top"),
            "damaged_anvil" => ("2", "damaged_anvil_top"),
            _ => ("0", "flattened_anvil_top"),
        };
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in [
                    top_key.to_owned(),
                    format!("anvil_top_damaged_{damage}"),
                    "anvil_top".to_owned(),
                    block.to_owned(),
                ] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["anvil_base", "anvil_bottom", "anvil"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["anvil_side", "anvil"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "stonecutter" || block == "stonecutter_block" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in [
                    "stonecutter2_top",
                    "stonecutter_top",
                    "stonecutter_saw",
                    "stonecutter",
                ] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["stonecutter2_bottom", "stonecutter_bottom", "stonecutter"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in [
                    "stonecutter2_side",
                    "stonecutter2_saw",
                    "stonecutter_side",
                    "stonecutter",
                ] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "portal" || block == "nether_portal" {
        for key in ["portal", "nether_portal"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "end_portal" {
        for key in ["end_portal", "end_portal_frame_top"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "decorated_pot" {
        let suffix = match texture_slot {
            ObjTextureSlot::Up => "top",
            ObjTextureSlot::Down => "bottom",
            ObjTextureSlot::Side => "side",
        };
        for key in [
            format!("decorated_pot_{suffix}"),
            "decorated_pot_side".to_owned(),
            "decorated_pot".to_owned(),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "shulker_box" || block.ends_with("_shulker_box") {
        if let Some(color) = block.strip_suffix("_shulker_box") {
            for key in [
                format!("shulker_top_{color}"),
                format!("{color}_shulker_box"),
                format!("shulker_box_{color}"),
                format!("shulker_{color}"),
                format!("blocks/shulker_top_{color}"),
            ] {
                push_texture_key_aliases(keys, key);
            }
        }
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["shulker_top", "shulker_box_top", "shulker_box"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["shulker_bottom", "shulker_box_bottom", "shulker_box"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["shulker_side", "shulker_box_side", "shulker_box"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Barrel
    if block == "barrel" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["barrel_top", "barrel"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["barrel_bottom", "barrel_top", "barrel"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["barrel_side", "barrel"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Barrel open top mapping
    if block == "barrel" && texture_slot == ObjTextureSlot::Up {
        push_texture_key_aliases(keys, "barrel_top_open");
    }
    // Water
    if is_water_block(block) {
        for key in [
            "still_water",
            "flowing_water",
            "water_still",
            "water_flow",
            "still_water_grey",
            "flowing_water_grey",
            "water",
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Lava
    if is_lava_block(block) {
        for key in [
            "still_lava",
            "flowing_lava",
            "lava_still",
            "lava_flow",
            "lava",
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Crafting table
    if block == "crafting_table" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["crafting_table_top", "crafting_table"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in [
                    "crafting_table_side",
                    "crafting_table_front",
                    "crafting_table",
                ] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["planks_oak", "oak_planks", "crafting_table"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Furnace variants
    if block == "furnace" || block == "lit_furnace" {
        let is_lit = block == "lit_furnace";
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["furnace_top", "furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                if is_lit {
                    for key in [
                        "furnace_front_on",
                        "furnace_front",
                        "furnace_side",
                        "furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                } else {
                    for key in [
                        "furnace_front_off",
                        "furnace_front",
                        "furnace_side",
                        "furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                }
            }
            ObjTextureSlot::Down => {
                for key in ["furnace_top", "furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "smoker" || block == "lit_smoker" {
        let is_lit = block == "lit_smoker";
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["smoker_top", "smoker"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                if is_lit {
                    for key in ["smoker_front_on", "smoker_front", "smoker_side", "smoker"] {
                        push_texture_key_aliases(keys, key);
                    }
                } else {
                    for key in ["smoker_front_off", "smoker_front", "smoker_side", "smoker"] {
                        push_texture_key_aliases(keys, key);
                    }
                }
            }
            ObjTextureSlot::Down => {
                for key in ["smoker_bottom", "smoker"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "blast_furnace" || block == "lit_blast_furnace" {
        let is_lit = block == "lit_blast_furnace";
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["blast_furnace_top", "blast_furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                if is_lit {
                    for key in [
                        "blast_furnace_front_on",
                        "blast_furnace_front",
                        "blast_furnace_side",
                        "blast_furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                } else {
                    for key in [
                        "blast_furnace_front_off",
                        "blast_furnace_front",
                        "blast_furnace_side",
                        "blast_furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                }
            }
            ObjTextureSlot::Down => {
                for key in ["blast_furnace_top", "blast_furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Dispenser / Dropper
    if block == "dispenser" || block == "dropper" {
        let block_str = block.to_owned();
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, format!("{block_str}_top"));
                push_texture_key_aliases(keys, "furnace_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, format!("{block_str}_front_horizontal"));
                push_texture_key_aliases(keys, format!("{block_str}_front"));
                push_texture_key_aliases(keys, format!("{block_str}_side"));
                push_texture_key_aliases(keys, block);
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, format!("{block_str}_top"));
                push_texture_key_aliases(keys, block);
            }
        }
    }
    // Observer
    if block == "observer" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["observer_top", "observer"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["observer_front", "observer_side", "observer"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["observer_back", "observer_top", "observer"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Piston
    if block == "piston" || block == "sticky_piston" {
        let top_key = if block == "sticky_piston" {
            "piston_top_sticky"
        } else {
            "piston_top_normal"
        };
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in [top_key, "piston_top", "piston"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["piston_side", "piston"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["piston_bottom", "piston_side", "piston"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // TNT
    if block == "tnt" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["tnt_top", "tnt"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["tnt_bottom", "tnt"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["tnt_side", "tnt"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Cauldron
    if block == "cauldron" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["cauldron_inner", "cauldron_top", "cauldron"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["cauldron_bottom", "cauldron"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["cauldron_side", "cauldron"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Bookshelf
    if block == "bookshelf" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["planks_oak", "oak_planks"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["bookshelf", "books"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Dirt variants
    if block == "mycelium" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "mycelium_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "mycelium_side");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "dirt");
            }
        }
    }
    if block == "podzol" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "dirt_podzol_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "dirt_podzol_side");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "dirt");
            }
        }
    }
    // Terracotta (colored)
    if let Some(color) = block.strip_suffix("_terracotta") {
        for key in [
            format!("glazed_terracotta_{color}"),
            format!("{color}_glazed_terracotta"),
            format!("hardened_clay_stained_{color}"),
            format!("{color}_terracotta"),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "terracotta" || block == "hardened_clay" {
        for key in ["hardened_clay", "terracotta"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Concrete
    if let Some(color) = block.strip_suffix("_concrete") {
        for key in [format!("concrete_{color}"), format!("{color}_concrete")] {
            push_texture_key_aliases(keys, key);
        }
    }
    if let Some(color) = block.strip_suffix("_concrete_powder") {
        for key in [
            format!("concrete_powder_{color}"),
            format!("{color}_concrete_powder"),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Melon / Pumpkin
    if block == "melon_block" || block == "melon" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["melon_top", "melon"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["melon_side", "melon"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "pumpkin" || block == "carved_pumpkin" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["pumpkin_top", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["pumpkin_face_off", "pumpkin_side", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "lit_pumpkin" || block == "jack_o_lantern" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["pumpkin_top", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["pumpkin_face_on", "pumpkin_side", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Cake
    if block == "cake" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "cake_top");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "cake_bottom");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "cake_side");
            }
        }
    }
    // Jukebox
    if block == "jukebox" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["jukebox_top", "jukebox"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            _ => {
                for key in ["jukebox_side", "jukebox"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Glowstone / Sea lantern / Shroomlight
    if block == "glowstone" {
        for key in ["glowstone", "glowstone_diamond"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "sea_lantern" {
        for key in ["sea_lantern", "sealantern"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "shroomlight" {
        for key in ["shroomlight", "shroom_light"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Smooth stone
    if block == "smooth_stone" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["smooth_stone", "stone_slab_top"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["smooth_stone", "stone_slab_side"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Quartz
    if block == "quartz_block" || block.starts_with("quartz") {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["quartz_block_top", "quartz_block", "quartz"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["quartz_block_side", "quartz_block", "quartz"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Purpur
    if block == "purpur_block" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["purpur_block_top", "purpur_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["purpur_block_side", "purpur_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Sponge
    if block == "sponge" {
        push_texture_key_aliases(keys, "sponge");
    }
    if block == "wet_sponge" {
        for key in ["sponge_wet", "sponge"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // End stone
    if block == "end_stone" {
        for key in ["end_stone", "endstone"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Netherrack
    if block == "netherrack" {
        for key in ["netherrack", "netherrack_block"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Soul sand
    if block == "soul_sand" {
        for key in ["soul_sand", "soulsand"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Ancient debris
    if block == "ancient_debris" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["ancient_debris_top", "ancient_debris"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["ancient_debris_side", "ancient_debris"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Hay block
    if block == "hay_block" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["hay_block_top", "hay_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["hay_block_side", "hay_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Sculk catalyst
    if block == "sculk_catalyst" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "sculk_catalyst_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "sculk_catalyst_side");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "sculk_catalyst_bottom");
            }
        }
    }
    // Ore generic fallback
    if block.ends_with("_ore") {
        let ore_name = block.strip_suffix("_ore").unwrap_or(block);
        push_texture_key_aliases(keys, &format!("ore_{ore_name}"));
    }
    // Noteblock
    if block == "noteblock" || block == "note_block" {
        for key in ["noteblock", "note_block"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Command block
    if block.contains("command_block") {
        for key in ["command_block_front", "command_block_side", "command_block"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Spawner
    if block == "mob_spawner" || block == "spawner" {
        for key in ["mob_spawner", "spawner"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Daylight detector
    if block == "daylight_detector" || block == "daylight_detector_inverted" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "daylight_detector_top");
            }
            ObjTextureSlot::Side | ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "daylight_detector_side");
            }
        }
    }
    // Moss block
    if block == "moss_block" {
        for key in ["moss_block", "moss"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Amethyst
    if block.contains("amethyst") {
        push_texture_key_aliases(keys, block);
        push_texture_key_aliases(keys, "amethyst_block");
    }
    // Copper blocks (oxidation variants, excluding chests)
    if block.contains("copper") && !block.contains("chest") {
        let base = block.trim_start_matches("waxed_");
        push_texture_key_aliases(keys, base);
    }
    // Packed mud / mud brick
    if block == "mud" {
        push_texture_key_aliases(keys, "mud");
    }
    if block == "packed_mud" {
        push_texture_key_aliases(keys, "packed_mud");
    }
    // Chiseled bookshelf
    if block == "chiseled_bookshelf" {
        for key in ["chiseled_bookshelf", "chiseled_bookshelf_side"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Sand aliases
    if block == "red_sand" {
        for key in ["red_sand", "sand_red"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Nether gold ore
    if block == "nether_gold_ore" {
        for key in ["nether_gold_ore", "gold_ore_nether"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Dried kelp block
    if block == "dried_kelp_block" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["dried_kelp_top", "dried_kelp_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["dried_kelp_side", "dried_kelp_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Barrel textures
    if block == "barrel" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["barrel_top", "barrel_top_open", "barrel"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["barrel_bottom", "barrel_top", "barrel"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["barrel_side", "barrel"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Water
    if is_water_block(block) {
        for key in [
            "still_water",
            "flowing_water",
            "water_still",
            "water_flow",
            "still_water_grey",
            "flowing_water_grey",
            "water",
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Lava
    if is_lava_block(block) {
        for key in [
            "still_lava",
            "flowing_lava",
            "lava_still",
            "lava_flow",
            "lava",
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Crafting table
    if block == "crafting_table" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["crafting_table_top", "crafting_table"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in [
                    "crafting_table_side",
                    "crafting_table_front",
                    "crafting_table",
                ] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["planks_oak", "oak_planks", "crafting_table"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Furnace variants
    if block == "furnace" || block == "lit_furnace" {
        let is_lit = block == "lit_furnace";
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["furnace_top", "furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                if is_lit {
                    for key in [
                        "furnace_front_on",
                        "furnace_front",
                        "furnace_side",
                        "furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                } else {
                    for key in [
                        "furnace_front_off",
                        "furnace_front",
                        "furnace_side",
                        "furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                }
            }
            ObjTextureSlot::Down => {
                for key in ["furnace_top", "furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "smoker" || block == "lit_smoker" {
        let is_lit = block == "lit_smoker";
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["smoker_top", "smoker"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                if is_lit {
                    for key in ["smoker_front_on", "smoker_front", "smoker_side", "smoker"] {
                        push_texture_key_aliases(keys, key);
                    }
                } else {
                    for key in ["smoker_front_off", "smoker_front", "smoker_side", "smoker"] {
                        push_texture_key_aliases(keys, key);
                    }
                }
            }
            ObjTextureSlot::Down => {
                for key in ["smoker_bottom", "smoker"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "blast_furnace" || block == "lit_blast_furnace" {
        let is_lit = block == "lit_blast_furnace";
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["blast_furnace_top", "blast_furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                if is_lit {
                    for key in [
                        "blast_furnace_front_on",
                        "blast_furnace_front",
                        "blast_furnace_side",
                        "blast_furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                } else {
                    for key in [
                        "blast_furnace_front_off",
                        "blast_furnace_front",
                        "blast_furnace_side",
                        "blast_furnace",
                    ] {
                        push_texture_key_aliases(keys, key);
                    }
                }
            }
            ObjTextureSlot::Down => {
                for key in ["blast_furnace_top", "blast_furnace"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Dispenser/Dropper
    if block == "dispenser" || block == "dropper" {
        let block_str = block.to_owned();
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, &format!("{block_str}_top"));
                push_texture_key_aliases(keys, "furnace_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, &format!("{block_str}_front_horizontal"));
                push_texture_key_aliases(keys, &format!("{block_str}_front"));
                push_texture_key_aliases(keys, &format!("{block_str}_side"));
                push_texture_key_aliases(keys, block);
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, &format!("{block_str}_top"));
                push_texture_key_aliases(keys, block);
            }
        }
    }
    // Observer
    if block == "observer" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["observer_top", "observer"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["observer_front", "observer_side", "observer"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["observer_back", "observer_top", "observer"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Piston
    if block == "piston" || block == "sticky_piston" {
        let top_key = if block == "sticky_piston" {
            "piston_top_sticky"
        } else {
            "piston_top_normal"
        };
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in [top_key, "piston_top", "piston"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["piston_side", "piston"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["piston_bottom", "piston_side", "piston"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // TNT
    if block == "tnt" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["tnt_top", "tnt"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["tnt_bottom", "tnt"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["tnt_side", "tnt"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Cauldron
    if block == "cauldron" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["cauldron_inner", "cauldron_top", "cauldron"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Down => {
                for key in ["cauldron_bottom", "cauldron"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["cauldron_side", "cauldron"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Bookshelf
    if block == "bookshelf" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["planks_oak", "oak_planks"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["bookshelf", "books"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Dirt variants
    if block == "mycelium" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "mycelium_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "mycelium_side");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "dirt");
            }
        }
    }
    if block == "podzol" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "dirt_podzol_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "dirt_podzol_side");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "dirt");
            }
        }
    }
    // Terracotta (stained)
    if let Some(color) = block.strip_suffix("_terracotta") {
        for key in [
            format!("glazed_terracotta_{color}"),
            format!("{color}_glazed_terracotta"),
            format!("hardened_clay_stained_{color}"),
            format!("{color}_terracotta"),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "terracotta" || block == "hardened_clay" {
        for key in ["hardened_clay", "terracotta"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Concrete
    if let Some(color) = block.strip_suffix("_concrete") {
        for key in [format!("concrete_{color}"), format!("{color}_concrete")] {
            push_texture_key_aliases(keys, key);
        }
    }
    if let Some(color) = block.strip_suffix("_concrete_powder") {
        for key in [
            format!("concrete_powder_{color}"),
            format!("{color}_concrete_powder"),
        ] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Melon / Pumpkin
    if block == "melon_block" || block == "melon" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["melon_top", "melon"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["melon_side", "melon"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "pumpkin" || block == "carved_pumpkin" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["pumpkin_top", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["pumpkin_face_off", "pumpkin_side", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    if block == "lit_pumpkin" || block == "jack_o_lantern" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["pumpkin_top", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["pumpkin_face_on", "pumpkin_side", "pumpkin"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Cake
    if block == "cake" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "cake_top");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "cake_bottom");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "cake_side");
            }
        }
    }
    // Jukebox
    if block == "jukebox" {
        match texture_slot {
            ObjTextureSlot::Up => {
                for key in ["jukebox_top", "jukebox"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            _ => {
                for key in ["jukebox_side", "jukebox"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Glowstone / Sea lantern / Shroomlight
    if block == "glowstone" {
        for key in ["glowstone", "glowstone_diamond"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "sea_lantern" {
        for key in ["sea_lantern", "sealantern"] {
            push_texture_key_aliases(keys, key);
        }
    }
    if block == "shroomlight" {
        for key in ["shroomlight", "shroom_light"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Smooth stone
    if block == "smooth_stone" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["smooth_stone", "stone_slab_top"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["smooth_stone", "stone_slab_side"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Quartz
    if block == "quartz_block" || block.starts_with("quartz") {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["quartz_block_top", "quartz_block", "quartz"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["quartz_block_side", "quartz_block", "quartz"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Purpur
    if block == "purpur_block" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["purpur_block_top", "purpur_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["purpur_block_side", "purpur_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Sponge
    if block == "sponge" {
        push_texture_key_aliases(keys, "sponge");
    }
    if block == "wet_sponge" {
        for key in ["sponge_wet", "sponge"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // End stone
    if block == "end_stone" {
        for key in ["end_stone", "endstone"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Netherrack
    if block == "netherrack" {
        for key in ["netherrack", "netherrack_block"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Soul sand
    if block == "soul_sand" {
        for key in ["soul_sand", "soulsand"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Ancient debris
    if block == "ancient_debris" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["ancient_debris_top", "ancient_debris"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["ancient_debris_side", "ancient_debris"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Hay block
    if block == "hay_block" {
        match texture_slot {
            ObjTextureSlot::Up | ObjTextureSlot::Down => {
                for key in ["hay_block_top", "hay_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
            ObjTextureSlot::Side => {
                for key in ["hay_block_side", "hay_block"] {
                    push_texture_key_aliases(keys, key);
                }
            }
        }
    }
    // Sculk
    if block == "sculk_catalyst" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "sculk_catalyst_top");
            }
            ObjTextureSlot::Side => {
                push_texture_key_aliases(keys, "sculk_catalyst_side");
            }
            ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "sculk_catalyst_bottom");
            }
        }
    }
    // Ore generic fallback
    if block.ends_with("_ore") {
        let ore_name = block.strip_suffix("_ore").unwrap_or(block);
        push_texture_key_aliases(keys, &format!("ore_{ore_name}"));
    }
    // Noteblock
    if block == "noteblock" || block == "note_block" {
        for key in ["noteblock", "note_block"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Command block
    if block.contains("command_block") {
        for key in ["command_block_front", "command_block_side", "command_block"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Spawner
    if block == "mob_spawner" || block == "spawner" {
        for key in ["mob_spawner", "spawner"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Daylight detector
    if block == "daylight_detector" || block == "daylight_detector_inverted" {
        match texture_slot {
            ObjTextureSlot::Up => {
                push_texture_key_aliases(keys, "daylight_detector_top");
            }
            ObjTextureSlot::Side | ObjTextureSlot::Down => {
                push_texture_key_aliases(keys, "daylight_detector_side");
            }
        }
    }
    // Moss block
    if block == "moss_block" {
        for key in ["moss_block", "moss"] {
            push_texture_key_aliases(keys, key);
        }
    }
    // Amethyst
    if block.contains("amethyst") {
        push_texture_key_aliases(keys, block);
        push_texture_key_aliases(keys, "amethyst_block");
    }
}

fn push_wood_planks_texture_aliases(keys: &mut Vec<String>, wood: &str) {
    match wood {
        "dark_oak" | "darkoak" | "big_oak" => {
            for key in [
                "wood_big_oak",
                "planks_big_oak",
                "big_oak_planks",
                "dark_oak_planks",
                "planks_dark_oak",
                "wood_dark_oak",
                "planks",
            ] {
                push_texture_key_aliases(keys, key);
            }
        }
        "spruce" => push_bedrock_wood_planks_aliases(keys, wood, "wood_spruce"),
        "birch" => push_bedrock_wood_planks_aliases(keys, wood, "wood_birch"),
        "jungle" => push_bedrock_wood_planks_aliases(keys, wood, "wood_jungle"),
        "acacia" => push_bedrock_wood_planks_aliases(keys, wood, "wood_acacia"),
        "oak" => push_bedrock_wood_planks_aliases(keys, wood, "wood_oak"),
        "mangrove" | "cherry" | "crimson" | "warped" | "pale_oak" | "bamboo" => {
            for key in [
                format!("{wood}_planks"),
                format!("planks_{wood}"),
                format!("wood_{wood}"),
                "planks".to_owned(),
            ] {
                push_texture_key_aliases(keys, key);
            }
        }
        _ => {
            for key in [
                format!("{wood}_planks"),
                format!("planks_{wood}"),
                format!("wood_{wood}"),
                "planks".to_owned(),
            ] {
                push_texture_key_aliases(keys, key);
            }
        }
    }
}

fn push_bedrock_wood_planks_aliases(keys: &mut Vec<String>, wood: &str, bedrock_key: &str) {
    for key in [
        bedrock_key.to_owned(),
        format!("planks_{wood}"),
        format!("{wood}_planks"),
        format!("wood_{wood}"),
        "planks".to_owned(),
    ] {
        push_texture_key_aliases(keys, key);
    }
}

fn chest_texture_aliases(block: &str, slot: ObjTextureSlot) -> Vec<String> {
    let face = match slot {
        ObjTextureSlot::Up | ObjTextureSlot::Down => "top",
        ObjTextureSlot::Side => "side",
    };
    let front_face = if matches!(slot, ObjTextureSlot::Side) {
        Some("front")
    } else {
        None
    };
    let inventory_base = match block {
        "ender_chest" => "ender_chest",
        "trapped_chest" => "chest",
        value if value.starts_with("waxed_") => value.trim_start_matches("waxed_"),
        value => value,
    };
    let entity_base = match block {
        "trapped_chest" => "trapped",
        "ender_chest" => "ender",
        "copper_chest" | "waxed_copper_chest" => "copper_default",
        "exposed_copper_chest" | "waxed_exposed_copper_chest" => "copper_exposed",
        "weathered_copper_chest" | "waxed_weathered_copper_chest" => "copper_weathered",
        "oxidized_copper_chest" | "waxed_oxidized_copper_chest" => "copper_oxidized",
        _ => "normal",
    };
    let mut keys = Vec::with_capacity(12);
    if let Some(front_face) = front_face {
        let front_base = if block == "trapped_chest" {
            "trapped_chest"
        } else {
            inventory_base
        };
        keys.push(format!("{front_base}_inventory_{front_face}"));
        keys.push(format!("{front_base}_{front_face}"));
    }
    keys.push(format!("{inventory_base}_inventory_{face}"));
    keys.push(format!("{inventory_base}_{face}"));
    if inventory_base != "chest" && inventory_base.ends_with("_chest") {
        keys.push(format!("chest_inventory_{face}"));
    }
    keys.push(format!("chest_{face}"));
    keys.push(format!("entity/chest/{entity_base}"));
    keys.push(format!("chest/{entity_base}"));
    keys.push(block.to_owned());
    if entity_base != "normal" {
        keys.push(format!("{entity_base}_chest"));
    }
    if matches!(slot, ObjTextureSlot::Side) {
        keys.push("chest_front".to_owned());
        keys.push("chest_side".to_owned());
        if block == "trapped_chest" {
            keys.push("trapped_chest_front".to_owned());
        }
        if block == "ender_chest" {
            keys.push("ender_chest_front".to_owned());
            keys.push("ender_chest_side".to_owned());
        }
    } else {
        keys.push("chest_top".to_owned());
        if block == "ender_chest" {
            keys.push("ender_chest_top".to_owned());
        }
    }
    keys
}

fn sign_texture_aliases(block: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity(24);
    if matches!(block, "standing_sign" | "wall_sign") {
        keys.push("entity/sign".to_owned());
        keys.push("sign".to_owned());
        return keys;
    }
    if block == "hanging_sign" {
        keys.push("entity/hanging_sign".to_owned());
        keys.push("hanging_sign".to_owned());
        return keys;
    }

    if let Some(wood) = block
        .strip_suffix("_standing_sign")
        .or_else(|| block.strip_suffix("_wall_sign"))
    {
        for wood in sign_wood_name_aliases(wood) {
            keys.push(format!("entity/{wood}_sign"));
            keys.push(format!("entity/sign_{wood}"));
            keys.push(format!("entity/signs/{wood}"));
            keys.push(format!("{wood}_sign"));
            keys.push(format!("sign_{wood}"));
        }
    }
    if let Some(wood) = block
        .strip_suffix("_wall_hanging_sign")
        .or_else(|| block.strip_suffix("_hanging_sign"))
    {
        keys.push(format!("entity/{block}"));
        for wood in sign_wood_name_aliases(wood) {
            keys.push(format!("entity/{wood}_hanging_sign"));
            keys.push(format!("entity/signs/hanging/{wood}"));
            keys.push(format!("{wood}_hanging_sign"));
            keys.push(format!("hanging_sign_{wood}"));
            keys.push(format!("{wood}_sign"));
            keys.push(format!("sign_{wood}"));
        }
    }
    keys.push(block.to_owned());
    keys
}

fn sign_wood_name_aliases(wood: &str) -> Vec<String> {
    let mut aliases = Vec::with_capacity(3);
    push_unique_string(&mut aliases, wood.to_owned());
    match wood {
        "dark_oak" => {
            push_unique_string(&mut aliases, "darkoak".to_owned());
            push_unique_string(&mut aliases, "big_oak".to_owned());
        }
        "darkoak" | "big_oak" => {
            push_unique_string(&mut aliases, "dark_oak".to_owned());
            push_unique_string(&mut aliases, "darkoak".to_owned());
        }
        _ => {}
    }
    aliases
}

fn copper_golem_texture_key(block: &str) -> Option<&'static str> {
    let block = block.strip_prefix("waxed_").unwrap_or(block);
    match block {
        "copper_golem_statue" => Some("copper_golem"),
        "exposed_copper_golem_statue" => Some("copper_golem_exposed"),
        "weathered_copper_golem_statue" => Some("copper_golem_weathered"),
        "oxidized_copper_golem_statue" => Some("copper_golem_oxidized"),
        _ => None,
    }
}

fn push_texture_key_aliases(keys: &mut Vec<String>, texture_key: impl AsRef<str>) {
    let normalized = obj_normalize_texture_key(texture_key.as_ref());
    if normalized.is_empty() {
        return;
    }

    let mut aliases = Vec::with_capacity(8);
    push_unique_string(&mut aliases, normalized.clone());
    if let Some(stripped) = normalized.strip_prefix("minecraft:") {
        push_unique_string(&mut aliases, stripped.to_owned());
    }
    if let Some(stripped) = strip_texture_face_suffix(&normalized) {
        push_unique_string(&mut aliases, stripped);
    }
    if let Some(file_name) = normalized
        .rsplit('/')
        .next()
        .filter(|name| *name != normalized.as_str())
    {
        push_unique_string(&mut aliases, file_name.to_owned());
        if let Some(stripped) = file_name.strip_prefix("minecraft:") {
            push_unique_string(&mut aliases, stripped.to_owned());
        }
        if let Some(stripped) = strip_texture_face_suffix(file_name) {
            push_unique_string(&mut aliases, stripped);
        }
    }

    for alias in aliases {
        push_unique_string(keys, alias);
    }
}

fn strip_texture_face_suffix(value: &str) -> Option<String> {
    for suffix in [
        "_top", "_bottom", "_up", "_down", "_side", "_north", "_south", "_east", "_west",
    ] {
        if let Some(stripped) = value
            .strip_suffix(suffix)
            .filter(|stripped| !stripped.is_empty())
        {
            return Some(stripped.to_owned());
        }
    }
    None
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NamedObjMaterial, ObjExportMaterial, ObjFace, ObjMaterial, ObjMeshFace, ObjMeshFaceSource,
        ObjTextureCopy, ObjTextureResolver, candle_block_for_cake, obj_alpha_texture_path,
        obj_default_face_uvs_from_corners, obj_document_string,
        obj_export_from_face_sources_with_package_roots, obj_export_from_mesh_face_groups,
        obj_export_from_mesh_face_groups_with_progress, obj_face_normal_from_triangle,
        obj_face_texture_slot_suffix, obj_faces_string, obj_material_library_from_export_materials,
        obj_material_library_string, obj_material_name_for_block, obj_material_name_for_face,
        obj_material_name_for_slot, obj_material_slot_component, obj_mesh_face_materials,
        obj_mesh_faces_from_source, obj_mesh_faces_string, obj_texture_copies,
        path_starts_with_directory, vanilla_resource_pack_roots, world_resource_pack_ids,
        world_resource_pack_paths, write_obj_texture_copy,
    };
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;

    #[test]
    fn material_library_should_emit_alpha_map_for_cutout_detail_textures() {
        let material = ObjMaterial::from_preview_color(
            "minecraft_redstone_wire__mat_up",
            [0.8, 0.0, 0.0, 1.0],
            Some("textures/blocks/redstone_dust_cross.png".to_owned()),
        );

        let material_library = obj_material_library_string([NamedObjMaterial {
            name: Cow::Borrowed("minecraft_redstone_wire__mat_up"),
            material: material.clone(),
        }]);

        assert!(
            material_library
                .contains("map_d -imfchan a textures/blocks/redstone_dust_cross_alpha.png")
        );
        assert_eq!(
            material.alpha_texture_path.as_deref(),
            Some("textures/blocks/redstone_dust_cross_alpha.png")
        );
    }

    #[test]
    fn candle_cake_slot_should_resolve_to_candle_block_name() {
        assert_eq!(
            candle_block_for_cake("candle_cake").as_deref(),
            Some("candle")
        );
        assert_eq!(
            candle_block_for_cake("blue_candle_cake").as_deref(),
            Some("blue_candle")
        );
        assert_eq!(candle_block_for_cake("cake"), None);
    }

    #[test]
    fn material_library_should_tint_grass_top_but_not_plain_stone() {
        let grass = ObjMaterial::from_preview_color(
            "minecraft_grass_block_up",
            [0.25, 0.70, 0.20, 1.0],
            Some("textures/blocks/grass_top.png".to_owned()),
        );
        let grass_slot = ObjMaterial::from_preview_color(
            "minecraft_grass__mat_up",
            [0.25, 0.70, 0.20, 1.0],
            Some("textures/blocks/grass_top.png".to_owned()),
        );
        let grass_side = ObjMaterial::from_preview_color(
            "minecraft_grass__mat_side",
            [0.25, 0.70, 0.20, 1.0],
            Some("textures/blocks/grass_side_carried.png".to_owned()),
        );
        let water = ObjMaterial::from_preview_color(
            "minecraft_flowing_water__mat_up",
            [1.0, 1.0, 1.0, 1.0],
            Some("textures/blocks/water_still.png".to_owned()),
        );
        let stone = ObjMaterial::from_preview_color(
            "minecraft_stone_side",
            [0.25, 0.70, 0.20, 0.4],
            Some("textures/blocks/stone.png".to_owned()),
        );

        assert_eq!(grass.texture_tint, Some([0.25, 0.70, 0.20]));
        assert_eq!(grass_slot.texture_tint, Some([0.25, 0.70, 0.20]));
        assert!(grass_side.use_texture_alpha);
        assert_eq!(water.texture_tint, Some([0.267, 0.686, 0.961]));
        assert!((water.dissolve - 0.65).abs() < f32::EPSILON);
        assert_eq!(stone.texture_tint, None);
        assert_eq!(stone.diffuse_color, [1.0, 1.0, 1.0]);
        assert_eq!(stone.dissolve, 1.0);
    }

    #[test]
    fn material_library_should_use_preview_color_when_texture_is_missing() {
        let material = ObjMaterial::from_preview_color(
            "minecraft_unknown_side",
            [0.25, 0.50, 0.75, 0.40],
            None,
        );

        let material_library = obj_material_library_string([NamedObjMaterial {
            name: Cow::Borrowed("minecraft_unknown_side"),
            material,
        }]);

        assert!(material_library.contains("Kd 0.250000 0.500000 0.750000"));
        assert!(material_library.contains("d 0.400000"));
        assert!(!material_library.contains("map_Kd"));
    }

    #[test]
    fn material_library_should_emit_alpha_maps_for_named_cutout_textures() {
        let materials = [
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_iron_bars_side"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_iron_bars_side",
                    [1.0, 1.0, 1.0, 1.0],
                    Some("textures/blocks/iron_bars.png".to_owned()),
                ),
            },
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_glass_side"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_glass_side",
                    [1.0, 1.0, 1.0, 1.0],
                    Some("textures/blocks/glass.png".to_owned()),
                ),
            },
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_black_stained_glass_side"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_black_stained_glass_side",
                    [1.0, 1.0, 1.0, 1.0],
                    Some("textures/blocks/glass_black.png".to_owned()),
                ),
            },
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_glass_pane__mat_east"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_glass_pane__mat_east",
                    [1.0, 1.0, 1.0, 1.0],
                    Some("textures/blocks/glass_pane_top.png".to_owned()),
                ),
            },
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_poppy_side"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_poppy_side",
                    [0.45, 0.72, 0.33, 1.0],
                    Some("textures/blocks/flower_rose.png".to_owned()),
                ),
            },
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_web_side"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_web_side",
                    [1.0, 1.0, 1.0, 1.0],
                    Some("textures/blocks/web.png".to_owned()),
                ),
            },
            NamedObjMaterial {
                name: Cow::Borrowed("minecraft_copper_grate_side"),
                material: ObjMaterial::from_preview_color(
                    "minecraft_copper_grate_side",
                    [1.0, 1.0, 1.0, 1.0],
                    Some("textures/blocks/copper_grate.png".to_owned()),
                ),
            },
        ];

        let material_library = obj_material_library_string(materials);

        assert!(material_library.contains("map_d -imfchan a textures/blocks/iron_bars_alpha.png"));
        assert!(material_library.contains("map_d -imfchan a textures/blocks/glass_alpha.png"));
        assert!(
            material_library.contains("map_d -imfchan a textures/blocks/glass_black_alpha.png")
        );
        assert!(
            material_library.contains("map_d -imfchan a textures/blocks/glass_pane_top_alpha.png")
        );
        assert!(
            material_library.contains("map_d -imfchan a textures/blocks/flower_rose_alpha.png")
        );
        assert!(material_library.contains("map_d -imfchan a textures/blocks/web_alpha.png"));
        assert!(
            material_library.contains("map_d -imfchan a textures/blocks/copper_grate_alpha.png")
        );
        assert!(material_library.contains("d 1.000000"));
        assert!(material_library.contains("illum 4"));
    }

    #[test]
    fn texture_resolver_should_resolve_redstone_wire_cross_and_line() {
        let pack = TestPack::new();
        pack.write(
            "textures/terrain_texture.json",
            r#"{
                "texture_data": {
                    "redstone_dust_cross": {"textures":"textures/blocks/redstone_dust_cross"},
                    "redstone_dust_line": {"textures":"textures/blocks/redstone_dust_line"}
                }
            }"#,
        );
        pack.write_bytes("textures/blocks/redstone_dust_cross.png", b"png");
        pack.write_bytes("textures/blocks/redstone_dust_line.png", b"png");

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        let cross = resolver
            .texture_for("minecraft_redstone_wire__mat_up", [0, 1, 0])
            .unwrap_or_else(|| panic!("missing redstone cross texture"));
        let line = resolver
            .texture_for("minecraft_redstone_wire__mat_down", [0, 1, 0])
            .unwrap_or_else(|| panic!("missing redstone line texture"));

        assert_eq!(
            cross.relative_path.as_str(),
            "textures/blocks/redstone_dust_cross.png"
        );
        assert_eq!(
            line.relative_path.as_str(),
            "textures/blocks/redstone_dust_line.png"
        );
    }

    #[test]
    fn texture_resolver_should_ignore_item_texture_json_for_block_materials() {
        let pack = TestPack::new();
        pack.write(
            "textures/item_texture.json",
            r#"{"texture_data":{"brick":{"textures":"textures/items/brick"}}"#,
        );
        pack.write_bytes("textures/items/brick.png", b"png");

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");

        assert!(
            resolver
                .texture_for("minecraft_brick_side", [0, 0, 1])
                .is_none()
        );
    }

    #[test]
    fn texture_resolver_should_resolve_common_variant_aliases_and_block_faces() {
        let pack = TestPack::new();
        pack.write(
            "blocks.json",
            r#"{
                "grass": {
                    "textures": {
                        "up": "grass_top",
                        "down": "grass_bottom",
                        "side": "grass_side"
                    },
                    "carried_textures": {
                        "side": "grass_side_carried"
                    }
                },
                "torch": { "textures": "torch_on" },
                "chest": {
                    "textures": {
                        "up": "chest_inventory_top",
                        "down": "chest_inventory_top",
                        "north": "chest_inventory_side",
                        "south": "chest_inventory_front",
                        "west": "chest_inventory_side",
                        "east": "chest_inventory_side"
                    }
                },
                "hopper": {
                    "textures": {
                        "up": "hopper_top",
                        "down": "hopper_inside",
                        "north": "hopper_outside",
                        "south": "hopper_outside",
                        "west": "hopper_outside",
                        "east": "hopper_outside"
                    }
                },
                "spruce_fence_gate": {
                    "textures": "wood_spruce"
                },
                "glass_pane": {
                    "textures": {
                        "down": "glass",
                        "east": "glass_pane_top",
                        "north": "glass",
                        "south": "glass",
                        "up": "glass",
                        "west": "glass"
                    }
                }
            }"#,
        );
        pack.write(
            "textures/terrain_texture.json",
            r#"{
                "texture_data": {
                    "wool_colored_red": {"textures":"textures/blocks/wool_colored_red"},
                    "wool_colored_blue": {"textures":"textures/blocks/wool_colored_blue"},
                    "brick_block": {"textures":"textures/blocks/brick_block"},
                    "wood_big_oak": {"textures":"textures/blocks/wood_big_oak"},
                    "iron_bars": {"textures":"textures/blocks/iron_bars"},
                    "portal": {"textures":"textures/blocks/portal"},
                    "shulker_top_blue": {"textures":"textures/blocks/shulker_top_blue"},
                    "grass_top": {"textures":"textures/blocks/grass_top"},
                    "grass_bottom": {"textures":"textures/blocks/dirt"},
                    "grass_side_carried": {"textures":"textures/blocks/grass_side_carried"},
                    "torch_on": {"textures":"textures/blocks/torch_on"},
                    "chest_inventory_top": {"textures":"textures/blocks/chest_top"},
                    "chest_inventory_side": {"textures":"textures/blocks/chest_side"},
                    "chest_inventory_front": {"textures":"textures/blocks/chest_front"},
                    "hopper_top": {"textures":"textures/blocks/hopper_top"},
                    "hopper_inside": {"textures":"textures/blocks/hopper_inside"},
                    "hopper_outside": {"textures":"textures/blocks/hopper_outside"},
                    "wood_spruce": {"textures":"textures/blocks/wood_spruce"},
                    "wood_birch": {"textures":"textures/blocks/wood_birch"},
                    "flattened_anvil_top": {"textures":"textures/blocks/anvil_top_damaged_0"},
                    "glass": {"textures":"textures/blocks/glass"},
                    "glass_pane_top": {"textures":"textures/blocks/glass_pane_top"},
                    "still_water": {"textures":"textures/blocks/water_still"}
                }
            }"#,
        );
        for file_name in [
            "wool_colored_red.png",
            "wool_colored_blue.png",
            "brick_block.png",
            "wood_big_oak.png",
            "iron_bars.png",
            "portal.png",
            "shulker_top_blue.png",
            "grass_top.png",
            "dirt.png",
            "grass_side_carried.png",
            "torch_on.png",
            "chest_top.png",
            "chest_side.png",
            "chest_front.png",
            "hopper_top.png",
            "hopper_inside.png",
            "hopper_outside.png",
            "wood_spruce.png",
            "wood_birch.png",
            "anvil_top_damaged_0.png",
            "glass.png",
            "glass_pane_top.png",
            "water_still.png",
        ] {
            pack.write_bytes(&format!("textures/blocks/{file_name}"), b"png");
        }
        pack.write_bytes("textures/entity/oak_sign.png", b"png");
        pack.write_bytes("textures/entity/sign_darkoak.png", b"png");
        pack.write_bytes(
            "textures/entity/copper_golem/copper_golem_weathered.png",
            b"png",
        );

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");

        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_red_wool_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/wool_colored_red.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_blue_wool_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/wool_colored_blue.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_bricks_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/brick_block.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_dark_oak_planks_side", [0, 0, 1])
                .as_deref(),
            Some("textures/blocks/wood_big_oak.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_iron_bars_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/iron_bars.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_portal_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/portal.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_blue_shulker_box_up", [0, 1, 0])
                .as_deref(),
            Some("textures/blocks/shulker_top_blue.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_grass_block_up", [0, 1, 0]).as_deref(),
            Some("textures/blocks/grass_top.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_grass_block_down", [0, -1, 0]).as_deref(),
            Some("textures/blocks/dirt.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_grass_block_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/grass_side_carried.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_torch_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/torch_on.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_chest_up", [0, 1, 0]).as_deref(),
            Some("textures/blocks/chest_top.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_chest_south", [0, 0, -1]).as_deref(),
            Some("textures/blocks/chest_front.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_hopper_down", [0, -1, 0]).as_deref(),
            Some("textures/blocks/hopper_inside.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_hopper_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/hopper_outside.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_spruce_fence_gate_side", [0, 0, 1])
                .as_deref(),
            Some("textures/blocks/wood_spruce.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_birch_fence_gate_side", [0, 0, 1])
                .as_deref(),
            Some("textures/blocks/wood_birch.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_birch_fence_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/wood_birch.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_anvil_up", [0, 1, 0]).as_deref(),
            Some("textures/blocks/anvil_top_damaged_0.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_glass_pane__mat_east", [0, 1, 0])
                .as_deref(),
            Some("textures/blocks/glass_pane_top.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_glass_pane_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/glass.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_flowing_water__mat_up", [0, 1, 0])
                .as_deref(),
            Some("textures/blocks/water_still.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_oak_wall_sign__mat_front", [0, 0, 1])
                .as_deref(),
            Some("textures/entity/oak_sign.png")
        );
        assert_eq!(
            resolved_relative_path(
                &resolver,
                "minecraft_dark_oak_wall_sign__mat_front",
                [0, 0, 1],
            )
            .as_deref(),
            Some("textures/entity/sign_darkoak.png")
        );
        assert_eq!(
            resolved_relative_path(
                &resolver,
                "minecraft_waxed_weathered_copper_golem_statue__mat_body",
                [0, 0, 1],
            )
            .as_deref(),
            Some("textures/entity/copper_golem/copper_golem_weathered.png")
        );
    }

    #[test]
    fn texture_resolver_should_resolve_entity_texture_fallbacks() {
        let pack = TestPack::new();
        pack.write_bytes("textures/entity/chest/normal.png", b"png");
        pack.write_bytes("textures/entity/chest/trapped.png", b"png");
        pack.write_bytes("textures/entity/dark_oak_hanging_sign.png", b"png");
        pack.write_bytes("textures/entity/oak_hanging_sign.png", b"png");

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");

        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_chest__mat_front", [0, 0, 1]).as_deref(),
            Some("textures/entity/chest/normal.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_trapped_chest_side", [0, 0, 1]).as_deref(),
            Some("textures/entity/chest/trapped.png")
        );
        assert_eq!(
            resolved_relative_path(
                &resolver,
                "minecraft_dark_oak_hanging_sign__mat_front",
                [0, 0, 1],
            )
            .as_deref(),
            Some("textures/entity/dark_oak_hanging_sign.png")
        );
        assert_eq!(
            resolved_relative_path(
                &resolver,
                "minecraft_oak_wall_hanging_sign__mat_front",
                [0, 0, 1],
            )
            .as_deref(),
            Some("textures/entity/oak_hanging_sign.png")
        );
    }

    #[test]
    fn texture_resolver_should_resolve_material_instances_across_resource_stack() {
        let pack = TestPack::new();
        pack.write(
            "block_pack/blocks/split_block.json",
            r#"{
                "minecraft:block": {
                    "description": { "identifier": "minecraft:split_block" },
                    "components": {
                        "minecraft:material_instances": {
                            "*": { "texture": "split_custom_key" }
                        }
                    }
                }
            }"#,
        );
        pack.write(
            "block_pack/blocks/stonecutter.json",
            r#"{
                "minecraft:block": {
                    "description": { "identifier": "minecraft:stonecutter" },
                    "components": {
                        "minecraft:material_instances": {
                            "*": { "texture": "stonecutter2_side" },
                            "saw": { "texture": "stonecutter2_saw" }
                        }
                    }
                }
            }"#,
        );
        pack.write(
            "texture_pack/textures/terrain_texture.json",
            r#"{
                "texture_data": {
                    "split_custom_key": {"textures":"textures/blocks/split_custom"},
                    "stonecutter2_side": {"textures":"textures/blocks/stonecutter2_side"},
                    "stonecutter2_saw": {"textures":"textures/blocks/stonecutter2_saw"}
                }
            }"#,
        );
        pack.write_bytes("texture_pack/textures/blocks/split_custom.png", b"png");
        pack.write_bytes("texture_pack/textures/blocks/stonecutter2_side.png", b"png");
        pack.write_bytes("texture_pack/textures/blocks/stonecutter2_saw.tga", b"tga");

        let resolver = ObjTextureResolver::with_pack_roots(
            [
                pack.path().join("block_pack"),
                pack.path().join("texture_pack"),
            ],
            "textures",
        );

        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_split_block_side", [0, 0, 1]).as_deref(),
            Some("textures/blocks/split_custom.png")
        );
        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_stonecutter__mat_saw", [0, 1, 0])
                .as_deref(),
            Some("textures/blocks/stonecutter2_saw.png")
        );
    }

    #[test]
    fn texture_resolver_should_resolve_versioned_package_overlay_texture() {
        let package = TestPack::new();
        package.write_bytes("data/resource_packs/vanilla/textures/.keep", b"");
        package.write(
            "data/resource_packs/vanilla_1.17.0/blocks.json",
            r#"{"amethyst_cluster":{"textures":"amethyst_cluster"}}"#,
        );
        package.write(
            "data/resource_packs/vanilla_1.17.0/textures/terrain_texture.json",
            r#"{"texture_data":{"amethyst_cluster":{"textures":"textures/blocks/amethyst_cluster"}}}"#,
        );
        package.write_bytes(
            "data/resource_packs/vanilla_1.17.0/textures/blocks/amethyst_cluster.png",
            b"png",
        );

        let resolver = ObjTextureResolver::with_package_roots([package.path()], "textures");

        assert_eq!(
            resolved_relative_path(&resolver, "minecraft_amethyst_cluster_up", [0, 1, 0])
                .as_deref(),
            Some("textures/blocks/amethyst_cluster.png")
        );
    }

    #[test]
    fn texture_prefix_should_require_path_separator() {
        assert!(!path_starts_with_directory(
            "texturesamethyst_cluster_up.png",
            "textures"
        ));
        assert!(path_starts_with_directory(
            "textures/amethyst_cluster_up.png",
            "textures"
        ));
    }

    #[test]
    fn vanilla_resource_pack_roots_should_include_base_and_sorted_overlays() {
        let pack = TestPack::new();
        pack.write_bytes("data/resource_packs/vanilla/textures/.keep", b"");
        pack.write_bytes("data/resource_packs/vanilla_1.20.0/textures/.keep", b"");
        pack.write_bytes("data/resource_packs/vanilla_1.21.40/textures/.keep", b"");

        let roots = vanilla_resource_pack_roots(&pack.path());

        let expected = vec![
            pack.path(),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla")
                .join("client"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.21.40")
                .join("client"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.21.40"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.20.0")
                .join("client"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.20.0"),
            pack.path()
                .join("data")
                .join("resourcepacks")
                .join("vanilla")
                .join("client"),
            pack.path()
                .join("data")
                .join("resourcepacks")
                .join("vanilla"),
        ];

        assert_eq!(roots, expected);
    }

    #[test]
    fn world_resource_pack_paths_should_follow_world_manifest_pack_order() {
        let root = TestPack::new();
        let world_path = root
            .path()
            .join("Users")
            .join("player")
            .join("games")
            .join("com.mojang")
            .join("minecraftWorlds")
            .join("world");
        let local_pack = world_path.join("resource_packs").join("pack-local");
        let global_pack = root
            .path()
            .join("Users")
            .join("player")
            .join("games")
            .join("com.mojang")
            .join("resource_packs")
            .join("global-pack");
        let shared_pack = root
            .path()
            .join("Users")
            .join("Shared")
            .join("games")
            .join("com.mojang")
            .join("resource_packs")
            .join("shared-pack");
        for pack_path in [&local_pack, &global_pack, &shared_pack] {
            must_ok(fs::create_dir_all(pack_path.join("textures")));
        }
        root.write(
            "Users/player/games/com.mojang/minecraftWorlds/world/resource_packs/pack-local/manifest.json",
            r#"{"header":{"uuid":"{11111111-1111-1111-1111-111111111111}"}}"#,
        );
        root.write(
            "Users/player/games/com.mojang/resource_packs/global-pack/manifest.json",
            r#"{"header":{"uuid":"22222222-2222-2222-2222-222222222222"}}"#,
        );
        root.write(
            "Users/Shared/games/com.mojang/resource_packs/shared-pack/manifest.json",
            r#"{"header":{"uuid":"33333333-3333-3333-3333-333333333333"}}"#,
        );
        root.write(
            "Users/player/games/com.mojang/minecraftWorlds/world/world_resource_packs.json",
            r#"[
                // comments and trailing commas are accepted
                {"pack_id":"22222222-2222-2222-2222-222222222222"},
                {"pack_id":"11111111-1111-1111-1111-111111111111"},
                {"uuid":"33333333-3333-3333-3333-333333333333"},
            ]"#,
        );

        let paths = world_resource_pack_paths(&world_path);

        assert_eq!(paths, vec![global_pack, local_pack, shared_pack]);
    }

    #[test]
    fn world_resource_pack_ids_should_normalize_and_dedupe_pack_ids() {
        let root = TestPack::new();
        let world_path = root.path().join("world");
        root.write(
            "world/world_resource_packs.json",
            r#"[
                {"pack_id":"{AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA}"},
                {"uuid":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"},
                {"pack_id":"BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB"},
            ]"#,
        );

        let pack_ids = world_resource_pack_ids(&world_path);

        assert_eq!(
            pack_ids,
            vec![
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_owned(),
                "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".to_owned(),
            ]
        );
    }

    #[test]
    fn obj_export_target_should_use_directory_named_after_selected_obj_stem() {
        let root = TestPack::new();
        let selected_path = root.path().join("chunk-selection.obj");

        let target = must_ok(super::ObjExportTarget::from_obj_path(&selected_path));

        assert_eq!(target.export_root, root.path().join("chunk-selection"));
        assert_eq!(
            target.obj_path,
            root.path()
                .join("chunk-selection")
                .join("chunk-selection.obj")
        );
        assert_eq!(
            target.material_library_path,
            root.path()
                .join("chunk-selection")
                .join("chunk-selection.mtl")
        );
        assert_eq!(target.material_library_name, "chunk-selection.mtl");
    }

    #[test]
    fn alpha_texture_path_should_append_alpha_before_extension() {
        assert_eq!(
            obj_alpha_texture_path("textures/blocks/web.png"),
            "textures/blocks/web_alpha.png"
        );
        assert_eq!(
            obj_alpha_texture_path("textures/blocks/web_alpha.png"),
            "textures/blocks/web_alpha.png"
        );
        assert_eq!(
            obj_alpha_texture_path("textures/blocks/custom"),
            "textures/blocks/custom_alpha"
        );
    }

    #[test]
    fn obj_faces_string_should_write_material_vertices_uvs_and_indices() {
        let text = obj_faces_string(
            [
                ObjFace {
                    material: Cow::Borrowed("minecraft_test_side"),
                    positions: [
                        [0.0, 0.0, 0.0],
                        [1.0, 0.0, 0.0],
                        [1.0, 1.0, 0.0],
                        [0.0, 1.0, 0.0],
                    ],
                    uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                },
                ObjFace {
                    material: Cow::Borrowed("minecraft_test_side"),
                    positions: [
                        [0.0, 0.0, 1.0],
                        [1.0, 0.0, 1.0],
                        [1.0, 1.0, 1.0],
                        [0.0, 1.0, 1.0],
                    ],
                    uv: [[-1.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]],
                },
            ],
            5,
        );

        assert_eq!(text.matches("usemtl minecraft_test_side").count(), 1);
        assert!(text.contains("v 1.000000 1.000000 1.000000"));
        assert!(text.contains("vt 0.000000 0.000000"));
        assert!(text.contains("f 5/5 6/6 7/7 8/8"));
        assert!(text.contains("f 9/9 10/10 11/11 12/12"));
    }

    #[test]
    fn obj_mesh_faces_string_should_convert_triangle_faces_to_obj_quads() {
        let text = obj_mesh_faces_string(
            [ObjMeshFace {
                material: Cow::Borrowed("minecraft_test_side"),
                color: [1.0, 1.0, 1.0, 1.0],
                triangle_positions: [
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [1.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0],
                    [1.0, 1.0, 0.0],
                    [0.0, 1.0, 0.0],
                ],
                uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            }],
            13,
        );

        assert!(text.contains("usemtl minecraft_test_side\n"));
        assert!(text.contains("v 0.000000 1.000000 0.000000\n"));
        assert!(text.contains("f 13/13 14/14 15/15 16/16\n"));
    }

    #[test]
    fn obj_mesh_face_materials_should_sample_normal_from_triangle_vertices() {
        let pack = TestPack::new();
        pack.write(
            "textures/terrain_texture.json",
            r#"{"texture_data":{"test_top":{"textures":"textures/blocks/test_top"}}}"#,
        );
        pack.write_bytes("textures/blocks/test_top.png", b"png");

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        let materials = obj_mesh_face_materials(
            [ObjMeshFace {
                material: Cow::Borrowed("minecraft_test_up"),
                color: [0.2, 0.3, 0.4, 1.0],
                triangle_positions: [
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [1.0, 0.0, -1.0],
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, -1.0],
                    [1.0, 0.0, 0.0],
                ],
                uv: [[0.0, 0.0]; 4],
            }],
            &resolver,
        );

        let material = materials
            .get("minecraft_test_up")
            .unwrap_or_else(|| panic!("missing sampled mesh material"));
        assert_eq!(
            material.material.relative_texture_path.as_deref(),
            Some("textures/blocks/test_top.png")
        );
    }

    #[test]
    fn obj_export_from_mesh_face_groups_should_assemble_document_materials_and_progress() {
        let pack = TestPack::new();
        pack.write(
            "textures/terrain_texture.json",
            r#"{"texture_data":{"test_top":{"textures":"textures/blocks/test_top"}}}"#,
        );
        pack.write_bytes("textures/blocks/test_top.png", b"png");

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        let face = ObjMeshFace {
            material: Cow::Borrowed("minecraft_test_up"),
            color: [0.2, 0.3, 0.4, 1.0],
            triangle_positions: [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, -1.0],
                [0.0, 0.0, 0.0],
                [1.0, 0.0, -1.0],
                [1.0, 0.0, 0.0],
            ],
            uv: [[0.0, 0.0]; 4],
        };
        let mut progress = Vec::new();

        let export = obj_export_from_mesh_face_groups_with_progress(
            "test export",
            "selection.mtl",
            "selection",
            [vec![face]],
            &resolver,
            |done, total| progress.push((done, total)),
        );

        assert!(
            export
                .obj_text
                .starts_with("# test export\nmtllib selection.mtl\n")
        );
        assert!(export.obj_text.contains("usemtl minecraft_test_up\n"));
        assert!(
            export
                .material_library_text
                .contains("map_Kd textures/blocks/test_top.png")
        );
        assert_eq!(export.texture_copies.len(), 1);
        assert_eq!(progress, [(0, 1), (1, 1)]);
    }

    #[test]
    fn obj_export_from_mesh_face_groups_should_cull_internal_opaque_faces_before_materials() {
        let pack = TestPack::new();
        pack.write(
            "textures/terrain_texture.json",
            r#"{
                "texture_data": {
                    "stone": {"textures":"textures/blocks/stone"},
                    "dirt": {"textures":"textures/blocks/dirt"},
                    "gold_block": {"textures":"textures/blocks/gold_block"}
                }
            }"#,
        );
        pack.write_bytes("textures/blocks/stone.png", b"png");
        pack.write_bytes("textures/blocks/dirt.png", b"png");
        pack.write_bytes("textures/blocks/gold_block.png", b"png");

        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        let export = obj_export_from_mesh_face_groups(
            "test export",
            "selection.mtl",
            "selection",
            [
                vec![
                    mesh_face(
                        "minecraft_stone_side",
                        [
                            [1.0, 0.0, 0.0],
                            [1.0, 1.0, 0.0],
                            [1.0, 1.0, 1.0],
                            [1.0, 0.0, 1.0],
                        ],
                    ),
                    mesh_face(
                        "minecraft_gold_block_side",
                        [
                            [0.0, 0.0, 2.0],
                            [1.0, 0.0, 2.0],
                            [1.0, 1.0, 2.0],
                            [0.0, 1.0, 2.0],
                        ],
                    ),
                ],
                vec![mesh_face(
                    "minecraft_dirt_side",
                    [
                        [1.0, 0.0, 0.0],
                        [1.0, 0.0, 1.0],
                        [1.0, 1.0, 1.0],
                        [1.0, 1.0, 0.0],
                    ],
                )],
            ],
            &resolver,
        );

        assert_eq!(export.obj_text.matches("\nf ").count(), 1);
        assert_eq!(export.material_count, 1);
        assert_eq!(export.texture_copies.len(), 1);
        assert!(
            export
                .obj_text
                .contains("usemtl minecraft_gold_block_side\n")
        );
        assert!(!export.material_library_text.contains("stone.png"));
        assert!(!export.material_library_text.contains("dirt.png"));
    }

    #[test]
    fn obj_export_from_mesh_face_groups_should_keep_transparent_overlapping_faces() {
        let pack = TestPack::new();
        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");

        let export = obj_export_from_mesh_face_groups(
            "test export",
            "selection.mtl",
            "selection",
            [
                vec![mesh_face(
                    "minecraft_glass_side",
                    [
                        [1.0, 0.0, 0.0],
                        [1.0, 1.0, 0.0],
                        [1.0, 1.0, 1.0],
                        [1.0, 0.0, 1.0],
                    ],
                )],
                vec![mesh_face(
                    "minecraft_stone_side",
                    [
                        [1.0, 0.0, 0.0],
                        [1.0, 0.0, 1.0],
                        [1.0, 1.0, 1.0],
                        [1.0, 1.0, 0.0],
                    ],
                )],
            ],
            &resolver,
        );

        assert_eq!(export.obj_text.matches("\nf ").count(), 2);
        assert_eq!(export.material_count, 2);
        assert!(export.obj_text.contains("usemtl minecraft_glass_side\n"));
        assert!(export.obj_text.contains("usemtl minecraft_stone_side\n"));
    }

    #[test]
    fn obj_export_from_face_sources_should_build_faces_and_resolve_package_textures() {
        let package = TestPack::new();
        package.write_bytes("data/resource_packs/vanilla/textures/.keep", b"");
        package.write(
            "data/resource_packs/vanilla_1.21.0/textures/terrain_texture.json",
            r#"{"texture_data":{"test_top":{"textures":"textures/blocks/test_top"}}}"#,
        );
        package.write_bytes(
            "data/resource_packs/vanilla_1.21.0/textures/blocks/test_top.png",
            b"png",
        );
        let source = TestFaceSource {
            material: "minecraft_test_up",
        };
        let mut progress = Vec::new();

        let export = obj_export_from_face_sources_with_package_roots(
            "face source export",
            "selection.mtl",
            "selection",
            [&source],
            [package.path()],
            "textures",
            |done, total| progress.push((done, total)),
        );

        assert!(export.obj_text.contains("usemtl minecraft_test_up\n"));
        assert!(
            export
                .material_library_text
                .contains("map_Kd textures/blocks/test_top.png")
        );
        assert_eq!(export.texture_copies.len(), 1);
        assert_eq!(progress, [(0, 1), (1, 1)]);
    }

    #[test]
    fn obj_mesh_faces_from_source_should_skip_incomplete_faces() {
        let source = TestSparseFaceSource;

        let faces = obj_mesh_faces_from_source(&source);

        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].material, Cow::Borrowed("minecraft_test_up"));
    }

    #[test]
    fn obj_default_face_uvs_should_scale_with_face_edges() {
        let uv = obj_default_face_uvs_from_corners(&[
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 3.0, 0.0],
            [0.0, 3.0, 0.0],
        ]);

        assert_eq!(uv, [[0.0, 0.0], [2.0, 0.0], [2.0, 3.0], [0.0, 3.0]]);
    }

    #[test]
    fn obj_face_normal_from_triangle_should_return_dominant_axis() {
        assert_eq!(
            obj_face_normal_from_triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
            Some([0, 0, 1])
        );
        assert_eq!(
            obj_face_normal_from_triangle([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, -1.0]),
            Some([1, 0, 0])
        );
    }

    #[test]
    fn obj_document_string_should_write_header_and_parts() {
        let text = obj_document_string(
            "bedrock-block-model OBJ export",
            "selection.mtl",
            "bedrock_block_model_selection",
            ["usemtl minecraft_test_side\n", "f 1/1 2/2 3/3 4/4\n"],
        );

        assert!(text.starts_with("# bedrock-block-model OBJ export\n"));
        assert!(text.contains("mtllib selection.mtl\n"));
        assert!(text.contains("o bedrock_block_model_selection\n"));
        assert!(text.ends_with("f 1/1 2/2 3/3 4/4\n"));
    }

    #[test]
    fn face_texture_slot_suffix_should_match_obj_export_convention() {
        assert_eq!(obj_face_texture_slot_suffix([0, 1, 0]), "up");
        assert_eq!(obj_face_texture_slot_suffix([0, -1, 0]), "down");
        assert_eq!(obj_face_texture_slot_suffix([1, 0, 0]), "side");
    }

    #[test]
    fn material_name_helpers_should_sanitize_slots_and_append_face_suffixes() {
        assert_eq!(
            obj_material_name_for_block("minecraft:grass block"),
            "minecraft_grass_block"
        );
        assert_eq!(obj_material_name_for_block(" ! "), "minecraft_unknown");
        assert_eq!(
            obj_material_name_for_face("minecraft_test", [0, 1, 0]),
            "minecraft_test_up"
        );
        assert_eq!(
            obj_material_name_for_slot("minecraft_chest", "front latch"),
            "minecraft_chest__mat_front_latch"
        );
        assert_eq!(obj_material_slot_component("*"), "default");
        assert_eq!(obj_material_slot_component(" ! "), "default");
    }

    #[test]
    fn material_library_from_export_materials_should_wrap_named_materials() {
        let mut materials = BTreeMap::new();
        materials.insert(
            "minecraft_stone_side",
            ObjExportMaterial::from_preview_color(
                "minecraft_stone_side",
                [0.25, 0.5, 0.75, 0.4],
                Some(PathBuf::from("stone.png")),
                Some("textures/blocks/stone.png".to_owned()),
            ),
        );

        let material_library = obj_material_library_from_export_materials(&materials);

        assert!(material_library.contains("newmtl minecraft_stone_side"));
        assert!(material_library.contains("map_Kd textures/blocks/stone.png"));
    }

    #[test]
    fn export_from_parts_should_build_obj_mtl_texture_copies_and_counts() {
        let mut materials = BTreeMap::new();
        materials.insert(
            "minecraft_web_side",
            ObjExportMaterial::from_preview_color(
                "minecraft_web_side",
                [1.0, 1.0, 1.0, 1.0],
                Some(PathBuf::from("textures/blocks/web.png")),
                Some("textures/blocks/web.png".to_owned()),
            ),
        );

        let export = super::obj_export_from_parts(
            "bedrock-block-model OBJ export",
            "selection.mtl",
            "bedrock_block_model_selection",
            &materials,
            ["usemtl minecraft_web_side\n"],
        );

        assert!(export.obj_text.contains("mtllib selection.mtl"));
        assert!(
            export
                .material_library_text
                .contains("newmtl minecraft_web_side")
        );
        assert_eq!(export.material_count, 1);
        assert_eq!(export.textured_material_count, 1);
        assert_eq!(export.texture_copies.len(), 2);
    }

    #[test]
    fn texture_copies_should_include_diffuse_and_alpha_mask_targets() {
        let material = ObjExportMaterial::from_preview_color(
            "minecraft_web_side",
            [1.0, 1.0, 1.0, 1.0],
            Some(PathBuf::from("textures/blocks/web.tga")),
            Some("textures/blocks/web.png".to_owned()),
        );

        let copies = obj_texture_copies([&material]);

        assert_eq!(copies.len(), 2);
        assert!(
            copies
                .iter()
                .any(|copy| copy.relative_path == "textures/blocks/web.png"
                    && !copy.alpha_mask
                    && copy.needs_png_conversion())
        );
        assert!(
            copies
                .iter()
                .any(|copy| copy.relative_path == "textures/blocks/web_alpha.png"
                    && copy.alpha_mask
                    && copy.needs_png_conversion())
        );
    }

    #[test]
    fn write_obj_texture_copy_should_write_alpha_mask_png_from_source_alpha() {
        let pack = TestPack::new();
        let source_path = pack.path().join("web.png");
        let target_path = pack.path().join("web_alpha.png");
        let mut image = image::RgbaImage::new(2, 1);
        image.put_pixel(0, 0, image::Rgba([10, 20, 30, 0]));
        image.put_pixel(1, 0, image::Rgba([200, 210, 220, 255]));
        must_ok(image.save(&source_path));
        let texture_copy = ObjTextureCopy {
            source_path,
            relative_path: "textures/blocks/web_alpha.png".to_owned(),
            tint: None,
            alpha_mask: true,
        };

        must_ok(write_obj_texture_copy(&texture_copy, &target_path));

        let mask = must_ok(image::open(&target_path)).to_rgb8();
        assert_eq!(mask.get_pixel(0, 0), &image::Rgb([0, 0, 0]));
        assert_eq!(
            mask.get_pixel(mask.width() - 1, 0),
            &image::Rgb([255, 255, 255])
        );
    }

    #[test]
    fn write_obj_texture_copy_should_apply_biome_tint_to_png() {
        let pack = TestPack::new();
        let source_path = pack.path().join("grass.png");
        let target_path = pack.path().join("grass_tinted.png");
        let mut image = image::RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([100, 150, 200, 255]));
        must_ok(image.save(&source_path));
        let texture_copy = ObjTextureCopy {
            source_path,
            relative_path: "textures/blocks/grass.png".to_owned(),
            tint: Some([0.5, 1.0, 0.25]),
            alpha_mask: false,
        };

        must_ok(write_obj_texture_copy(&texture_copy, &target_path));

        let tinted = must_ok(image::open(&target_path)).to_rgba8();
        assert_eq!(tinted.get_pixel(0, 0), &image::Rgba([50, 150, 50, 255]));
    }

    #[test]
    fn write_obj_export_files_should_write_document_materials_and_textures() {
        let pack = TestPack::new();
        let source_path = pack.path().join("pack/textures/blocks/grass.png");
        let mut image = image::RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([100, 150, 200, 255]));
        must_ok(fs::create_dir_all(
            source_path
                .parent()
                .unwrap_or_else(|| panic!("source texture should have parent")),
        ));
        must_ok(image.save(&source_path));
        let export = super::ObjExport {
            obj_text: "# obj\n".to_owned(),
            material_library_text: "# mtl\n".to_owned(),
            texture_copies: vec![ObjTextureCopy {
                source_path,
                relative_path: "textures/blocks/grass.png".to_owned(),
                tint: Some([0.5, 1.0, 0.25]),
                alpha_mask: false,
            }],
            material_count: 1,
            textured_material_count: 1,
        };
        let export_root = pack.path().join("export");
        let obj_path = export_root.join("selection.obj");
        let material_path = export_root.join("selection.mtl");

        let summary = must_ok(super::write_obj_export_files(
            &export,
            &obj_path,
            &material_path,
            &export_root,
        ));

        assert_eq!(summary.texture_copy_count, 1);
        assert_eq!(must_ok(fs::read_to_string(&obj_path)), "# obj\n");
        assert_eq!(must_ok(fs::read_to_string(&material_path)), "# mtl\n");
        let copied = must_ok(image::open(export_root.join("textures/blocks/grass.png"))).to_rgba8();
        assert_eq!(copied.get_pixel(0, 0), &image::Rgba([50, 150, 50, 255]));
    }

    #[test]
    fn write_obj_export_files_should_reject_texture_paths_outside_export_root() {
        let pack = TestPack::new();
        let source_path = pack.path().join("stone.png");
        must_ok(fs::write(&source_path, b"not used"));
        let export = super::ObjExport {
            obj_text: "# obj\n".to_owned(),
            material_library_text: "# mtl\n".to_owned(),
            texture_copies: vec![ObjTextureCopy {
                source_path,
                relative_path: "../outside.png".to_owned(),
                tint: None,
                alpha_mask: false,
            }],
            material_count: 1,
            textured_material_count: 1,
        };
        let export_root = pack.path().join("export");
        let obj_path = export_root.join("selection.obj");
        let material_path = export_root.join("selection.mtl");

        let error = super::write_obj_export_files(&export, &obj_path, &material_path, &export_root)
            .unwrap_err();

        assert!(error.to_string().contains("inside export root"));
    }

    struct TestPack {
        directory: TempDir,
    }

    impl TestPack {
        fn new() -> Self {
            Self {
                directory: must_ok(TempDir::new()),
            }
        }

        fn path(&self) -> PathBuf {
            self.directory.path().to_path_buf()
        }

        fn write(&self, relative_path: &str, content: &str) {
            self.write_bytes(relative_path, content.as_bytes());
        }

        fn write_bytes(&self, relative_path: &str, content: &[u8]) {
            let path = self.directory.path().join(relative_path);
            if let Some(parent) = path.parent() {
                must_ok(fs::create_dir_all(parent));
            }
            must_ok(fs::write(path, content));
        }
    }

    fn must_ok<T, E: std::fmt::Debug>(result: std::result::Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    }

    fn resolved_relative_path(
        resolver: &ObjTextureResolver,
        material: &str,
        normal: [i32; 3],
    ) -> Option<String> {
        resolver
            .texture_for(material, normal)
            .map(|texture| texture.relative_path)
    }

    fn mesh_face(material: &'static str, positions: [[f32; 3]; 4]) -> ObjMeshFace<'static> {
        ObjMeshFace {
            material: Cow::Borrowed(material),
            color: [1.0, 1.0, 1.0, 1.0],
            triangle_positions: [
                positions[0],
                positions[1],
                positions[2],
                positions[0],
                positions[2],
                positions[3],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        }
    }

    struct TestFaceSource {
        material: &'static str,
    }

    impl ObjMeshFaceSource for TestFaceSource {
        fn obj_face_count(&self) -> usize {
            1
        }

        fn obj_face_material(&self, face_index: usize) -> Option<&str> {
            (face_index == 0).then_some(self.material)
        }

        fn obj_face_color(&self, face_index: usize) -> Option<[f32; 4]> {
            (face_index == 0).then_some([1.0, 1.0, 1.0, 1.0])
        }

        fn obj_face_triangle_positions(&self, face_index: usize) -> Option<[[f32; 3]; 6]> {
            (face_index == 0).then_some([
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, -1.0],
                [0.0, 0.0, 0.0],
                [1.0, 0.0, -1.0],
                [1.0, 0.0, 0.0],
            ])
        }

        fn obj_face_uv(&self, face_index: usize) -> Option<[[f32; 2]; 4]> {
            (face_index == 0).then_some([[0.0, 0.0]; 4])
        }
    }

    struct TestSparseFaceSource;

    impl ObjMeshFaceSource for TestSparseFaceSource {
        fn obj_face_count(&self) -> usize {
            2
        }

        fn obj_face_material(&self, face_index: usize) -> Option<&str> {
            match face_index {
                0 => Some("minecraft_test_up"),
                1 => None,
                _ => None,
            }
        }

        fn obj_face_color(&self, face_index: usize) -> Option<[f32; 4]> {
            (face_index == 0).then_some([1.0, 1.0, 1.0, 1.0])
        }

        fn obj_face_triangle_positions(&self, face_index: usize) -> Option<[[f32; 3]; 6]> {
            TestFaceSource {
                material: "minecraft_test_up",
            }
            .obj_face_triangle_positions(face_index)
        }

        fn obj_face_uv(&self, face_index: usize) -> Option<[[f32; 2]; 4]> {
            (face_index == 0).then_some([[0.0, 0.0]; 4])
        }
    }

    #[test]
    fn texture_tint_should_fallback_to_standard_minecraft_biome_colors() {
        let grass_tint =
            super::obj_material_texture_tint("minecraft_grass_block_up", [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(grass_tint, Some([0.48, 0.74, 0.32]));

        let water_tint =
            super::obj_material_texture_tint("minecraft_flowing_water_up", [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(water_tint, Some([0.267, 0.686, 0.961]));
    }
}
