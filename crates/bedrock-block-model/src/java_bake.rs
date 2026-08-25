use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::{BlockModelError, JavaModelRepository, Result};

pub const JAVA_MODEL_DB_SCHEMA: u32 = 1;
const MAGIC: &[u8; 8] = b"BMCBJDB1";
const NONE_U32: u32 = u32::MAX;
const FACE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaModelBakeStats {
    pub schema: u32,
    pub source_version: String,
    pub client_sha1: Option<String>,
    pub blocks: u32,
    pub variants: u32,
    pub multipart_parts: u32,
    pub clauses: u32,
    pub predicates: u32,
    pub applies: u32,
    pub referenced_model_ids: u32,
    pub unique_models: u32,
    pub strings: u32,
    pub model_data_bytes: u32,
    pub string_data_bytes: u32,
    pub database_bytes: u32,
    pub warnings: u32,
}

#[derive(Clone, Debug, Default)]
struct ResolvedRawModel {
    elements: Vec<Value>,
}

#[derive(Clone, Copy, Debug)]
struct BlockRecord {
    block_id: u32,
    variant_start: u32,
    variant_count: u32,
    multipart_start: u32,
    multipart_count: u32,
}

#[derive(Clone, Copy, Debug)]
struct RuleRecord {
    clause_start: u32,
    clause_count: u32,
    apply_start: u32,
    apply_count: u32,
}

#[derive(Clone, Copy, Debug)]
struct ClauseRecord {
    predicate_start: u32,
    predicate_count: u32,
}

#[derive(Clone, Copy, Debug)]
struct PredicateRecord {
    key: u32,
    values: u32,
}

#[derive(Clone, Copy, Debug)]
struct ApplyRecord {
    model: u32,
    x: i16,
    y: i16,
    weight: u16,
    flags: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PredicateText {
    key: String,
    values: String,
}

type ConditionDnf = Vec<Vec<PredicateText>>;

#[derive(Default)]
struct StringPool {
    ids: BTreeMap<String, u32>,
    values: Vec<String>,
}

impl StringPool {
    fn intern(&mut self, value: impl AsRef<str>) -> Result<u32> {
        let value = value.as_ref();
        if let Some(id) = self.ids.get(value) {
            return Ok(*id);
        }
        let id = u32_len(self.values.len(), "string pool")?;
        self.values.push(value.to_owned());
        self.ids.insert(value.to_owned(), id);
        Ok(id)
    }
}

struct BakeBuilder {
    assets_root: PathBuf,
    strings: StringPool,
    blocks: Vec<BlockRecord>,
    variants: Vec<RuleRecord>,
    multipart_parts: Vec<RuleRecord>,
    clauses: Vec<ClauseRecord>,
    predicates: Vec<PredicateRecord>,
    applies: Vec<ApplyRecord>,
    model_cache: BTreeMap<String, ResolvedRawModel>,
    model_ids: BTreeMap<String, u32>,
    geometry_ids: BTreeMap<Vec<u8>, u32>,
    models: Vec<Vec<u8>>,
    warnings: u32,
}

impl BakeBuilder {
    fn new(assets_root: PathBuf) -> Self {
        Self {
            assets_root,
            strings: StringPool::default(),
            blocks: Vec::new(),
            variants: Vec::new(),
            multipart_parts: Vec::new(),
            clauses: Vec::new(),
            predicates: Vec::new(),
            applies: Vec::new(),
            model_cache: BTreeMap::new(),
            model_ids: BTreeMap::new(),
            geometry_ids: BTreeMap::new(),
            models: Vec::new(),
            warnings: 0,
        }
    }

    fn compile_blockstates(&mut self) -> Result<()> {
        let blockstates_root = self.assets_root.join("minecraft").join("blockstates");
        if !blockstates_root.is_dir() {
            return Err(BlockModelError::Message(format!(
                "Java assets do not contain minecraft/blockstates: {}",
                blockstates_root.display()
            )));
        }

        let mut files = Vec::new();
        for entry in WalkDir::new(&blockstates_root) {
            let entry = entry.map_err(|source| BlockModelError::Walk {
                path: blockstates_root.clone(),
                source,
            })?;
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|extension| extension == "json")
            {
                files.push(entry.into_path());
            }
        }
        files.sort();

        for path in files {
            self.compile_blockstate(&blockstates_root, &path)?;
        }
        Ok(())
    }

