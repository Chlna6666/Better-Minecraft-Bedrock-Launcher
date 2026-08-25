use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::java_bake::JAVA_MODEL_DB_SCHEMA;
use crate::{BlockFace, BlockModelError, Result};

const MAGIC: &[u8; 8] = b"BMCBJDB1";
const HEADER_SIZE: usize = 104;
const NONE_U32: u32 = u32::MAX;
const BLOCK_RECORD_SIZE: usize = 20;
const RULE_RECORD_SIZE: usize = 16;
const CLAUSE_RECORD_SIZE: usize = 8;
const PREDICATE_RECORD_SIZE: usize = 8;
const APPLY_RECORD_SIZE: usize = 12;
const BLOB_INDEX_RECORD_SIZE: usize = 8;
const PACKED_UNIT_PER_BLOCK: f32 = 4096.0;
const PACKED_UV_PER_TEXTURE: f32 = 4096.0;

static EMBEDDED_DATABASE: OnceLock<JavaModelDatabase<'static>> = OnceLock::new();
static EMBEDDED_BYTES: &[u8] = include_bytes!("../generated/vanilla_models.bin");

/// Read-only view over the packed Java Edition model database.
///
/// Construction validates the complete file once. All normal lookups then borrow directly from
/// the backing byte slice: no JSON parsing, model-parent traversal, heap allocation, or geometry
/// copying is performed by this type.
#[derive(Clone, Copy, Debug)]
pub struct JavaModelDatabase<'a> {
    bytes: &'a [u8],
    header: Header,
}

/// Supplies Java blockstate properties without forcing the database to allocate a map.
pub trait JavaPropertySource {
    #[must_use]
    fn java_property(&self, key: &str) -> Option<&str>;
}

impl JavaPropertySource for BTreeMap<String, String> {
    fn java_property(&self, key: &str) -> Option<&str> {
        self.get(key).map(String::as_str)
    }
}

impl<'a> JavaPropertySource for [(&'a str, &'a str)] {
    fn java_property(&self, key: &str) -> Option<&str> {
        self.iter()
            .find_map(|(candidate, value)| (*candidate == key).then_some(*value))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JavaModelId(u32);

impl JavaModelId {
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// One concrete model application chosen from a Java blockstate variant or multipart part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JavaModelApplication {
    pub model: JavaModelId,
    pub x_degrees: i16,
    pub y_degrees: i16,
    pub uv_lock: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JavaModelAxis {
    X,
    Y,
    Z,
}

/// Zero-copy view of one deduplicated packed model.
#[derive(Clone, Copy, Debug)]
pub struct JavaPackedModel<'a> {
    database: JavaModelDatabase<'a>,
    bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
pub struct JavaPackedElement<'a> {
    database: JavaModelDatabase<'a>,
    pub from: [i16; 3],
    pub to: [i16; 3],
    pub rotation_axis: Option<JavaModelAxis>,
    pub rotation_origin: [i16; 3],
    pub rotation_angle_hundredths: i16,
    pub rescale: bool,
    pub shade: bool,
    face_count: u8,
    faces: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
pub struct JavaPackedFace<'a> {
    pub face: BlockFace,
    pub material_slot: &'a str,
    pub uv: Option<[i16; 4]>,
    pub rotation_quarter_turns: u8,
    pub cull_face: Option<BlockFace>,
    pub tint_index: Option<i16>,
}

#[derive(Clone, Debug)]
pub struct JavaPackedElementIter<'a> {
    database: JavaModelDatabase<'a>,
    bytes: &'a [u8],
    cursor: usize,
    remaining: u16,
}

#[derive(Clone, Debug)]
pub struct JavaPackedFaceIter<'a> {
    database: JavaModelDatabase<'a>,
    bytes: &'a [u8],
    cursor: usize,
    remaining: u8,
}

