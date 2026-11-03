//! Ignored local-world audit using the editor's actual model and face pipeline.
use super::*;
use bedrock_world::surface::WorldScanOptions;
use bedrock_world::{BlockPalette, ChunkRecordTag, SubChunk};

pub(crate) fn run(world_path: PathBuf, package_roots: Vec<PathBuf>) {
    let world = World::open(
        &world_path,
        OpenOptions {
            read_only: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut positions = world.chunk_positions(WorldScanOptions::default()).unwrap();
    let env_int = |name| std::env::var(name).ok().and_then(|v| v.parse::<i32>().ok());
    if let Some(radius) = env_int("BMCBL_WORLD3D_TEST_AUDIT_RADIUS") {
        let x = env_int("BMCBL_WORLD3D_TEST_X")
            .expect("audit region X")
            .div_euclid(16);
        let z = env_int("BMCBL_WORLD3D_TEST_Z")
            .expect("audit region Z")
            .div_euclid(16);
        positions.retain(|pos| inside_view(*pos, Dimension::Overworld, x, z, radius));
    }
    let mut states = BTreeMap::<String, BlockState>::new();
    let mut frame_faces = Vec::new();
    let mut nonair = 0u64;
    for (index, pos) in positions.iter().enumerate() {
        let chunk = world.chunk(*pos).unwrap();
        if chunk
            .records
            .iter()
            .any(|record| record.key.tag == ChunkRecordTag::BlockEntity)
        {
            let data = world
                .query_chunk_data(
                    *pos,
                    ChunkLoadOptions {
                        data_request: ChunkDataRequest::new().full_3d_indices().block_entities(),
                        ..Default::default()
                    },
                )
                .unwrap();
            frame_faces.extend(FrameDisplay::collect(&data).iter().map(|frame| {
                (
                    format!("frame at {:?} {:?}", pos, frame.position()),
                    frame.face(),
                )
            }));
        }
        for record in chunk
            .records
            .iter()
            .filter(|r| r.key.tag == ChunkRecordTag::SubChunkPrefix)
        {
            let y = record.key.subchunk_y.unwrap();
            let Some(subchunk) = world
                .subchunk(*pos, y, SubChunkDecodeMode::CountsOnly)
                .unwrap()
            else {
                continue;
            };
            let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
                panic!("material audit requires palette subchunks at {pos:?}/{y}");
            };
            for palette in storages {
                for (state, count) in palette.states.into_iter().zip(palette.counts.unwrap()) {
                    if count == 0
                        || matches!(
                            state.name.strip_prefix("minecraft:").unwrap_or(&state.name),
                            "air" | "cave_air" | "void_air"
                        )
                    {
                        continue;
                    }
                    nonair += u64::from(count);
                    states
                        .entry(format!("{}|{:?}", state.name, state.states))
                        .or_insert(state);
                }
            }
        }
        if index % 1000 == 0 {
            eprintln!(
                "material audit: read {index}/{} chunks, {} states",
                positions.len(),
                states.len()
            );
        }
    }
    eprintln!(
        "material audit: chunks={} states={} nonair_storage_entries={nonair}",
        positions.len(),
        states.len()
    );
    let config = ChunkServiceConfig {
        world_path,
        world_name: "material-audit".into(),
        dimension: Dimension::Overworld,
        package_roots,
        view_radius: 1,
    };
    let mut worker = Worker::new(&config, world);
    let state_list = states.into_iter().collect::<Vec<_>>();
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let mut audit = MaterialAudit::default();
    let mut face_count = 0usize;
    let mut empty_states = Vec::new();
    // Isolate each state from occluding neighbors while retaining the same captured-model,
    // resource-model and family fallback selection as the streaming editor.
    for (state_index, batch) in state_list.chunks(1).enumerate() {
        let data = isolated_states(pos, batch);
        let blocks =
            p3d::preview_3d_collect_chunk_blocks_with_block_models(&data, Some(&worker.models))
                .unwrap();
        let mut builder = p3d::Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(pos, blocks);
        let before = face_count;
        for chunk in builder.build_stream_faces(&[pos]) {
            face_count += chunk.faces.len();
            for face in chunk.faces {
                audit.check(&mut worker, &face, &batch[0].0);
            }
        }
        if before == face_count {
            empty_states.push(batch[0].0.clone());
        }
        if state_index % 500 == 0 {
            eprintln!(
                "material audit: modeled {} states, {} material/normal pairs, {} missing",
                state_index + 1,
                audit.checked.len(),
                audit.missing.len()
            );
        }
    }
    for (context, face) in &frame_faces {
        audit.check(&mut worker, face, context);
    }
    let report = serde_json::json!({
        "chunks": positions.len(), "unique_states": state_list.len(),
        "nonair_storage_entries": nonair, "generated_faces": face_count,
        "material_normal_pairs": audit.checked.len(), "textures": audit.textures.len(),
        "missing": audit.missing.values().collect::<Vec<_>>(), "pbr_errors": audit.pbr_errors,
        "resolved": audit.resolved, "states_without_faces": empty_states,
        "frame_faces": frame_faces.len(),
    });
    std::fs::create_dir_all("target").unwrap();
    let output = std::env::var_os("BMCBL_WORLD3D_TEST_AUDIT_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/world3d-material-audit.json"));
    std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!(
        "material audit: states={} faces={face_count} pairs={} missing={} pbr_errors={} -> {}",
        state_list.len(),
        audit.checked.len(),
        audit.missing.len(),
        audit.pbr_errors.len(),
        output.display()
    );
    assert!(
        audit.missing.is_empty() && audit.pbr_errors.is_empty(),
        "unresolved editor materials; see {}",
        output.display()
    );
}