    fn compile_blockstate(&mut self, root: &Path, path: &Path) -> Result<()> {
        let value = read_json(path)?;
        let object = value.as_object().ok_or_else(|| {
            BlockModelError::Message(format!(
                "Java blockstate is not an object: {}",
                path.display()
            ))
        })?;

        let relative = path.strip_prefix(root).map_err(|_| {
            BlockModelError::Message(format!(
                "Java blockstate escaped blockstates root: {}",
                path.display()
            ))
        })?;
        let mut block_path = relative.with_extension("").to_string_lossy().replace('\\', "/");
        if block_path.starts_with('/') {
            block_path.remove(0);
        }
        let block_id = self.strings.intern(format!("minecraft:{block_path}"))?;

        let variant_start = u32_len(self.variants.len(), "variant records")?;
        if let Some(variants) = object.get("variants").and_then(Value::as_object) {
            let mut selectors = variants.keys().cloned().collect::<Vec<_>>();
            selectors.sort();
            for selector in selectors {
                let apply = variants.get(&selector).expect("selector came from variants");
                let condition = variant_selector_dnf(&selector)?;
                let (clause_start, clause_count) = self.compile_condition(&condition)?;
                let (apply_start, apply_count) = self.compile_applies(apply)?;
                self.variants.push(RuleRecord {
                    clause_start,
                    clause_count,
                    apply_start,
                    apply_count,
                });
            }
        }
        let variant_count = u32_len(self.variants.len(), "variant records")? - variant_start;

        let multipart_start = u32_len(self.multipart_parts.len(), "multipart records")?;
        if let Some(parts) = object.get("multipart").and_then(Value::as_array) {
            for part in parts {
                let part = part.as_object().ok_or_else(|| {
                    BlockModelError::Message(format!(
                        "Java multipart entry is not an object: {}",
                        path.display()
                    ))
                })?;
                let condition = match part.get("when") {
                    Some(when) => multipart_condition_dnf(when)?,
                    None => vec![Vec::new()],
                };
                let apply = part.get("apply").ok_or_else(|| {
                    BlockModelError::Message(format!(
                        "Java multipart entry has no apply: {}",
                        path.display()
                    ))
                })?;
                let (clause_start, clause_count) = self.compile_condition(&condition)?;
                let (apply_start, apply_count) = self.compile_applies(apply)?;
                self.multipart_parts.push(RuleRecord {
                    clause_start,
                    clause_count,
                    apply_start,
                    apply_count,
                });
            }
        }
        let multipart_count =
            u32_len(self.multipart_parts.len(), "multipart records")? - multipart_start;

        self.blocks.push(BlockRecord {
            block_id,
            variant_start,
            variant_count,
            multipart_start,
            multipart_count,
        });
        Ok(())
    }

    fn compile_condition(&mut self, dnf: &ConditionDnf) -> Result<(u32, u32)> {
        let clause_start = u32_len(self.clauses.len(), "condition clauses")?;
        for clause in dnf {
            let predicate_start = u32_len(self.predicates.len(), "condition predicates")?;
            let mut predicates = clause.clone();
            predicates.sort_by(|left, right| {
                left.key
                    .cmp(&right.key)
                    .then_with(|| left.values.cmp(&right.values))
            });
            for predicate in predicates {
                let key = self.strings.intern(predicate.key)?;
                let values = self.strings.intern(predicate.values)?;
                self.predicates.push(PredicateRecord { key, values });
            }
            let predicate_count =
                u32_len(self.predicates.len(), "condition predicates")? - predicate_start;
            self.clauses.push(ClauseRecord {
                predicate_start,
                predicate_count,
            });
        }
        Ok((
            clause_start,
            u32_len(self.clauses.len(), "condition clauses")? - clause_start,
        ))
    }

    fn compile_applies(&mut self, value: &Value) -> Result<(u32, u32)> {
        let apply_start = u32_len(self.applies.len(), "model applications")?;
        match value {
            Value::Array(items) => {
                for item in items {
                    self.compile_apply(item)?;
                }
            }
            Value::Object(_) => self.compile_apply(value)?,
            _ => {
                return Err(BlockModelError::Message(
                    "Java blockstate apply must be an object or array".to_owned(),
                ));
            }
        }
        let count = u32_len(self.applies.len(), "model applications")? - apply_start;
        if count == 0 {
            return Err(BlockModelError::Message(
                "Java blockstate apply array must not be empty".to_owned(),
            ));
        }
        Ok((apply_start, count))
    }

