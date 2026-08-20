//! Authoritative, data-driven Bedrock BlockState migration.
//!
//! The executor follows the ordering semantics used by PocketMine-MP's
//! `BedrockBlockUpgradeSchema`: schemas are grouped by Mojang storage-version id and sorted by
//! numeric schema id, same-version multi-schema groups are not skipped, and `remappedStates`
//! short-circuits the remaining transformations in the matching schema.

use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Packed Mojang BlockState storage version (`major << 24 | minor << 16 | patch << 8 | revision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockStateStorageVersion(i32);

impl BlockStateStorageVersion {
    /// Builds a packed storage version from its four Bedrock components.
    #[must_use]
    pub const fn from_components(major: u8, minor: u8, patch: u8, revision: u8) -> Self {
        Self(
            ((major as i32) << 24)
                | ((minor as i32) << 16)
                | ((patch as i32) << 8)
                | revision as i32,
        )
    }

    /// Wraps a raw version stored in BlockState NBT.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// Returns the packed value persisted in BlockState NBT.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }
}

/// One JSON source document used to build an authoritative catalog.
#[derive(Debug, Clone, Copy)]
pub struct BlockStateSchemaSource<'a> {
    /// Stable schema filename. Its numeric prefix defines ordering inside equal-version groups.
    pub name: &'a str,
    /// UTF-8 JSON schema document.
    pub json: &'a str,
}

/// Parsed, validated authoritative schema catalog.
#[derive(Debug, Clone)]
pub struct AuthoritativeBlockStateCatalog {
    groups: Vec<SchemaGroup>,
    output_version: BlockStateStorageVersion,
    schema_count: usize,
}

#[derive(Debug, Clone)]
struct SchemaGroup {
    result_version: BlockStateStorageVersion,
    schemas: Vec<Schema>,
}

#[derive(Debug, Clone, Default)]
struct Schema {
    id: u32,
    renamed_ids: BTreeMap<String, String>,
    added_properties: BTreeMap<String, BTreeMap<String, NbtTag>>,
    removed_properties: BTreeMap<String, Vec<String>>,
    renamed_properties: BTreeMap<String, BTreeMap<String, String>>,
    remapped_property_values: BTreeMap<String, BTreeMap<String, String>>,
    value_remap_index: BTreeMap<String, Vec<ValueRemap>>,
    flattened_properties: BTreeMap<String, FlattenRule>,
    remapped_states: BTreeMap<String, Vec<StateRemap>>,
}

#[derive(Debug, Clone)]
struct ValueRemap {
    old: NbtTag,
    new: NbtTag,
}

#[derive(Debug, Clone)]
struct FlattenRule {
    prefix: String,
    property: String,
    property_type: FlattenPropertyType,
    suffix: String,
    value_remaps: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlattenPropertyType {
    Byte,
    Int,
    String,
}

#[derive(Debug, Clone)]
struct StateRemap {
    old_state: BTreeMap<String, NbtTag>,
    new_name: StateRemapName,
    new_state: BTreeMap<String, NbtTag>,
    copied_state: Vec<String>,
}

#[derive(Debug, Clone)]
enum StateRemapName {
    Literal(String),
    Flattened(FlattenRule),
}

impl AuthoritativeBlockStateCatalog {
    /// Parses, validates, groups and orders an authoritative schema corpus.
    pub fn from_sources(sources: &[BlockStateSchemaSource<'_>]) -> Result<Self> {
        if sources.is_empty() {
            return Err(validation(
                "authoritative BlockState schema corpus is empty",
            ));
        }

        let mut groups = BTreeMap::<BlockStateStorageVersion, Vec<Schema>>::new();
        for source in sources {
            let schema_id = parse_schema_id(source.name)?;
            let value: Value = serde_json::from_str(source.json).map_err(|error| {
                validation(format!(
                    "invalid BlockState schema {}: {error}",
                    source.name
                ))
            })?;
            let root = object(&value, "schema root")?;
            let result_version = BlockStateStorageVersion::from_components(
                u8_field(root, "maxVersionMajor")?,
                u8_field(root, "maxVersionMinor")?,
                u8_field(root, "maxVersionPatch")?,
                u8_field(root, "maxVersionRevision")?,
            );
            let schema = Schema::parse(schema_id, root)?;
            let siblings = groups.entry(result_version).or_default();
            if siblings.iter().any(|existing| existing.id == schema_id) {
                return Err(validation(format!(
                    "duplicate BlockState schema id {schema_id} for storage version {}",
                    result_version.raw()
                )));
            }
            siblings.push(schema);
        }

        let mut ordered = Vec::with_capacity(groups.len());
        let mut schema_count = 0usize;
        for (result_version, mut schemas) in groups {
            schemas.sort_by_key(|schema| schema.id);
            schema_count = schema_count.saturating_add(schemas.len());
            ordered.push(SchemaGroup {
                result_version,
                schemas,
            });
        }
        let output_version = ordered
            .last()
            .map(|group| group.result_version)
            .ok_or_else(|| {
                validation("authoritative BlockState schema corpus produced no groups")
            })?;
        Ok(Self {
            groups: ordered,
            output_version,
            schema_count,
        })
    }

    /// Returns the newest BlockState storage version represented by this corpus.
    #[must_use]
    pub const fn output_version(&self) -> BlockStateStorageVersion {
        self.output_version
    }

    /// Returns the number of schema documents in the catalog.
    #[must_use]
    pub const fn schema_count(&self) -> usize {
        self.schema_count
    }

    /// Returns the number of distinct result-version groups.
    #[must_use]
    pub fn version_group_count(&self) -> usize {
        self.groups.len()
    }

    /// Upgrades one historical BlockState through the complete catalog.
    ///
    /// A state newer than the corpus is rejected so callers can preserve its original raw bytes.
    pub fn upgrade(&self, state: &BlockState) -> Result<BlockState> {
        let source_version =
            BlockStateStorageVersion::from_raw(state.version.ok_or_else(|| {
                validation(format!("BlockState {} has no storage version", state.name))
            })?);
        if source_version > self.output_version {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "BlockState {} uses future storage version {}; authoritative corpus ends at {}",
                state.name,
                source_version.raw(),
                self.output_version.raw()
            )));
        }

        let mut name = state.name.clone();
        let mut states = state.states.clone();
        for group in &self.groups {
            // Compare every group with the original source version. Mojang has shipped incompatible
            // schema changes without bumping this version id, so a same-version group is skipped only
            // when that group contains exactly one schema.
            if source_version > group.result_version
                || (source_version == group.result_version && group.schemas.len() == 1)
            {
                continue;
            }
            for schema in &group.schemas {
                (name, states) = schema.apply(&name, states)?;
            }
        }

        Ok(BlockState {
            name,
            states,
            version: Some(self.output_version.raw()),
        })
    }
}

impl Schema {
    fn parse(id: u32, root: &Map<String, Value>) -> Result<Self> {
        let value_remap_index = parse_value_remap_index(root.get("remappedPropertyValuesIndex"))?;
        let remapped_property_values = parse_nested_string_map(root.get("remappedPropertyValues"))?;
        for (block, properties) in &remapped_property_values {
            for (property, index) in properties {
                if !value_remap_index.contains_key(index) {
                    return Err(validation(format!(
                        "schema {id} references missing value-remap index {index} for {block}.{property}"
                    )));
                }
            }
        }

        Ok(Self {
            id,
            renamed_ids: parse_string_map(root.get("renamedIds"))?,
            added_properties: parse_nested_state_map(root.get("addedProperties"))?,
            removed_properties: parse_string_list_map(root.get("removedProperties"))?,
            renamed_properties: parse_nested_string_map(root.get("renamedProperties"))?,
            remapped_property_values,
            value_remap_index,
            flattened_properties: parse_flatten_map(root.get("flattenedProperties"))?,
            remapped_states: parse_state_remap_map(root.get("remappedStates"))?,
        })
    }

    fn apply(
        &self,
        old_name: &str,
        mut states: BTreeMap<String, NbtTag>,
    ) -> Result<(String, BTreeMap<String, NbtTag>)> {
        if let Some(remapped) = self.apply_state_remap(old_name, &states)? {
            return Ok(remapped);
        }

        if self.renamed_ids.contains_key(old_name)
            && self.flattened_properties.contains_key(old_name)
        {
            return Err(validation(format!(
                "schema {} defines both renamedIds and flattenedProperties for {old_name}",
                self.id
            )));
        }

        let new_name = if let Some(name) = self.renamed_ids.get(old_name) {
            name.clone()
        } else if let Some(flatten) = self.flattened_properties.get(old_name) {
            let (name, flattened_states) = flatten.apply(old_name, &states);
            states = flattened_states;
            name
        } else {
            old_name.to_string()
        };

        if let Some(added) = self.added_properties.get(old_name) {
            for (property, value) in added {
                states
                    .entry(property.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        if let Some(removed) = self.removed_properties.get(old_name) {
            for property in removed {
                states.remove(property);
            }
        }
        if let Some(renames) = self.renamed_properties.get(old_name) {
            for (old_property, new_property) in renames {
                if let Some(old_value) = states.remove(old_property) {
                    let mapped = self.remap_property_value(old_name, old_property, &old_value)?;
                    states.insert(new_property.clone(), mapped);
                }
            }
        }
        if let Some(remaps) = self.remapped_property_values.get(old_name) {
            for old_property in remaps.keys() {
                if let Some(old_value) = states.get(old_property).cloned() {
                    let mapped = self.remap_property_value(old_name, old_property, &old_value)?;
                    states.insert(old_property.clone(), mapped);
                }
            }
        }
        Ok((new_name, states))
    }

    fn apply_state_remap(
        &self,
        old_name: &str,
        old_state: &BTreeMap<String, NbtTag>,
    ) -> Result<Option<(String, BTreeMap<String, NbtTag>)>> {
        let Some(remaps) = self.remapped_states.get(old_name) else {
            return Ok(None);
        };
        for remap in remaps {
            if remap.old_state.len() > old_state.len()
                || !remap
                    .old_state
                    .iter()
                    .all(|(name, value)| old_state.get(name) == Some(value))
            {
                continue;
            }
            let new_name = match &remap.new_name {
                StateRemapName::Literal(name) => name.clone(),
                StateRemapName::Flattened(flatten) => flatten.apply(old_name, old_state).0,
            };
            let mut new_state = remap.new_state.clone();
            for copied in &remap.copied_state {
                if let Some(value) = old_state.get(copied) {
                    new_state.insert(copied.clone(), value.clone());
                }
            }
            return Ok(Some((new_name, new_state)));
        }
        Ok(None)
    }

    fn remap_property_value(
        &self,
        old_name: &str,
        old_property: &str,
        old_value: &NbtTag,
    ) -> Result<NbtTag> {
        let Some(index_name) = self
            .remapped_property_values
            .get(old_name)
            .and_then(|properties| properties.get(old_property))
        else {
            return Ok(old_value.clone());
        };
        let pairs = self.value_remap_index.get(index_name).ok_or_else(|| {
            validation(format!(
                "schema {} lost value-remap index {index_name}",
                self.id
            ))
        })?;
        Ok(pairs
            .iter()
            .find(|pair| &pair.old == old_value)
            .map_or_else(|| old_value.clone(), |pair| pair.new.clone()))
    }
}

impl FlattenRule {
    fn parse(value: &Value) -> Result<Self> {
        let root = object(value, "flatten rule")?;
        let property_type = match root.get("flattenedPropertyType").and_then(Value::as_str) {
            Some("byte") => FlattenPropertyType::Byte,
            Some("int") => FlattenPropertyType::Int,
            Some("string") | None => FlattenPropertyType::String,
            Some(other) => {
                return Err(validation(format!("unknown flattenedPropertyType {other}")));
            }
        };
        Ok(Self {
            prefix: string_field(root, "prefix")?.to_string(),
            property: string_field(root, "flattenedProperty")?.to_string(),
            property_type,
            suffix: string_field(root, "suffix")?.to_string(),
            value_remaps: parse_string_map(root.get("flattenedValueRemaps"))?,
        })
    }

    fn apply(
        &self,
        old_name: &str,
        old_state: &BTreeMap<String, NbtTag>,
    ) -> (String, BTreeMap<String, NbtTag>) {
        let Some(value) = old_state.get(&self.property) else {
            return (old_name.to_string(), old_state.clone());
        };
        let raw = match (self.property_type, value) {
            (FlattenPropertyType::Byte, NbtTag::Byte(value)) => value.to_string(),
            (FlattenPropertyType::Int, NbtTag::Int(value)) => value.to_string(),
            (FlattenPropertyType::String, NbtTag::String(value)) => value.clone(),
            _ => return (old_name.to_string(), old_state.clone()),
        };
        let embedded = self
            .value_remaps
            .get(&raw)
            .map_or(raw.as_str(), String::as_str);
        let mut states = old_state.clone();
        states.remove(&self.property);
        (
            format!("{}{}{}", self.prefix, embedded, self.suffix),
            states,
        )
    }
}

fn parse_state_remap(value: &Value) -> Result<StateRemap> {
    let root = object(value, "remapped state")?;
    let new_name = match (root.get("newName"), root.get("newFlattenedName")) {
        (Some(Value::String(name)), None) => StateRemapName::Literal(name.clone()),
        (None, Some(flatten)) => StateRemapName::Flattened(FlattenRule::parse(flatten)?),
        (Some(_), Some(_)) => {
            return Err(validation(
                "remappedState contains both newName and newFlattenedName",
            ));
        }
        _ => {
            return Err(validation(
                "remappedState contains neither valid newName nor newFlattenedName",
            ));
        }
    };
    Ok(StateRemap {
        old_state: parse_state_map(root.get("oldState"))?,
        new_name,
        new_state: parse_state_map(root.get("newState"))?,
        copied_state: parse_string_array(root.get("copiedState"))?,
    })
}

fn parse_state_remap_map(value: Option<&Value>) -> Result<BTreeMap<String, Vec<StateRemap>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "remappedStates")?;
    let mut output = BTreeMap::new();
    for (name, remaps) in root {
        let array = remaps
            .as_array()
            .ok_or_else(|| validation(format!("remappedStates.{name} must be an array")))?;
        let mut parsed = Vec::with_capacity(array.len());
        for remap in array {
            parsed.push(parse_state_remap(remap)?);
        }
        output.insert(name.clone(), parsed);
    }
    Ok(output)
}

fn parse_flatten_map(value: Option<&Value>) -> Result<BTreeMap<String, FlattenRule>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "flattenedProperties")?;
    root.iter()
        .map(|(name, rule)| FlattenRule::parse(rule).map(|rule| (name.clone(), rule)))
        .collect()
}

fn parse_value_remap_index(value: Option<&Value>) -> Result<BTreeMap<String, Vec<ValueRemap>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "remappedPropertyValuesIndex")?;
    let mut output = BTreeMap::new();
    for (name, pairs) in root {
        let array = pairs.as_array().ok_or_else(|| {
            validation(format!(
                "remappedPropertyValuesIndex.{name} must be an array"
            ))
        })?;
        let mut parsed = Vec::with_capacity(array.len());
        for pair in array {
            let pair = object(pair, "property value remap")?;
            parsed.push(ValueRemap {
                old: parse_state_value(required(pair, "old")?)?,
                new: parse_state_value(required(pair, "new")?)?,
            });
        }
        output.insert(name.clone(), parsed);
    }
    Ok(output)
}

fn parse_nested_state_map(
    value: Option<&Value>,
) -> Result<BTreeMap<String, BTreeMap<String, NbtTag>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "nested state map")?;
    root.iter()
        .map(|(name, states)| parse_state_map(Some(states)).map(|states| (name.clone(), states)))
        .collect()
}

fn parse_state_map(value: Option<&Value>) -> Result<BTreeMap<String, NbtTag>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let root = object(value, "state map")?;
    root.iter()
        .map(|(name, value)| parse_state_value(value).map(|value| (name.clone(), value)))
        .collect()
}

fn parse_state_value(value: &Value) -> Result<NbtTag> {
    let root = object(value, "state value")?;
    if root.len() != 1 {
        return Err(validation(
            "schema state value must contain exactly one of byte/int/string",
        ));
    }
    if let Some(value) = root.get("byte") {
        let value = value
            .as_i64()
            .ok_or_else(|| validation("schema byte value must be an integer"))?;
        return i8::try_from(value)
            .map(NbtTag::Byte)
            .map_err(|_| validation(format!("schema byte value {value} does not fit i8")));
    }
    if let Some(value) = root.get("int") {
        let value = value
            .as_i64()
            .ok_or_else(|| validation("schema int value must be an integer"))?;
        return i32::try_from(value)
            .map(NbtTag::Int)
            .map_err(|_| validation(format!("schema int value {value} does not fit i32")));
    }
    if let Some(Value::String(value)) = root.get("string") {
        return Ok(NbtTag::String(value.clone()));
    }
    Err(validation("unknown schema state value type"))
}

fn parse_nested_string_map(
    value: Option<&Value>,
) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "nested string map")?;
    root.iter()
        .map(|(name, values)| parse_string_map(Some(values)).map(|values| (name.clone(), values)))
        .collect()
}

fn parse_string_map(value: Option<&Value>) -> Result<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "string map")?;
    root.iter()
        .map(|(name, value)| {
            value
                .as_str()
                .map(|value| (name.clone(), value.to_string()))
                .ok_or_else(|| validation(format!("{name} must contain a string value")))
        })
        .collect()
}

fn parse_string_list_map(value: Option<&Value>) -> Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let root = object(value, "string list map")?;
    root.iter()
        .map(|(name, value)| parse_string_array(Some(value)).map(|values| (name.clone(), values)))
        .collect()
}

fn parse_string_array(value: Option<&Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| validation("expected a string array"))?;
    array
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| validation("array entry must be a string"))
        })
        .collect()
}

fn u8_field(root: &Map<String, Value>, name: &str) -> Result<u8> {
    let value = required(root, name)?
        .as_u64()
        .ok_or_else(|| validation(format!("{name} must be an unsigned integer")))?;
    u8::try_from(value).map_err(|_| validation(format!("{name} exceeds u8")))
}

fn string_field<'a>(root: &'a Map<String, Value>, name: &str) -> Result<&'a str> {
    required(root, name)?
        .as_str()
        .ok_or_else(|| validation(format!("{name} must be a string")))
}

fn required<'a>(root: &'a Map<String, Value>, name: &str) -> Result<&'a Value> {
    root.get(name)
        .ok_or_else(|| validation(format!("missing required schema field {name}")))
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| validation(format!("{context} must be an object")))
}

fn parse_schema_id(name: &str) -> Result<u32> {
    let prefix = name.split_once('_').map_or(name, |(prefix, _)| prefix);
    prefix.parse::<u32>().map_err(|_| {
        validation(format!(
            "BlockState schema filename has no numeric prefix: {name}"
        ))
    })
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}
