//! Global records and chunk tick documents edited through bedrock-world.

use super::*;
use bedrock_world::chunk::ticks::ChunkTicks;
use serde::Deserialize;

#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TickDocument {
    #[serde(deserialize_with = "required_queue")]
    pending: Option<Vec<NbtTag>>,
    #[serde(deserialize_with = "required_queue")]
    random: Option<Vec<NbtTag>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeDocument {
    height_map: Vec<i16>,
    storages: Vec<BiomeLayer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeLayer {
    palette: Vec<u32>,
    indices: Option<Vec<u16>>,
}

pub(super) fn biome_json(data: &bedrock_world::biome::BiomeData) -> serde_json::Value {
    serde_json::json!({
        "height_map": data.height_map,
        "storages": data.storages.iter().map(|storage| serde_json::json!({
            "palette": storage.palette, "indices": storage.indices,
        })).collect::<Vec<_>>(),
    })
}

fn required_queue<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<NbtTag>>, D::Error> {
    Option::deserialize(deserializer)
}

pub(super) fn global_detail(
    editor: &WorldEditor,
    kind: GlobalRecordKind,
) -> bedrock_map_render::Result<ProfessionalDetail> {
    let record = editor.global(kind.clone())?;
    let source = record.as_ref().and_then(|record| record.source().cloned());
    let roots = record.as_ref().map_or(&[][..], global::Record::roots);
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::GlobalRecord {
            kind: kind.clone(),
            source,
        },
        title: t!(
            "MapViewer.core.records.global_record_title",
            kind = global_kind_label(&kind)
        ),
        sections: vec![EditSection {
            title: t!("MapViewer.core.records.global_data"),
            rows: vec![readonly_row(
                t!("MapViewer.core.records.record_label"),
                if record.is_some() {
                    t!("MapViewer.core.records.present")
                } else {
                    t!("MapViewer.core.records.missing")
                },
            )],
        }],
        json: pretty_json(serde_json::json!(roots)),
    })
}

pub(super) fn ticks_detail(
    editor: &WorldEditor,
    chunk: ChunkPos,
) -> bedrock_map_render::Result<ProfessionalDetail> {
    let ticks = editor.world().chunk_ticks(chunk)?;
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::TickQueues {
            chunk,
            pending: ticks.pending_source().cloned(),
            random: ticks.random_source().cloned(),
        },
        title: t!(
            "MapViewer.core.records.tick_queues_title",
            chunk_x = chunk.x,
            chunk_z = chunk.z
        ),
        sections: vec![EditSection {
            title: t!("MapViewer.core.records.chunk_tick_queue"),
            rows: vec![
                readonly_row(
                    t!("MapViewer.core.records.pending_tick_record"),
                    if ticks.has_pending() {
                        t!("MapViewer.core.records.present")
                    } else {
                        t!("MapViewer.core.records.missing")
                    },
                ),
                readonly_row(
                    t!("MapViewer.core.records.random_tick_record"),
                    if ticks.has_random() {
                        t!("MapViewer.core.records.present")
                    } else {
                        t!("MapViewer.core.records.missing")
                    },
                ),
            ],
        }],
        json: pretty_json(serde_json::json!(TickDocument {
            pending: ticks.pending().map(<[NbtTag]>::to_vec),
            random: ticks.random().map(<[NbtTag]>::to_vec),
        })),
    })
}

pub(super) fn write_global(
    editor: &WorldEditor,
    kind: GlobalRecordKind,
    source: Option<Bytes>,
    action: EditAction,
    text: Option<String>,
) -> bedrock_world::Result<EditInvalidation> {
    let mut record = match source {
        Some(source) => global::parse(kind, source)?,
        None => global::Record::new(kind, Vec::new()),
    };
    let mut transaction = editor.world().transaction();
    match action {
        EditAction::Save => {
            *record.roots_mut() = parse_document(text)?;
            transaction.save_global(&record)?;
        }
        EditAction::Delete => transaction.remove_global(&record),
    }
    transaction.commit()?;
    Ok(EditInvalidation::metadata())
}

pub(super) fn write_ticks(
    editor: &WorldEditor,
    chunk: ChunkPos,
    source: (Option<Bytes>, Option<Bytes>),
    action: EditAction,
    text: Option<String>,
) -> bedrock_world::Result<EditInvalidation> {
    let mut ticks = ChunkTicks::from_records(source.0, source.1)?;
    let document = match action {
        EditAction::Save => parse_document(text)?,
        EditAction::Delete => TickDocument {
            pending: None,
            random: None,
        },
    };
    match document.pending {
        Some(roots) => ticks.set_pending(roots),
        None => ticks.remove_pending(),
    }
    match document.random {
        Some(roots) => ticks.set_random(roots),
        None => ticks.remove_random(),
    }
    editor.world().write_chunk_ticks(chunk, &ticks)?;
    Ok(EditInvalidation::chunk(chunk).with_metadata())
}

fn parse_document<T: serde::de::DeserializeOwned>(
    text: Option<String>,
) -> bedrock_world::Result<T> {
    let text = text.ok_or_else(|| {
        bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.missing_edit_content").to_string(),
        )
    })?;
    serde_json::from_str(&text).map_err(|error| {
        bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.invalid_edit_content", error = error).to_string(),
        )
    })
}