    fn compile_apply(&mut self, value: &Value) -> Result<()> {
        let object = value.as_object().ok_or_else(|| {
            BlockModelError::Message("Java model application is not an object".to_owned())
        })?;
        let model_name = object.get("model").and_then(Value::as_str).ok_or_else(|| {
            BlockModelError::Message("Java model application has no model id".to_owned())
        })?;
        let model = self.compile_model(model_name)?;
        let x = i16_json(object.get("x"), 0, "blockstate x rotation")?;
        let y = i16_json(object.get("y"), 0, "blockstate y rotation")?;
        if i32::from(x).rem_euclid(90) != 0 || i32::from(y).rem_euclid(90) != 0 {
            return Err(BlockModelError::Message(format!(
                "Java blockstate rotation for {model_name} is not a multiple of 90: x={x}, y={y}"
            )));
        }
        let weight = match object.get("weight").and_then(Value::as_u64) {
            Some(value) => u16::try_from(value).map_err(|_| {
                BlockModelError::Message(format!(
                    "Java model weight is outside u16 range for {model_name}: {value}"
                ))
            })?,
            None => 1,
        };
        if weight == 0 {
            return Err(BlockModelError::Message(format!(
                "Java model weight must be positive for {model_name}"
            )));
        }
        let mut flags = 0_u16;
        if object.get("uvlock").and_then(Value::as_bool).unwrap_or(false) {
            flags |= 1;
        }
        self.applies.push(ApplyRecord {
            model,
            x,
            y,
            weight,
            flags,
        });
        Ok(())
    }

    fn compile_model(&mut self, id: &str) -> Result<u32> {
        if let Some(model) = self.model_ids.get(id) {
            return Ok(*model);
        }
        let resolved = self.resolve_model(id, 0)?;
        let packed = self.pack_model(&resolved)?;
        let model = if let Some(existing) = self.geometry_ids.get(&packed) {
            *existing
        } else {
            let model = u32_len(self.models.len(), "unique Java models")?;
            self.models.push(packed.clone());
            self.geometry_ids.insert(packed, model);
            model
        };
        self.model_ids.insert(id.to_owned(), model);
        Ok(model)
    }

    fn resolve_model(&mut self, id: &str, depth: usize) -> Result<ResolvedRawModel> {
        if let Some(model) = self.model_cache.get(id) {
            return Ok(model.clone());
        }
        if depth > 64 {
            return Err(BlockModelError::Message(format!(
                "Java model parent chain is too deep or cyclic at {id}"
            )));
        }
        let path = model_path(&self.assets_root, id);
        if !path.is_file() {
            let (_, model_path) = split_resource_id(id);
            if model_path.starts_with("builtin/") {
                self.warnings = self.warnings.saturating_add(1);
                let model = ResolvedRawModel::default();
                self.model_cache.insert(id.to_owned(), model.clone());
                return Ok(model);
            }
            return Err(BlockModelError::Message(format!(
                "referenced Java model does not exist: {id} ({})",
                path.display()
            )));
        }

        let value = read_json(&path)?;
        let object = value.as_object().ok_or_else(|| {
            BlockModelError::Message(format!("Java model is not an object: {}", path.display()))
        })?;
        let mut resolved = if let Some(parent) = object.get("parent").and_then(Value::as_str) {
            self.resolve_model(parent, depth + 1)?
        } else {
            ResolvedRawModel::default()
        };
        if let Some(elements) = object.get("elements").and_then(Value::as_array) {
            resolved.elements.clone_from(elements);
        }
        self.model_cache.insert(id.to_owned(), resolved.clone());
        Ok(resolved)
    }

    fn pack_model(&mut self, model: &ResolvedRawModel) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        for element in &model.elements {
            match self.pack_element(element)? {
                Some(element) => elements.push(element),
                None => self.warnings = self.warnings.saturating_add(1),
            }
        }