#[derive(Clone, Copy, Debug)]
struct Header {
    schema: u32,
    source_version_id: u32,
    client_sha1_id: u32,
    block_count: u32,
    variant_count: u32,
    multipart_count: u32,
    clause_count: u32,
    predicate_count: u32,
    apply_count: u32,
    model_count: u32,
    string_count: u32,
    blocks_offset: usize,
    variants_offset: usize,
    multipart_offset: usize,
    clauses_offset: usize,
    predicates_offset: usize,
    applies_offset: usize,
    model_index_offset: usize,
    model_data_offset: usize,
    string_index_offset: usize,
    string_data_offset: usize,
    file_size: usize,
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

impl<'a> JavaModelDatabase<'a> {
    /// Validates and opens a packed model database.
    ///
    /// # Errors
    ///
    /// Returns an error when the magic/schema, section layout, record references, UTF-8 string
    /// pool, or packed geometry payload is malformed.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        let header = Header::parse(bytes)?;
        let database = Self { bytes, header };
        database.validate_records()?;
        Ok(database)
    }

    #[must_use]
    pub const fn schema(self) -> u32 {
        self.header.schema
    }

    #[must_use]
    pub fn source_version(self) -> &'a str {
        self.string(self.header.source_version_id)
            .expect("validated database source version")
    }

    #[must_use]
    pub fn client_sha1(self) -> Option<&'a str> {
        (self.header.client_sha1_id != NONE_U32).then(|| {
            self.string(self.header.client_sha1_id)
                .expect("validated database client SHA-1")
        })
    }

    #[must_use]
    pub const fn block_count(self) -> u32 {
        self.header.block_count
    }

    #[must_use]
    pub const fn model_count(self) -> u32 {
        self.header.model_count
    }

    /// Visits the concrete model applications for one Java blockstate.
    ///
    /// `seed` is used only for Java weighted model arrays. Callers should derive it from a stable
    /// world/block position so random-looking vanilla variants remain deterministic while chunk
    /// remeshing. The callback runs once for the selected normal variant and once for every
    /// matching multipart part. The method itself performs no heap allocation.
    ///
    /// Returns `false` when `block_id` does not exist in the database.
    pub fn for_each_model_application<P, F>(
        self,
        block_id: &str,
        properties: &P,
        seed: u64,
        mut visit: F,
    ) -> bool
    where
        P: JavaPropertySource + ?Sized,
        F: FnMut(JavaModelApplication),
    {
        let Some((block_index, block)) = self.find_block(block_id) else {
            return false;
        };

        for offset in 0..block.variant_count {
            let rule_index = block.variant_start + offset;
            let rule = self.rule(self.header.variants_offset, rule_index);
            if self.rule_matches(rule, properties) {
                if let Some(application) = self.choose_application(
                    rule,
                    seed ^ u64::from(block_index) ^ (u64::from(rule_index) << 32),
                ) {
                    visit(application);
                }
                break;
            }
        }

        for offset in 0..block.multipart_count {
            let rule_index = block.multipart_start + offset;
            let rule = self.rule(self.header.multipart_offset, rule_index);
            if self.rule_matches(rule, properties) {
                if let Some(application) = self.choose_application(
                    rule,
                    seed ^ 0xA076_1D64_78BD_642F ^ (u64::from(rule_index) << 32),
                ) {
                    visit(application);
                }
            }
        }
        true
    }

    #[must_use]
    pub fn model(self, id: JavaModelId) -> Option<JavaPackedModel<'a>> {
        let bytes = self.model_blob(id.0)?;
        Some(JavaPackedModel {
            database: self,
            bytes,
        })
    }

    #[must_use]
    pub fn contains_block(self, block_id: &str) -> bool {
        self.find_block(block_id).is_some()
    }

    fn find_block(self, requested: &str) -> Option<(u32, BlockRecord)> {
        let mut low = 0_u32;
        let mut high = self.header.block_count;
        while low < high {
            let mid = low + (high - low) / 2;
            let record = self.block(mid);
            let stored = self
                .string(record.block_id)
                .expect("validated block id string");
            match compare_block_id(stored, requested) {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => return Some((mid, record)),
            }
        }
        None
    }

    fn rule_matches<P>(self, rule: RuleRecord, properties: &P) -> bool
    where
        P: JavaPropertySource + ?Sized,
    {
        (0..rule.clause_count).any(|offset| {
            let clause = self.clause(rule.clause_start + offset);
            (0..clause.predicate_count).all(|predicate_offset| {
                let predicate = self.predicate(clause.predicate_start + predicate_offset);
                let key = self.string(predicate.key).expect("validated predicate key");
                let expected = self
                    .string(predicate.values)
                    .expect("validated predicate values");
                let Some(actual) = properties.java_property(key) else {
                    return false;
                };
                expected.split('|').any(|candidate| candidate == actual)
            })
        })
    }

    fn choose_application(self, rule: RuleRecord, seed: u64) -> Option<JavaModelApplication> {
        if rule.apply_count == 0 {
            return None;
        }
        let total = (0..rule.apply_count)
            .map(|offset| u64::from(self.apply(rule.apply_start + offset).weight))
            .sum::<u64>();
        if total == 0 {
            return None;
        }
        let mut target = split_mix_64(seed) % total;
        for offset in 0..rule.apply_count {
            let apply = self.apply(rule.apply_start + offset);
            let weight = u64::from(apply.weight);
            if target < weight {
                return Some(JavaModelApplication {
                    model: JavaModelId(apply.model),
                    x_degrees: apply.x,
                    y_degrees: apply.y,
                    uv_lock: apply.flags & 1 != 0,
                });
            }
            target -= weight;
        }
        None
    }

    fn validate_records(self) -> Result<()> {
        let mut previous_block: Option<&str> = None;
        for index in 0..self.header.block_count {
            let block = self.block(index);
            self.validate_string_id(block.block_id, "block id")?;
            validate_range(
                block.variant_start,
                block.variant_count,
                self.header.variant_count,
                "block variant range",
            )?;
            validate_range(
                block.multipart_start,
                block.multipart_count,
                self.header.multipart_count,
                "block multipart range",
            )?;
            let block_id = self.string(block.block_id).expect("string id validated above");
            if let Some(previous) = previous_block {
                if previous >= block_id {
                    return invalid_database("block index is not strictly sorted");
                }
            }
            previous_block = Some(block_id);
        }

        for index in 0..self.header.variant_count {
            self.validate_rule(self.rule(self.header.variants_offset, index))?;
        }
        for index in 0..self.header.multipart_count {
            self.validate_rule(self.rule(self.header.multipart_offset, index))?;
        }
        for index in 0..self.header.clause_count {
            let clause = self.clause(index);
            validate_range(
                clause.predicate_start,
                clause.predicate_count,
                self.header.predicate_count,
                "clause predicate range",
            )?;
        }
        for index in 0..self.header.predicate_count {
            let predicate = self.predicate(index);
            self.validate_string_id(predicate.key, "predicate key")?;
            self.validate_string_id(predicate.values, "predicate values")?;
        }
        for index in 0..self.header.apply_count {
            let apply = self.apply(index);
            if apply.model >= self.header.model_count {
                return invalid_database("model application references an invalid model id");
            }
            if apply.weight == 0 {
                return invalid_database("model application has zero weight");
            }
            if i32::from(apply.x).rem_euclid(90) != 0
                || i32::from(apply.y).rem_euclid(90) != 0
            {
                return invalid_database("model application rotation is not a multiple of 90");
            }
        }

        self.validate_string_id(self.header.source_version_id, "source version")?;
        if self.header.client_sha1_id != NONE_U32 {
            self.validate_string_id(self.header.client_sha1_id, "client SHA-1")?;
        }
        for index in 0..self.header.string_count {
            let Some(value) = self.string_blob(index) else {
                return invalid_database("string index points outside string data");
            };
            if std::str::from_utf8(value).is_err() {
                return invalid_database("string pool contains invalid UTF-8");
            }
        }
        for index in 0..self.header.model_count {
            let Some(model) = self.model_blob(index) else {
                return invalid_database("model index points outside model data");
            };
            validate_model_blob(self, model)?;
        }
        Ok(())
    }

    fn validate_rule(self, rule: RuleRecord) -> Result<()> {
        if rule.clause_count == 0 {
            return invalid_database("blockstate rule has no condition clause");
        }
        if rule.apply_count == 0 {
            return invalid_database("blockstate rule has no model application");
        }
        validate_range(
            rule.clause_start,
            rule.clause_count,
            self.header.clause_count,
            "rule clause range",
        )?;
        validate_range(
            rule.apply_start,
            rule.apply_count,
            self.header.apply_count,
            "rule apply range",
        )
    }

    fn validate_string_id(self, id: u32, label: &str) -> Result<()> {
        if id >= self.header.string_count {
            return invalid_database(format!("{label} references an invalid string id"));
        }
        Ok(())
    }

    fn block(self, index: u32) -> BlockRecord {
        let base = self.header.blocks_offset + as_usize(index) * BLOCK_RECORD_SIZE;
        BlockRecord {
            block_id: read_u32(self.bytes, base).expect("validated block record"),
            variant_start: read_u32(self.bytes, base + 4).expect("validated block record"),
            variant_count: read_u32(self.bytes, base + 8).expect("validated block record"),
            multipart_start: read_u32(self.bytes, base + 12).expect("validated block record"),
            multipart_count: read_u32(self.bytes, base + 16).expect("validated block record"),
        }
    }

    fn rule(self, section: usize, index: u32) -> RuleRecord {
        let base = section + as_usize(index) * RULE_RECORD_SIZE;
        RuleRecord {
            clause_start: read_u32(self.bytes, base).expect("validated rule record"),
            clause_count: read_u32(self.bytes, base + 4).expect("validated rule record"),
            apply_start: read_u32(self.bytes, base + 8).expect("validated rule record"),
            apply_count: read_u32(self.bytes, base + 12).expect("validated rule record"),
        }
    }

    fn clause(self, index: u32) -> ClauseRecord {
        let base = self.header.clauses_offset + as_usize(index) * CLAUSE_RECORD_SIZE;
        ClauseRecord {
            predicate_start: read_u32(self.bytes, base).expect("validated clause record"),
            predicate_count: read_u32(self.bytes, base + 4).expect("validated clause record"),
        }
    }

    fn predicate(self, index: u32) -> PredicateRecord {
        let base = self.header.predicates_offset + as_usize(index) * PREDICATE_RECORD_SIZE;
        PredicateRecord {
            key: read_u32(self.bytes, base).expect("validated predicate record"),
            values: read_u32(self.bytes, base + 4).expect("validated predicate record"),
        }
    }

    fn apply(self, index: u32) -> ApplyRecord {
        let base = self.header.applies_offset + as_usize(index) * APPLY_RECORD_SIZE;
        ApplyRecord {
            model: read_u32(self.bytes, base).expect("validated apply record"),
            x: read_i16(self.bytes, base + 4).expect("validated apply record"),
            y: read_i16(self.bytes, base + 6).expect("validated apply record"),
            weight: read_u16(self.bytes, base + 8).expect("validated apply record"),
            flags: read_u16(self.bytes, base + 10).expect("validated apply record"),
        }
    }

    fn string(self, id: u32) -> Option<&'a str> {
        std::str::from_utf8(self.string_blob(id)?).ok()
    }

    fn string_blob(self, id: u32) -> Option<&'a [u8]> {
        blob(
            self.bytes,
            self.header.string_index_offset,
            self.header.string_count,
            self.header.string_data_offset,
            self.header.file_size,
            id,
        )
    }

    fn model_blob(self, id: u32) -> Option<&'a [u8]> {
        blob(
            self.bytes,
            self.header.model_index_offset,
            self.header.model_count,
            self.header.model_data_offset,
            self.header.string_index_offset,
            id,
        )
    }
}