#[derive(Default)]
struct MaterialAudit {
    checked: BTreeSet<(String, [i32; 3])>,
    missing: BTreeMap<(String, [i32; 3]), MissingMaterial>,
    pbr_errors: Vec<String>,
    textures: HashMap<gpui_3d::TextureAssetId, Arc<TextureAsset>>,
    resolved: Vec<serde_json::Value>,
}

#[derive(serde::Serialize)]
struct MissingMaterial {
    material: String,
    normal: [i32; 3],
    contexts: BTreeSet<String>,
    stage: &'static str,
    source: Option<PathBuf>,
    error: Option<String>,
}

impl MaterialAudit {
    fn check(&mut self, worker: &mut Worker, face: &p3d::Preview3dStreamFace, context: &str) {
        let key = (face.material.to_string(), face.normal);
        if let Some(missing) = self.missing.get_mut(&key) {
            missing.contexts.insert(context.to_owned());
            return;
        }
        if !self.checked.insert(key.clone()) {
            return;
        }
        worker.last_error = None;
        let source = resolve_material_source(&worker.resolver, &key.0, key.1);
        // Check the actual lookup and image decode before build_chunk_mesh binds
        // the missing-texture placeholder. A valid placeholder binding is not a hit.
        let Some((texture, _)) = worker.resolve_texture(&key.0, key.1) else {
            self.missing.insert(
                key.clone(),
                MissingMaterial {
                    material: key.0,
                    normal: key.1,
                    contexts: BTreeSet::from([context.to_owned()]),
                    stage: if source.is_some() {
                        "image_decode"
                    } else {
                        "path_lookup"
                    },
                    source: source.map(|s| s.source_path.path().to_path_buf()),
                    error: worker.last_error.take(),
                },
            );
            return;
        };
        self.resolved.push(serde_json::json!({
            "material": key.0, "normal": key.1,
            "source": source.as_ref().map(|s| s.source_path.path()),
            "pack": source.as_ref().map(|s| &s.pack_root),
            "width": texture.width(), "height": texture.height(),
        }));
        self.textures.insert(texture.id(), texture);
        for (texture, _) in worker.resolve_pbr(&key.0, key.1).into_iter().flatten() {
            self.textures.insert(texture.id(), texture);
        }
        if let Some(error) = worker.last_error.take() {
            self.pbr_errors.push(format!("{context}: {error}"));
        }
    }
}

fn isolated_states(pos: ChunkPos, states: &[(String, BlockState)]) -> ChunkData {
    let mut palette = vec![BlockState {
        name: "minecraft:air".into(),
        states: BTreeMap::new(),
        version: None,
    }];
    let mut indices = vec![0u16; 4096];
    for (index, (_, state)) in states.iter().enumerate() {
        let x = (index % 4 * 4 + 1) as u8;
        let y = (index / 4 % 4 * 4 + 1) as u8;
        let z = (index / 16 * 4 + 1) as u8;
        indices[bedrock_world::block::block_storage_index(x, y, z)] = palette.len() as u16;
        palette.push(state.clone());
    }
    ChunkData {
        pos,
        is_loaded: true,
        height_map: None,
        legacy_biomes: None,
        legacy_biome_colors: None,
        biome_data: BTreeMap::new(),
        subchunks: BTreeMap::from([(
            0,
            SubChunk {
                y: 0,
                format: SubChunkFormat::Paletted {
                    version: 9,
                    storages: vec![BlockPalette::with_unpacked_indices(palette, indices, None)],
                },
            },
        )]),
        block_entities: Vec::new(),
        legacy_terrain: None,
        column_samples: None,
        version: ChunkVersion::New,
    }
}