        let mut bytes = Vec::new();
        push_u16(&mut bytes, u16_len(elements.len(), "model elements")?);
        for element in elements {
            bytes.extend_from_slice(&element);
        }
        Ok(bytes)
    }

    fn pack_element(&mut self, value: &Value) -> Result<Option<Vec<u8>>> {
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let Some(from) = object.get("from").and_then(vector3_f64) else {
            return Ok(None);
        };
        let Some(to) = object.get("to").and_then(vector3_f64) else {
            return Ok(None);
        };

        let mut bytes = Vec::new();
        for value in from.into_iter().chain(to) {
            push_i16(&mut bytes, quantize_java_unit(value, "element coordinate")?);
        }

        let mut rotation_axis = 0_u8;
        let mut rotation_flags = 0_u8;
        let mut rotation_angle = 0_i16;
        let mut rotation_origin = [0_i16; 3];
        if let Some(rotation) = object.get("rotation").and_then(Value::as_object) {
            rotation_axis = match rotation.get("axis").and_then(Value::as_str) {
                Some("x") => 1,
                Some("y") => 2,
                Some("z") => 3,
                Some(other) => {
                    return Err(BlockModelError::Message(format!(
                        "unsupported Java element rotation axis: {other}"
                    )));
                }
                None => 0,
            };
            if rotation.get("rescale").and_then(Value::as_bool).unwrap_or(false) {
                rotation_flags |= 1;
            }
            if let Some(angle) = rotation.get("angle").and_then(Value::as_f64) {
                rotation_angle = quantize_angle(angle)?;
            }
            if let Some(origin) = rotation.get("origin").and_then(vector3_f64) {
                for (index, value) in origin.into_iter().enumerate() {
                    rotation_origin[index] = quantize_java_unit(value, "rotation origin")?;
                }
            }
        }
        bytes.push(rotation_axis);
        bytes.push(rotation_flags);
        push_i16(&mut bytes, rotation_angle);
        for value in rotation_origin {
            push_i16(&mut bytes, value);
        }
        bytes.push(u8::from(object.get("shade").and_then(Value::as_bool).unwrap_or(true)));

        let mut packed_faces = Vec::new();
        if let Some(faces) = object.get("faces").and_then(Value::as_object) {
            for (face_id, face_name) in FACE_NAMES.iter().enumerate() {
                let Some(face) = faces.get(*face_name).and_then(Value::as_object) else {
                    continue;
                };
                let texture = face.get("texture").and_then(Value::as_str).unwrap_or_default();
                let slot = semantic_material_slot(texture, face_name);
                let slot_id = self.strings.intern(slot)?;
                let mut face_bytes = Vec::new();
                face_bytes.push(u8::try_from(face_id).expect("six block faces fit in u8"));
                push_u32(&mut face_bytes, slot_id);

                let mut flags = 0_u8;
                let uv = face.get("uv").and_then(vector4_f64);
                if uv.is_some() {
                    flags |= 1;
                }
                let cull_face = face
                    .get("cullface")
                    .and_then(Value::as_str)
                    .and_then(face_code);
                if cull_face.is_some() {
                    flags |= 2;
                }
                let tint_index = face.get("tintindex").and_then(Value::as_i64);
                if tint_index.is_some() {
                    flags |= 4;
                }
                face_bytes.push(flags);

                let rotation = face.get("rotation").and_then(Value::as_i64).unwrap_or(0);
                if rotation.rem_euclid(90) != 0 {
                    return Err(BlockModelError::Message(format!(
                        "Java face rotation is not a multiple of 90: {rotation}"
                    )));
                }
                let turns = u8::try_from(rotation.rem_euclid(360) / 90)
                    .expect("quarter turns fit in u8");
                face_bytes.push(turns);
                face_bytes.push(cull_face.unwrap_or(u8::MAX));
                let tint = match tint_index {
                    Some(value) => i16::try_from(value).map_err(|_| {
                        BlockModelError::Message(format!(
                            "Java tint index is outside i16 range: {value}"
                        ))
                    })?,
                    None => -1,
                };
                push_i16(&mut face_bytes, tint);
                if let Some(uv) = uv {
                    for value in uv {
                        push_i16(&mut face_bytes, quantize_uv(value)?);
                    }
                }
                packed_faces.push(face_bytes);
            }
        }
        bytes.push(u8_len(packed_faces.len(), "element faces")?);
        for face in packed_faces {
            bytes.extend_from_slice(&face);
        }
        Ok(Some(bytes))
    }

    fn finish(
        mut self,
        source_version: &str,
        client_sha1: Option<&str>,
    ) -> Result<(Vec<u8>, JavaModelBakeStats)> {
        let source_version_id = self.strings.intern(source_version)?;
        let client_sha1_id = match client_sha1 {
            Some(value) => self.strings.intern(value)?,
            None => NONE_U32,
        };

        let block_section = encode_blocks(&self.blocks);
        let variant_section = encode_rules(&self.variants);
        let multipart_section = encode_rules(&self.multipart_parts);
        let clause_section = encode_clauses(&self.clauses);
        let predicate_section = encode_predicates(&self.predicates);
        let apply_section = encode_applies(&self.applies);
        let (model_index, model_data) = encode_blob_pool(&self.models)?;
        let string_blobs = self
            .strings
            .values
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let (string_index, string_data) = encode_blob_pool(&string_blobs)?;

        let header_size = 104_u32;
        let mut offset = header_size;
        let blocks_offset = take_offset(&mut offset, block_section.len(), "block section")?;
        let variants_offset = take_offset(&mut offset, variant_section.len(), "variant section")?;
        let multipart_offset = take_offset(&mut offset, multipart_section.len(), "multipart section")?;
        let clauses_offset = take_offset(&mut offset, clause_section.len(), "clause section")?;
        let predicates_offset = take_offset(&mut offset, predicate_section.len(), "predicate section")?;
        let applies_offset = take_offset(&mut offset, apply_section.len(), "apply section")?;
        let model_index_offset = take_offset(&mut offset, model_index.len(), "model index")?;
        let model_data_offset = take_offset(&mut offset, model_data.len(), "model data")?;
        let string_index_offset = take_offset(&mut offset, string_index.len(), "string index")?;
        let string_data_offset = take_offset(&mut offset, string_data.len(), "string data")?;
        let file_size = offset;

        let mut bytes = Vec::with_capacity(usize::try_from(file_size).unwrap_or(0));
        bytes.extend_from_slice(MAGIC);
        for value in [
            JAVA_MODEL_DB_SCHEMA,
            header_size,
            source_version_id,
            client_sha1_id,
            u32_len(self.blocks.len(), "blocks")?,
            u32_len(self.variants.len(), "variants")?,
            u32_len(self.multipart_parts.len(), "multipart parts")?,
            u32_len(self.clauses.len(), "clauses")?,
            u32_len(self.predicates.len(), "predicates")?,
            u32_len(self.applies.len(), "applies")?,
            u32_len(self.models.len(), "models")?,
            u32_len(self.strings.values.len(), "strings")?,
            blocks_offset,
            variants_offset,
            multipart_offset,
            clauses_offset,
            predicates_offset,
            applies_offset,
            model_index_offset,
            model_data_offset,
            string_index_offset,
            string_data_offset,
            file_size,
            0,
        ] {
            push_u32(&mut bytes, value);
        }
        debug_assert_eq!(bytes.len(), usize::try_from(header_size).unwrap_or_default());
        bytes.extend_from_slice(&block_section);
        bytes.extend_from_slice(&variant_section);
        bytes.extend_from_slice(&multipart_section);
        bytes.extend_from_slice(&clause_section);
        bytes.extend_from_slice(&predicate_section);
        bytes.extend_from_slice(&apply_section);
        bytes.extend_from_slice(&model_index);
        bytes.extend_from_slice(&model_data);
        bytes.extend_from_slice(&string_index);
        bytes.extend_from_slice(&string_data);

        let stats = JavaModelBakeStats {
            schema: JAVA_MODEL_DB_SCHEMA,
            source_version: source_version.to_owned(),
            client_sha1: client_sha1.map(str::to_owned),
            blocks: u32_len(self.blocks.len(), "blocks")?,
            variants: u32_len(self.variants.len(), "variants")?,
            multipart_parts: u32_len(self.multipart_parts.len(), "multipart parts")?,
            clauses: u32_len(self.clauses.len(), "clauses")?,
            predicates: u32_len(self.predicates.len(), "predicates")?,
            applies: u32_len(self.applies.len(), "applies")?,
            referenced_model_ids: u32_len(self.model_ids.len(), "referenced model ids")?,
            unique_models: u32_len(self.models.len(), "unique models")?,
            strings: u32_len(self.strings.values.len(), "strings")?,
            model_data_bytes: u32_len(model_data.len(), "model data bytes")?,
            string_data_bytes: u32_len(string_data.len(), "string data bytes")?,
            database_bytes: u32_len(bytes.len(), "database bytes")?,
            warnings: self.warnings,
        };
        Ok((bytes, stats))
    }
}