impl Header {
    fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_SIZE || bytes.get(..8) != Some(MAGIC.as_slice()) {
            return invalid_database("invalid or truncated magic/header");
        }
        let field = |index: usize| -> Result<u32> {
            read_u32(bytes, 8 + index * 4)
                .ok_or_else(|| BlockModelError::Message("truncated Java model DB header".to_owned()))
        };
        let schema = field(0)?;
        if schema != JAVA_MODEL_DB_SCHEMA {
            return invalid_database(format!(
                "unsupported schema {schema}; expected {JAVA_MODEL_DB_SCHEMA}"
            ));
        }
        if as_usize(field(1)?) != HEADER_SIZE {
            return invalid_database("unexpected header size");
        }
        let header = Self {
            schema,
            source_version_id: field(2)?,
            client_sha1_id: field(3)?,
            block_count: field(4)?,
            variant_count: field(5)?,
            multipart_count: field(6)?,
            clause_count: field(7)?,
            predicate_count: field(8)?,
            apply_count: field(9)?,
            model_count: field(10)?,
            string_count: field(11)?,
            blocks_offset: as_usize(field(12)?),
            variants_offset: as_usize(field(13)?),
            multipart_offset: as_usize(field(14)?),
            clauses_offset: as_usize(field(15)?),
            predicates_offset: as_usize(field(16)?),
            applies_offset: as_usize(field(17)?),
            model_index_offset: as_usize(field(18)?),
            model_data_offset: as_usize(field(19)?),
            string_index_offset: as_usize(field(20)?),
            string_data_offset: as_usize(field(21)?),
            file_size: as_usize(field(22)?),
        };
        if header.file_size != bytes.len() {
            return invalid_database("header file size does not match byte slice length");
        }
        header.validate_sections()?;
        Ok(header)
    }

    fn validate_sections(self) -> Result<()> {
        let blocks_end = section_end(
            self.blocks_offset,
            self.block_count,
            BLOCK_RECORD_SIZE,
            "blocks",
        )?;
        let variants_end = section_end(
            self.variants_offset,
            self.variant_count,
            RULE_RECORD_SIZE,
            "variants",
        )?;
        let multipart_end = section_end(
            self.multipart_offset,
            self.multipart_count,
            RULE_RECORD_SIZE,
            "multipart",
        )?;
        let clauses_end = section_end(
            self.clauses_offset,
            self.clause_count,
            CLAUSE_RECORD_SIZE,
            "clauses",
        )?;
        let predicates_end = section_end(
            self.predicates_offset,
            self.predicate_count,
            PREDICATE_RECORD_SIZE,
            "predicates",
        )?;
        let applies_end = section_end(
            self.applies_offset,
            self.apply_count,
            APPLY_RECORD_SIZE,
            "applies",
        )?;
        let model_index_end = section_end(
            self.model_index_offset,
            self.model_count,
            BLOB_INDEX_RECORD_SIZE,
            "model index",
        )?;
        let string_index_end = section_end(
            self.string_index_offset,
            self.string_count,
            BLOB_INDEX_RECORD_SIZE,
            "string index",
        )?;

        let layout = [
            (self.blocks_offset, HEADER_SIZE, "blocks offset"),
            (self.variants_offset, blocks_end, "variants offset"),
            (self.multipart_offset, variants_end, "multipart offset"),
            (self.clauses_offset, multipart_end, "clauses offset"),
            (self.predicates_offset, clauses_end, "predicates offset"),
            (self.applies_offset, predicates_end, "applies offset"),
            (self.model_index_offset, applies_end, "model index offset"),
            (self.model_data_offset, model_index_end, "model data offset"),
        ];
        for (actual, expected, label) in layout {
            if actual != expected {
                return invalid_database(format!("{label} is not canonical"));
            }
        }
        if self.string_index_offset < self.model_data_offset
            || self.string_data_offset != string_index_end
            || self.string_data_offset > self.file_size
        {
            return invalid_database("model/string blob section layout is invalid");
        }
        Ok(())
    }
}