pub(super) fn write_spawn_areas(
    editor: &WorldEditor,
    chunk: ChunkPos,
    source: Option<Bytes>,
    action: EditAction,
    text: Option<String>,
) -> bedrock_world::Result<EditInvalidation> {
    let mut transaction = editor.world().transaction();
    match action {
        EditAction::Save => {
            let areas: Vec<HardcodedSpawnArea> = chunk_field(text, chunk, "areas")?;
            transaction.update_hardcoded_spawn_areas(chunk, source, &areas)?;
        }
        EditAction::Delete => transaction.remove_hardcoded_spawn_areas(chunk, source),
    }
    transaction.commit()?;
    Ok(EditInvalidation::chunk(chunk).with_metadata())
}

pub(super) fn write_block_entities(
    editor: &WorldEditor,
    chunk: ChunkPos,
    source: Option<Bytes>,
    text: Option<String>,
) -> bedrock_world::Result<EditInvalidation> {
    let roots: Vec<NbtTag> = chunk_field(text, chunk, "records")?;
    let entities = roots
        .into_iter()
        .map(BlockEntity::from_nbt)
        .collect::<bedrock_world::Result<Vec<_>>>()?;
    let mut transaction = editor.world().transaction();
    transaction.update_block_entities(chunk, source, &entities)?;
    transaction.commit()?;
    Ok(EditInvalidation::chunk(chunk).with_metadata())
}

pub(super) fn write_heightmap(
    editor: &WorldEditor,
    chunk: ChunkPos,
    version: ChunkVersion,
    text: Option<String>,
) -> bedrock_world::Result<EditInvalidation> {
    let heights: Vec<i16> = chunk_field(text, chunk, "heightmap")?;
    let current = editor.world().biome_data(chunk)?.ok_or_else(|| {
        bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.chunk_has_no_editable_heightmap").to_string(),
        )
    })?;
    if current.version != version {
        return Err(bedrock_world::BedrockWorldError::ConcurrentWrite(
            t!("MapViewer.core.records.heightmap_format_changed").to_string(),
        ));
    }
    editor.put_heightmap(chunk, version, HeightMap2d::new(heights)?)
}

pub(super) fn write_biomes(
    editor: &WorldEditor,
    chunk: ChunkPos,
    text: Option<String>,
) -> bedrock_world::Result<EditInvalidation> {
    let data: BiomeDocument = chunk_field(text, chunk, "biome")?;
    let current = editor.world().biome_data(chunk)?;
    if !current.is_some_and(|data| data.version == ChunkVersion::New) {
        return Err(bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.legacy_biome_format_needs_editor").to_string(),
        ));
    }
    let storages = data
        .storages
        .into_iter()
        .map(|layer| bedrock_world::biome::BiomeStorage {
            y: None,
            palette: layer.palette,
            indices: layer.indices,
            counts: Vec::new(),
        })
        .collect();
    editor.put_biome_storage(chunk, Biome3d::new(data.height_map, storages)?)
}

fn chunk_field<T: serde::de::DeserializeOwned>(
    text: Option<String>,
    chunk: ChunkPos,
    field: &str,
) -> bedrock_world::Result<T> {
    let document: serde_json::Value = parse_document(text)?;
    let serde_json::Value::Object(mut fields) = document else {
        return Err(bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.edit_content_must_be_object").to_string(),
        ));
    };
    if fields.remove("chunk") != Some(chunk_json(chunk)) || fields.len() != 1 {
        return Err(bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.chunk_coordinates_or_fields_mismatch").to_string(),
        ));
    }
    let value = fields.remove(field).ok_or_else(|| {
        bedrock_world::BedrockWorldError::Validation(
            t!("MapViewer.core.records.missing_field", field = field).to_string(),
        )
    })?;
    serde_json::from_value(value).map_err(|error| {
        bedrock_world::BedrockWorldError::Validation(
            t!(
                "MapViewer.core.records.invalid_field_content",
                field = field,
                error = error
            )
            .to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn tick_document_requires_both_queues_and_rejects_typos() {
        assert!(serde_json::from_str::<TickDocument>(r#"{"pending":null}"#).is_err());
        assert!(
            serde_json::from_str::<TickDocument>(r#"{"pending":null,"random":null,"randmo":[]}"#)
                .is_err()
        );
        let document: TickDocument =
            serde_json::from_str(r#"{"pending":[],"random":null}"#).unwrap();
        assert_eq!(document.pending, Some(Vec::new()));
        assert_eq!(document.random, None);
    }

    #[::core::prelude::v1::test]
    fn chunk_document_rejects_retargeting_and_extra_fields() {
        let chunk = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let valid = serde_json::json!({ "chunk": chunk_json(chunk), "heightmap": vec![42; 256] });
        assert_eq!(
            chunk_field::<Vec<i16>>(Some(valid.to_string()), chunk, "heightmap").unwrap(),
            vec![42; 256]
        );
        let invalid =
            serde_json::json!({ "chunk": chunk_json(chunk), "heightmap": [], "extra": true });
        assert!(chunk_field::<Vec<i16>>(Some(invalid.to_string()), chunk, "heightmap").is_err());
        let other = ChunkPos { x: 2, ..chunk };
        assert!(chunk_field::<Vec<i16>>(Some(valid.to_string()), other, "heightmap").is_err());
    }
}