/// Compiles extracted Java Edition blockstate/model JSON into a deterministic packed database.
///
/// The database stores blockstate conditions, weighted model applications, blockstate transforms,
/// parent-resolved model elements, element rotations, face UV metadata and semantic material slots.
/// Source JSON and Java client files are not needed at runtime after this step.
///
/// # Errors
///
/// Returns an error for malformed Java JSON, invalid model references, unsupported numeric ranges,
/// or output I/O failures.
pub fn bake_java_model_database(
    root: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<JavaModelBakeStats> {
    let repository = JavaModelRepository::from_root(root.as_ref())?;
    let assets_root = repository.assets_root().to_path_buf();
    let (source_version, client_sha1) = source_metadata(&assets_root)?;
    let mut builder = BakeBuilder::new(assets_root);
    builder.compile_blockstates()?;
    let (bytes, stats) = builder.finish(&source_version, client_sha1.as_deref())?;
    write_atomic(output.as_ref(), &bytes)?;
    Ok(stats)
}

fn source_metadata(assets_root: &Path) -> Result<(String, Option<String>)> {
    let manifest_path = assets_root
        .parent()
        .map(|parent| parent.join("manifest.json"))
        .unwrap_or_else(|| PathBuf::from("manifest.json"));
    if !manifest_path.is_file() {
        return Ok(("unknown".to_owned(), None));
    }
    let value = read_json(&manifest_path)?;
    let version = value
        .pointer("/version/id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let client_sha1 = value
        .pointer("/client/sha1")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((version, client_sha1))
}

fn variant_selector_dnf(selector: &str) -> Result<ConditionDnf> {
    if selector.trim().is_empty() {
        return Ok(vec![Vec::new()]);
    }
    let mut clause = Vec::new();
    for term in selector.split(',') {
        let (key, values) = term.split_once('=').ok_or_else(|| {
            BlockModelError::Message(format!("invalid Java variant selector: {selector}"))
        })?;
        clause.push(PredicateText {
            key: key.trim().to_owned(),
            values: normalize_values(values),
        });
    }
    Ok(vec![clause])
}

fn multipart_condition_dnf(value: &Value) -> Result<ConditionDnf> {
    let object = value.as_object().ok_or_else(|| {
        BlockModelError::Message("Java multipart condition must be an object".to_owned())
    })?;
    let mut result = vec![Vec::new()];
    let mut entries = object.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (key, expected) in entries {
        let term = match key.as_str() {
            "OR" => {
                let items = expected.as_array().ok_or_else(|| {
                    BlockModelError::Message("Java multipart OR must be an array".to_owned())
                })?;
                let mut alternatives = Vec::new();
                for item in items {
                    alternatives.extend(multipart_condition_dnf(item)?);
                }
                alternatives
            }
            "AND" => {
                let items = expected.as_array().ok_or_else(|| {
                    BlockModelError::Message("Java multipart AND must be an array".to_owned())
                })?;
                let mut conjunction = vec![Vec::new()];
                for item in items {
                    conjunction = and_dnf(conjunction, multipart_condition_dnf(item)?)?;
                }
                conjunction
            }
            _ => vec![vec![PredicateText {
                key: key.to_owned(),
                values: condition_value_string(expected)?,
            }]],
        };
        result = and_dnf(result, term)?;
    }
    Ok(result)
}

fn and_dnf(left: ConditionDnf, right: ConditionDnf) -> Result<ConditionDnf> {
    let size = left.len().checked_mul(right.len()).ok_or_else(|| {
        BlockModelError::Message("Java multipart condition expansion overflowed".to_owned())
    })?;
    if size > 4096 {
        return Err(BlockModelError::Message(format!(
            "Java multipart condition expands to too many clauses: {size}"
        )));
    }
    let mut output = Vec::with_capacity(size);
    for left_clause in left {
        for right_clause in &right {
            let mut clause = left_clause.clone();
            clause.extend(right_clause.iter().cloned());
            output.push(clause);
        }
    }
    Ok(output)
}

fn condition_value_string(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(normalize_values(value)),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(BlockModelError::Message(
            "Java multipart property condition must be string, bool or number".to_owned(),
        )),
    }
}

fn normalize_values(values: &str) -> String {
    let mut values = values
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    values.join("|")
}

fn model_path(assets_root: &Path, id: &str) -> PathBuf {
    let (namespace, path) = split_resource_id(id);
    assets_root
        .join(namespace)
        .join("models")
        .join(format!("{path}.json"))
}

fn split_resource_id(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

fn semantic_material_slot<'a>(texture: &str, face: &'a str) -> &'a str {
    let token = texture.trim_start_matches('#');
    match token {
        "top" | "up" | "end" => "up",
        "bottom" | "down" => "down",
        "side" => "side",
        "front" => "front",
        _ => face,
    }
}