impl<'a> JavaPackedModel<'a> {
    #[must_use]
    pub fn element_count(self) -> u16 {
        read_u16(self.bytes, 0).unwrap_or(0)
    }

    #[must_use]
    pub fn elements(self) -> JavaPackedElementIter<'a> {
        JavaPackedElementIter {
            database: self.database,
            bytes: self.bytes,
            cursor: 2,
            remaining: self.element_count(),
        }
    }
}

impl<'a> JavaPackedElement<'a> {
    #[must_use]
    pub fn from_block(self) -> [f32; 3] {
        self.from.map(|value| f32::from(value) / PACKED_UNIT_PER_BLOCK)
    }

    #[must_use]
    pub fn to_block(self) -> [f32; 3] {
        self.to.map(|value| f32::from(value) / PACKED_UNIT_PER_BLOCK)
    }

    #[must_use]
    pub fn rotation_origin_block(self) -> [f32; 3] {
        self.rotation_origin
            .map(|value| f32::from(value) / PACKED_UNIT_PER_BLOCK)
    }

    #[must_use]
    pub fn rotation_angle_degrees(self) -> f32 {
        f32::from(self.rotation_angle_hundredths) / 100.0
    }

    #[must_use]
    pub fn faces(self) -> JavaPackedFaceIter<'a> {
        JavaPackedFaceIter {
            database: self.database,
            bytes: self.faces,
            cursor: 0,
            remaining: self.face_count,
        }
    }
}