fn face_code(value: &str) -> Option<u8> {
    FACE_NAMES
        .iter()
        .position(|face| *face == value)
        .and_then(|index| u8::try_from(index).ok())
}

fn vector3_f64(value: &Value) -> Option<[f64; 3]> {
    let values = value.as_array()?;
    Some([
        values.first()?.as_f64()?,
        values.get(1)?.as_f64()?,
        values.get(2)?.as_f64()?,
    ])
}

fn vector4_f64(value: &Value) -> Option<[f64; 4]> {
    let values = value.as_array()?;
    Some([
        values.first()?.as_f64()?,
        values.get(1)?.as_f64()?,
        values.get(2)?.as_f64()?,
        values.get(3)?.as_f64()?,
    ])
}

fn quantize_java_unit(value: f64, label: &str) -> Result<i16> {
    // Java coordinates are expressed in sixteenths of a block. 256 fixed-point steps per Java
    // unit therefore yield 1/4096-block precision while retaining a compact i16 representation.
    quantize_i16(value * 256.0, label)
}

fn quantize_uv(value: f64) -> Result<i16> {
    quantize_i16(value * 256.0, "face UV")
}

fn quantize_angle(value: f64) -> Result<i16> {
    quantize_i16(value * 100.0, "element rotation angle")
}