impl JavaPackedFace<'_> {
    #[must_use]
    pub fn uv_normalized(self) -> Option<[f32; 4]> {
        self.uv
            .map(|uv| uv.map(|value| f32::from(value) / PACKED_UV_PER_TEXTURE))
    }

    #[must_use]
    pub fn rotation_degrees(self) -> u16 {
        u16::from(self.rotation_quarter_turns) * 90
    }
}

impl<'a> Iterator for JavaPackedElementIter<'a> {
    type Item = JavaPackedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let (element, consumed) = parse_element(self.database, self.bytes.get(self.cursor..)?)?;
        self.cursor += consumed;
        self.remaining -= 1;
        Some(element)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.remaining);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for JavaPackedElementIter<'_> {}

impl<'a> Iterator for JavaPackedFaceIter<'a> {
    type Item = JavaPackedFace<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let (face, consumed) = parse_face(self.database, self.bytes.get(self.cursor..)?)?;
        self.cursor += consumed;
        self.remaining -= 1;
        Some(face)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.remaining);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for JavaPackedFaceIter<'_> {}

/// Returns the embedded vanilla Java model database generated by CI.
///
/// Validation happens once on first use; subsequent calls are a `OnceLock` pointer load.
#[must_use]
pub fn vanilla_java_model_database() -> &'static JavaModelDatabase<'static> {
    EMBEDDED_DATABASE.get_or_init(|| {
        JavaModelDatabase::from_bytes(EMBEDDED_BYTES)
            .expect("embedded Java model database must be generated by block_model_tool bake")
    })
}

fn validate_model_blob(database: JavaModelDatabase<'_>, bytes: &[u8]) -> Result<()> {
    let Some(count) = read_u16(bytes, 0) else {
        return invalid_database("model payload is truncated before element count");
    };
    let mut cursor = 2_usize;
    for _ in 0..count {
        let Some(rest) = bytes.get(cursor..) else {
            return invalid_database("model payload element offset is invalid");
        };
        let Some((element, consumed)) = parse_element(database, rest) else {
            return invalid_database("model payload contains a malformed element");
        };
        for face in element.faces() {
            let _ = face;
        }
        cursor = cursor
            .checked_add(consumed)
            .ok_or_else(|| BlockModelError::Message("model payload offset overflow".to_owned()))?;
    }
    if cursor != bytes.len() {
        return invalid_database("model payload has trailing or unparsed bytes");
    }
    Ok(())
}

fn parse_element<'a>(
    database: JavaModelDatabase<'a>,
    bytes: &'a [u8],
) -> Option<(JavaPackedElement<'a>, usize)> {
    if bytes.len() < 24 {
        return None;
    }
    let from = [
        read_i16(bytes, 0)?,
        read_i16(bytes, 2)?,
        read_i16(bytes, 4)?,
    ];
    let to = [
        read_i16(bytes, 6)?,
        read_i16(bytes, 8)?,
        read_i16(bytes, 10)?,
    ];
    let rotation_axis = match *bytes.get(12)? {
        0 => None,
        1 => Some(JavaModelAxis::X),
        2 => Some(JavaModelAxis::Y),
        3 => Some(JavaModelAxis::Z),
        _ => return None,
    };
    let rotation_flags = *bytes.get(13)?;
    let rotation_angle_hundredths = read_i16(bytes, 14)?;
    let rotation_origin = [
        read_i16(bytes, 16)?,
        read_i16(bytes, 18)?,
        read_i16(bytes, 20)?,
    ];
    let shade = *bytes.get(22)? != 0;
    let face_count = *bytes.get(23)?;
    let mut cursor = 24_usize;
    for _ in 0..face_count {
        let (_, consumed) = parse_face(database, bytes.get(cursor..)?)?;
        cursor = cursor.checked_add(consumed)?;
    }
    Some((
        JavaPackedElement {
            database,
            from,
            to,
            rotation_axis,
            rotation_origin,
            rotation_angle_hundredths,
            rescale: rotation_flags & 1 != 0,
            shade,
            face_count,
            faces: bytes.get(24..cursor)?,
        },
        cursor,
    ))
}