fn quantize_i16(value: f64, label: &str) -> Result<i16> {
    if !value.is_finite() {
        return Err(BlockModelError::Message(format!(
            "Java {label} is not finite: {value}"
        )));
    }
    let rounded = value.round();
    if rounded < f64::from(i16::MIN) || rounded > f64::from(i16::MAX) {
        return Err(BlockModelError::Message(format!(
            "Java {label} is outside packed i16 range: {value}"
        )));
    }
    #[expect(clippy::cast_possible_truncation, reason = "range checked immediately above")]
    Ok(rounded as i16)
}

fn i16_json(value: Option<&Value>, default: i16, label: &str) -> Result<i16> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = value.as_i64().ok_or_else(|| {
        BlockModelError::Message(format!("Java {label} must be an integer"))
    })?;
    i16::try_from(value).map_err(|_| {
        BlockModelError::Message(format!("Java {label} is outside i16 range: {value}"))
    })
}

fn encode_blocks(records: &[BlockRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 20);
    for record in records {
        for value in [
            record.block_id,
            record.variant_start,
            record.variant_count,
            record.multipart_start,
            record.multipart_count,
        ] {
            push_u32(&mut bytes, value);
        }
    }
    bytes
}

fn encode_rules(records: &[RuleRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 16);
    for record in records {
        for value in [
            record.clause_start,
            record.clause_count,
            record.apply_start,
            record.apply_count,
        ] {
            push_u32(&mut bytes, value);
        }
    }
    bytes
}

fn encode_clauses(records: &[ClauseRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 8);
    for record in records {
        push_u32(&mut bytes, record.predicate_start);
        push_u32(&mut bytes, record.predicate_count);
    }
    bytes
}

fn encode_predicates(records: &[PredicateRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 8);
    for record in records {
        push_u32(&mut bytes, record.key);
        push_u32(&mut bytes, record.values);
    }
    bytes
}

fn encode_applies(records: &[ApplyRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 12);
    for record in records {
        push_u32(&mut bytes, record.model);
        push_i16(&mut bytes, record.x);
        push_i16(&mut bytes, record.y);
        push_u16(&mut bytes, record.weight);
        push_u16(&mut bytes, record.flags);
    }
    bytes
}

fn encode_blob_pool(blobs: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut index = Vec::with_capacity(blobs.len() * 8);
    let mut data = Vec::new();
    for blob in blobs {
        push_u32(&mut index, u32_len(data.len(), "blob offset")?);
        push_u32(&mut index, u32_len(blob.len(), "blob length")?);
        data.extend_from_slice(blob);
    }
    Ok((index, data))
}