fn parse_face<'a>(
    database: JavaModelDatabase<'a>,
    bytes: &'a [u8],
) -> Option<(JavaPackedFace<'a>, usize)> {
    if bytes.len() < 10 {
        return None;
    }
    let face = face_from_code(*bytes.first()?)?;
    let material_slot_id = read_u32(bytes, 1)?;
    let material_slot = database.string(material_slot_id)?;
    let flags = *bytes.get(5)?;
    let rotation_quarter_turns = *bytes.get(6)?;
    if rotation_quarter_turns > 3 {
        return None;
    }
    let cull_face = match *bytes.get(7)? {
        u8::MAX => None,
        code => Some(face_from_code(code)?),
    };
    let raw_tint = read_i16(bytes, 8)?;
    let tint_index = (raw_tint >= 0).then_some(raw_tint);
    let uv = if flags & 1 != 0 {
        Some([
            read_i16(bytes, 10)?,
            read_i16(bytes, 12)?,
            read_i16(bytes, 14)?,
            read_i16(bytes, 16)?,
        ])
    } else {
        None
    };
    let consumed = if uv.is_some() { 18 } else { 10 };
    Some((
        JavaPackedFace {
            face,
            material_slot,
            uv,
            rotation_quarter_turns,
            cull_face,
            tint_index,
        },
        consumed,
    ))
}

fn compare_block_id(stored: &str, requested: &str) -> Ordering {
    if requested.contains(':') {
        return stored.cmp(requested);
    }
    stored
        .strip_prefix("minecraft:")
        .unwrap_or(stored)
        .cmp(requested)
}

fn face_from_code(code: u8) -> Option<BlockFace> {
    match code {
        0 => Some(BlockFace::Down),
        1 => Some(BlockFace::Up),
        2 => Some(BlockFace::North),
        3 => Some(BlockFace::South),
        4 => Some(BlockFace::West),
        5 => Some(BlockFace::East),
        _ => None,
    }
}

fn blob<'a>(
    bytes: &'a [u8],
    index_offset: usize,
    count: u32,
    data_offset: usize,
    data_end: usize,
    id: u32,
) -> Option<&'a [u8]> {
    if id >= count {
        return None;
    }
    let record = index_offset.checked_add(as_usize(id).checked_mul(BLOB_INDEX_RECORD_SIZE)?)?;
    let offset = as_usize(read_u32(bytes, record)?);
    let len = as_usize(read_u32(bytes, record + 4)?);
    let start = data_offset.checked_add(offset)?;
    let end = start.checked_add(len)?;
    if start < data_offset || end > data_end {
        return None;
    }
    bytes.get(start..end)
}

fn validate_range(start: u32, count: u32, total: u32, label: &str) -> Result<()> {
    let Some(end) = start.checked_add(count) else {
        return invalid_database(format!("{label} overflows"));
    };
    if end > total {
        return invalid_database(format!("{label} points outside its section"));
    }
    Ok(())
}

fn section_end(offset: usize, count: u32, record_size: usize, label: &str) -> Result<usize> {
    let bytes = as_usize(count)
        .checked_mul(record_size)
        .ok_or_else(|| BlockModelError::Message(format!("Java model DB {label} size overflow")))?;
    offset
        .checked_add(bytes)
        .ok_or_else(|| BlockModelError::Message(format!("Java model DB {label} offset overflow")))
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_i16(bytes: &[u8], offset: usize) -> Option<i16> {
    Some(i16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

#[expect(clippy::cast_possible_truncation, reason = "u32 always fits usize on supported 32/64-bit targets")]
const fn as_usize(value: u32) -> usize {
    value as usize
}

fn split_mix_64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn invalid_database<T>(message: impl Into<String>) -> Result<T> {
    Err(BlockModelError::Message(format!(
        "invalid packed Java model database: {}",
        message.into()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_database_validates_and_finds_vanilla_blocks() {
        let database = vanilla_java_model_database();
        assert_eq!(database.schema(), JAVA_MODEL_DB_SCHEMA);
        assert!(!database.source_version().is_empty());
        assert!(database.block_count() > 1_000);
        assert!(database.model_count() > 100);
        assert!(database.contains_block("minecraft:stone"));
        assert!(database.contains_block("stone"));
        assert!(database.contains_block("minecraft:oak_trapdoor"));
        assert!(!database.contains_block("minecraft:bmcb_missing_test_block"));
    }

    #[test]
    fn stone_state_resolves_without_allocating_properties() {
        let database = vanilla_java_model_database();
        let properties: [(&str, &str); 0] = [];
        let mut applications = Vec::new();
        assert!(database.for_each_model_application(
            "minecraft:stone",
            properties.as_slice(),
            0,
            |application| applications.push(application),
        ));
        assert_eq!(applications.len(), 1);
        let model = database
            .model(applications[0].model)
            .expect("stone model should exist");
        assert!(model.element_count() > 0);
        let elements = model.elements().collect::<Vec<_>>();
        assert_eq!(elements.len(), usize::from(model.element_count()));
        assert!(elements.iter().any(|element| element.faces().count() > 0));
    }

    #[test]
    fn trapdoor_state_resolves_a_rotated_or_direct_model() {
        let database = vanilla_java_model_database();
        let properties = [
            ("facing", "north"),
            ("half", "bottom"),
            ("open", "false"),
            ("powered", "false"),
            ("waterlogged", "false"),
        ];
        let mut count = 0;
        assert!(database.for_each_model_application(
            "minecraft:oak_trapdoor",
            properties.as_slice(),
            42,
            |application| {
                count += 1;
                assert!(database.model(application.model).is_some());
            },
        ));
        assert_eq!(count, 1);
    }

    #[test]
    fn truncated_database_is_rejected() {
        let truncated = &EMBEDDED_BYTES[..EMBEDDED_BYTES.len() / 2];
        assert!(JavaModelDatabase::from_bytes(truncated).is_err());
    }
}