fn take_offset(offset: &mut u32, len: usize, label: &str) -> Result<u32> {
    let current = *offset;
    *offset = offset
        .checked_add(u32_len(len, label)?)
        .ok_or_else(|| BlockModelError::Message("Java model database exceeds 4 GiB".to_owned()))?;
    Ok(current)
}

fn u32_len(value: usize, label: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| {
        BlockModelError::Message(format!("{label} exceeds the Java model database u32 limit"))
    })
}

fn u16_len(value: usize, label: &str) -> Result<u16> {
    u16::try_from(value).map_err(|_| {
        BlockModelError::Message(format!("{label} exceeds the Java model database u16 limit"))
    })
}

fn u8_len(value: usize, label: &str) -> Result<u8> {
    u8::try_from(value).map_err(|_| {
        BlockModelError::Message(format!("{label} exceeds the Java model database u8 limit"))
    })
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_i16(bytes: &mut Vec<u8>, value: i16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).map_err(|source| BlockModelError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| BlockModelError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|source| BlockModelError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("java-models.bin");
    let temporary = path.with_file_name(format!(".{name}.tmp-{}-{stamp}", std::process::id()));
    let backup = path.with_file_name(format!(".{name}.backup-{}-{stamp}", std::process::id()));

    fs::write(&temporary, bytes).map_err(|source| BlockModelError::Write {
        path: temporary.clone(),
        source,
    })?;
    let had_destination = path.exists();
    if had_destination {
        fs::rename(path, &backup).map_err(|source| BlockModelError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    if let Err(source) = fs::rename(&temporary, path) {
        if had_destination && backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&temporary);
        return Err(BlockModelError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_or_and_conditions_expand_to_dnf() {
        let value = serde_json::json!({
            "AND": [
                {"north": "true"},
                {"OR": [{"east": "true"}, {"west": "true"}]}
            ]
        });
        let dnf = multipart_condition_dnf(&value).expect("condition should compile");
        assert_eq!(dnf.len(), 2);
        assert!(dnf.iter().all(|clause| clause.len() == 2));
    }

    #[test]
    fn bake_is_deterministic_and_deduplicates_models() {
        let directory = tempfile::tempdir().expect("temp dir");
        let root = directory.path();
        let assets = root.join("assets/minecraft");
        fs::create_dir_all(assets.join("blockstates")).expect("blockstate directory");
        fs::create_dir_all(assets.join("models/block")).expect("model directory");
        fs::write(
            root.join("manifest.json"),
            r#"{"version":{"id":"test-release"},"client":{"sha1":"abc123"}}"#,
        )
        .expect("manifest");
        fs::write(
            assets.join("blockstates/test.json"),
            r#"{
                "variants": {
                    "facing=north": {"model":"minecraft:block/test"},
                    "facing=south": [
                        {"model":"minecraft:block/test","y":180,"weight":2},
                        {"model":"minecraft:block/test","y":180,"weight":1}
                    ]
                },
                "multipart": [
                    {"when":{"powered":"true"},"apply":{"model":"minecraft:block/test"}}
                ]
            }"#,
        )
        .expect("blockstate");
        fs::write(
            assets.join("models/block/test.json"),
            r##"{
                "elements": [{
                    "from":[0,0,0], "to":[16,16,16],
                    "faces":{"north":{"texture":"#side","uv":[0,0,16,16]}}
                }]
            }"##,
        )
        .expect("model");

        let first = root.join("first.bin");
        let second = root.join("second.bin");
        let first_stats = bake_java_model_database(root, &first).expect("first bake");
        let second_stats = bake_java_model_database(root, &second).expect("second bake");
        assert_eq!(first_stats, second_stats);
        assert_eq!(first_stats.source_version, "test-release");
        assert_eq!(first_stats.blocks, 1);
        assert_eq!(first_stats.variants, 2);
        assert_eq!(first_stats.multipart_parts, 1);
        assert_eq!(first_stats.applies, 4);
        assert_eq!(first_stats.referenced_model_ids, 1);
        assert_eq!(first_stats.unique_models, 1);
        let first_bytes = fs::read(first).expect("first database");
        let second_bytes = fs::read(second).expect("second database");
        assert!(first_bytes.starts_with(MAGIC));
        assert_eq!(first_bytes, second_bytes);
    }
}
