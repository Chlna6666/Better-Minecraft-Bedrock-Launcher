use bedrock_render::{
    BakeOptions, ChunkRegion, ImageFormat, MapRenderer, RenderBackend, RenderExecutionProfile,
    RenderGpuBackend, RenderGpuFallbackPolicy, RenderGpuOptions, RenderGpuPipelineLevel, RenderJob,
    RenderLayout, RenderMemoryBudget, RenderMode, RenderOptions, RenderPalette,
    RenderThreadingOptions, TileCoord,
    editor::{MapEditInvalidation, MapWorldEditor},
};
use bedrock_world::{
    BedrockLevelDbStorage, World, OpenOptions, ChunkPos, Dimension,
    GlobalRecordKind, SlimeChunkBounds,
};
use bedrock_world::surface::WorldScanOptions;
use criterion::{Criterion, criterion_group, criterion_main};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

fn fixture_world_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bedrock-world")
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world")
}

fn renderer() -> Option<MapRenderer<BedrockLevelDbStorage>> {
    let world_path = fixture_world_path();
    if !world_path.join("db").join("CURRENT").exists() {
        return None;
    }
    let storage = BedrockLevelDbStorage::open(world_path.join("db")).ok()?;
    let world = Arc::new(World::from_typed_storage(
        world_path,
        storage,
        OpenOptions::default(),
    ));
    Some(MapRenderer::new(world, RenderPalette::default()))
}

fn dynamic_renderer() -> Option<MapRenderer> {
    let world_path = fixture_world_path();
    if !world_path.join("db").join("CURRENT").exists() {
        return None;
    }
    let storage = Arc::new(BedrockLevelDbStorage::open(world_path.join("db")).ok()?);
    let world = Arc::new(World::from_storage(
        world_path,
        storage,
        OpenOptions::default(),
    ));
    Some(MapRenderer::new(world, RenderPalette::default()))
}

fn emit_machine_readable_report(
    case: &str,
    storage: &str,
    renderer: &MapRenderer<impl bedrock_world::StorageBackend>,
) {
    emit_machine_readable_report_with_options(
        case,
        storage,
        "default",
        renderer,
        RenderOptions {
            format: ImageFormat::Rgba,
            threading: RenderThreadingOptions::Auto,
            execution_profile: RenderExecutionProfile::Export,
            memory_budget: RenderMemoryBudget::Disabled,
            ..RenderOptions::default()
        },
    );
}

fn emit_machine_readable_report_with_options(
    case: &str,
    storage: &str,
    backend_label: &str,
    renderer: &MapRenderer<impl bedrock_world::StorageBackend>,
    options: RenderOptions,
) {
    let region = ChunkRegion::new(Dimension::Overworld, 0, 0, 15, 15);
    let planned = MapRenderer::<BedrockLevelDbStorage>::plan_region_tiles(
        region,
        RenderMode::SurfaceBlocks,
        RenderLayout::default(),
    )
    .expect("plan benchmark report tiles");
    let rendered_tiles = AtomicUsize::new(0);
    let start = Instant::now();
    let result = renderer
        .render_web_tiles(&planned, options, |_planned, _tile| {
            rendered_tiles.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
        .expect("render benchmark report tiles");
    let elapsed_ms = start.elapsed().as_millis();
    let adapter = sanitize_report_value(result.stats.gpu_adapter_name.as_deref().unwrap_or("none"));
    let device = sanitize_report_value(result.stats.gpu_device_name.as_deref().unwrap_or("none"));
    let fallback = sanitize_report_value(
        result
            .stats
            .gpu_fallback_reason
            .as_deref()
            .unwrap_or("none"),
    );
    println!(
        "bedrock_render_report case={case} storage={storage} backend={backend_label} elapsed_ms={elapsed_ms} tiles={} worker_threads={} world_worker_threads={} prefix_scans={} exact_get_batches={} exact_keys_requested={} exact_keys_found={} db_read_ms={} decode_ms={} gpu_tiles={} cpu_tiles={} gpu_requested={:?} gpu_actual={:?} gpu_adapter={} gpu_device={} gpu_fallback={} gpu_upload_ms={} gpu_dispatch_ms={} gpu_readback_ms={} gpu_uploaded_bytes={} gpu_readback_bytes={} gpu_peak_in_flight={} gpu_buffer_reuses={}",
        rendered_tiles.load(Ordering::Relaxed),
        result.stats.peak_worker_threads,
        result.stats.world_worker_threads,
        result.stats.render_prefix_scans,
        result.stats.exact_get_batches,
        result.stats.exact_keys_requested,
        result.stats.exact_keys_found,
        result.stats.db_read_ms,
        result.stats.decode_ms,
        result.stats.gpu_tiles,
        result.stats.cpu_tiles,
        result.stats.gpu_requested_backend,
        result.stats.gpu_actual_backend,
        adapter,
        device,
        fallback,
        result.stats.gpu_upload_ms,
        result.stats.gpu_dispatch_ms,
        result.stats.gpu_readback_ms,
        result.stats.gpu_uploaded_bytes,
        result.stats.gpu_readback_bytes,
        result.stats.gpu_peak_in_flight,
        result.stats.gpu_buffer_reuses,
    );
}

const RUN_GPU_COMPARISON_REPORTS: bool = false;
const RUN_FULL_BENCHMARKS: bool = false;

fn emit_gpu_comparison_reports(renderer: &MapRenderer<impl bedrock_world::StorageBackend>) {
    if !RUN_GPU_COMPARISON_REPORTS {
        return;
    }
    for (label, backend, gpu_backend) in [
        ("cpu", RenderBackend::Cpu, RenderGpuBackend::Auto),
        ("auto", RenderBackend::Auto, RenderGpuBackend::Auto),
        ("dx11", RenderBackend::Wgpu, RenderGpuBackend::Dx11),
        ("vulkan", RenderBackend::Wgpu, RenderGpuBackend::Vulkan),
    ] {
        emit_machine_readable_report_with_options(
            "surface_region_rgba_gpu_compare",
            "generic",
            label,
            renderer,
            RenderOptions {
                format: ImageFormat::Rgba,
                backend,
                gpu: RenderGpuOptions {
                    backend: gpu_backend,
                    fallback_policy: RenderGpuFallbackPolicy::AllowCpu,
                    pipeline_level: RenderGpuPipelineLevel::ComposeOnly,
                    max_in_flight: 2,
                    batch_pixels: 2048 * 2048,
                    staging_pool_bytes: 64 * 1024 * 1024,
                    diagnostics: true,
                },
                threading: RenderThreadingOptions::Auto,
                execution_profile: RenderExecutionProfile::Export,
                memory_budget: RenderMemoryBudget::Disabled,
                ..RenderOptions::default()
            },
        );
    }
}

fn sanitize_report_value(value: &str) -> String {
    if value.is_empty() {
        return "none".to_string();
    }
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn emit_v02_editor_reports() {
    let world_path = fixture_world_path();
    if !world_path.join("db").join("CURRENT").exists() {
        return;
    }
    let Ok(editor) =
        MapWorldEditor::open_with_options(&world_path, OpenOptions::default())
    else {
        return;
    };
    emit_v02_overlay_report(&editor);
    emit_v02_map_report(&editor);
    emit_v02_global_report(&editor);
    emit_v02_hsa_report(&editor);
    emit_v02_invalidation_report();
}

fn emit_v02_overlay_report(editor: &MapWorldEditor) {
    let region = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 15,
        min_chunk_z: 0,
        max_chunk_z: 15,
    };
    let overlay_options = bedrock_render::editor::RegionOverlayQueryOptions {
        include_slime: true,
        include_entities: true,
        include_block_entities: true,
        include_pending_ticks: true,
        include_villages: true,
        include_hardcoded_spawn_areas: true,
        max_chunks: 4_096,
        max_items_per_kind: 10_000,
    };
    let start = Instant::now();
    let overlay = bedrock_render::editor::region_overlays(
        editor.world(),
        region,
        overlay_options,
        None,
    );
    let overlay_ms = start.elapsed().as_millis();
    if let Ok(overlay) = overlay {
        println!(
            "bedrock_render_report case=v02_overlay_query storage=editor elapsed_ms={overlay_ms} chunks={} entities={} block_entities={} hsa={} villages={}",
            overlay.scanned_chunks,
            overlay.entities.len(),
            overlay.block_entities.len(),
            overlay.hardcoded_spawn_areas.len(),
            overlay.villages.len(),
        );
    }
}

fn emit_v02_map_report(editor: &MapWorldEditor) {
    let start = Instant::now();
    let maps = editor.map_items(WorldScanOptions::default());
    let map_ms = start.elapsed().as_millis();
    if let Ok(maps) = maps {
        println!(
            "bedrock_render_report case=v02_map_scan storage=editor elapsed_ms={map_ms} records={}",
            maps.len(),
        );
    }
}

fn emit_v02_global_report(editor: &MapWorldEditor) {
    let start = Instant::now();
    let globals = editor.globals(WorldScanOptions::default());
    let global_ms = start.elapsed().as_millis();
    match globals {
        Ok(globals) => {
            let scoreboard_found = editor
                .global(GlobalRecordKind::Scoreboard)
                .ok()
                .flatten()
                .is_some();
            println!(
                "bedrock_render_report case=v02_global_scan storage=editor elapsed_ms={global_ms} records={} scoreboard_found={scoreboard_found} error=none",
                globals.len(),
            );
        }
        Err(error) => {
            let error = sanitize_report_value(&error.to_string());
            println!(
                "bedrock_render_report case=v02_global_scan storage=editor elapsed_ms={global_ms} records=0 scoreboard_found=false error={error}",
            );
        }
    }
}

fn emit_v02_hsa_report(editor: &MapWorldEditor) {
    let start = Instant::now();
    let hsa = editor.hardcoded_spawn_areas(WorldScanOptions::default());
    let hsa_ms = start.elapsed().as_millis();
    if let Ok(hsa) = hsa {
        let area_count = hsa.iter().map(|(_, areas)| areas.len()).sum::<usize>();
        println!(
            "bedrock_render_report case=v02_hsa_scan storage=editor elapsed_ms={hsa_ms} chunks={} areas={area_count}",
            hsa.len(),
        );
    }
}

fn emit_v02_invalidation_report() {
    let start = Instant::now();
    let invalidation = MapEditInvalidation::chunks([
        ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 1,
            z: 0,
            dimension: Dimension::Overworld,
        },
    ])
    .with_metadata();
    let invalidation_ms = start.elapsed().as_nanos();
    println!(
        "bedrock_render_report case=v02_edit_invalidation storage=memory elapsed_ns={invalidation_ms} affected_chunks={} refresh_metadata={} refresh_overlays={} clear_tile_cache={}",
        invalidation.affected_chunks().len(),
        invalidation.refresh_metadata(),
        invalidation.refresh_overlays(),
        invalidation.clear_tile_cache(),
    );
}

// Keep the related Criterion cases in one harness so shared fixture setup is measured consistently.
#[allow(clippy::too_many_lines)]
fn render_benches(c: &mut Criterion) {
    let Some(renderer) = renderer() else {
        return;
    };
    emit_machine_readable_report("surface_region_rgba", "generic", &renderer);
    emit_gpu_comparison_reports(&renderer);
    emit_v02_editor_reports();
    if let Some(dynamic) = dynamic_renderer() {
        emit_machine_readable_report("surface_region_rgba", "dynamic", &dynamic);
    }
    let coord = TileCoord {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    c.bench_function("bedrock_render/biome_tile_256_rgba", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::Biome { y: 64 }),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render biome tile");
        });
    });
    c.bench_function("bedrock_render/biome_tile_256_webp", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::Biome { y: 64 }),
                    &RenderOptions::default(),
                )
                .expect("render biome tile");
        });
    });
    c.bench_function("bedrock_render/fixed_y_tile_256_rgba", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::LayerBlocks { y: 64 }),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render layer tile");
        });
    });
    c.bench_function("bedrock_render/raw_biome_tile_256_rgba", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::RawBiomeLayer { y: 64 }),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render raw biome tile");
        });
    });
    c.bench_function("bedrock_render/surface_tile_256_rgba", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::SurfaceBlocks),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render surface tile");
        });
    });
    c.bench_function("bedrock_render/bake_chunk_surface", |bench| {
        bench.iter(|| {
            renderer
                .bake_chunk(
                    ChunkPos {
                        x: 0,
                        z: 0,
                        dimension: Dimension::Overworld,
                    },
                    BakeOptions {
                        mode: RenderMode::SurfaceBlocks,
                        ..BakeOptions::default()
                    },
                )
                .expect("bake surface chunk");
        });
    });
    c.bench_function("bedrock_render/render_tile_surface_from_bake", |bench| {
        bench.iter(|| {
            renderer
                .render_tile_from_bake(
                    RenderJob::new(coord, RenderMode::SurfaceBlocks),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        threading: RenderThreadingOptions::Auto,
                        ..RenderOptions::default()
                    },
                )
                .expect("render surface from bake");
        });
    });
    c.bench_function("bedrock_render/heightmap_tile_256_rgba", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::HeightMap),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render heightmap tile");
        });
    });
    c.bench_function("bedrock_render/cave_slice_tile_256_rgba", |bench| {
        bench.iter(|| {
            renderer
                .render_tile(
                    RenderJob::new(coord, RenderMode::CaveSlice { y: 32 }),
                    &RenderOptions {
                        format: ImageFormat::Rgba,
                        ..RenderOptions::default()
                    },
                )
                .expect("render cave slice tile");
        });
    });
    let jobs = (0..4)
        .map(|x| {
            RenderJob::new(
                TileCoord {
                    x,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::Biome { y: 64 },
            )
        })
        .collect::<Vec<_>>();
    c.bench_function("bedrock_render/tile_batch_auto_threads", |bench| {
        bench.iter(|| {
            renderer
                .render_tiles(
                    jobs.clone(),
                    RenderOptions {
                        format: ImageFormat::Rgba,
                        threading: RenderThreadingOptions::Auto,
                        ..RenderOptions::default()
                    },
                )
                .expect("render batch");
        });
    });
    c.bench_function("bedrock_render/tile_batch_single_thread", |bench| {
        bench.iter(|| {
            renderer
                .render_tiles(
                    jobs.clone(),
                    RenderOptions {
                        format: ImageFormat::Rgba,
                        threading: RenderThreadingOptions::Single,
                        ..RenderOptions::default()
                    },
                )
                .expect("render batch");
        });
    });
    let interactive_region = ChunkRegion::new(Dimension::Overworld, 0, 0, 31, 31);
    let interactive_tiles = MapRenderer::<BedrockLevelDbStorage>::plan_region_tiles(
        interactive_region,
        RenderMode::SurfaceBlocks,
        RenderLayout::default(),
    )
    .expect("plan interactive benchmark tiles");
    c.bench_function(
        "bedrock_render/interactive_ordered_surface_batch_2x2_rgba",
        |bench| {
            bench.iter(|| {
                renderer
                    .render_web_tiles(
                        &interactive_tiles,
                        RenderOptions {
                            format: ImageFormat::Rgba,
                            threading: RenderThreadingOptions::Fixed(4),
                            execution_profile: RenderExecutionProfile::Interactive,
                            memory_budget: RenderMemoryBudget::Disabled,
                            ..RenderOptions::default()
                        },
                        |_planned, _tile| Ok(()),
                    )
                    .expect("render interactive batch");
            });
        },
    );

    if !RUN_FULL_BENCHMARKS {
        return;
    }

    let web_region = ChunkRegion::new(Dimension::Overworld, 0, 0, 31, 31);
    let web_tiles = MapRenderer::<BedrockLevelDbStorage>::plan_region_tiles(
        web_region,
        RenderMode::SurfaceBlocks,
        RenderLayout::default(),
    )
    .expect("plan web tiles");
    for threads in [1_usize, 8, 16, 32] {
        c.bench_function(
            &format!("bedrock_render/web_export_surface_threads_{threads}"),
            |bench| {
                bench.iter(|| {
                    renderer
                        .render_web_tiles(
                            &web_tiles,
                            RenderOptions {
                                format: ImageFormat::Rgba,
                                threading: if threads == 1 {
                                    RenderThreadingOptions::Single
                                } else {
                                    RenderThreadingOptions::Fixed(threads)
                                },
                                execution_profile: RenderExecutionProfile::Export,
                                memory_budget: RenderMemoryBudget::Disabled,
                                ..RenderOptions::default()
                            },
                            |_planned, _tile| Ok(()),
                        )
                        .expect("web export surface");
                });
            },
        );
    }
    c.bench_function("bedrock_render/global_chunk_bake_queue", |bench| {
        bench.iter(|| {
            renderer
                .render_web_tiles(
                    &web_tiles,
                    RenderOptions {
                        format: ImageFormat::Rgba,
                        threading: RenderThreadingOptions::Auto,
                        execution_profile: RenderExecutionProfile::Export,
                        memory_budget: RenderMemoryBudget::Disabled,
                        ..RenderOptions::default()
                    },
                    |_planned, _tile| Ok(()),
                )
                .expect("global bake queue");
        });
    });
    c.bench_function("bedrock_render/tile_compose_encode_pipeline", |bench| {
        bench.iter(|| {
            renderer
                .render_web_tiles(
                    &web_tiles,
                    RenderOptions {
                        format: ImageFormat::WebP,
                        threading: RenderThreadingOptions::Auto,
                        execution_profile: RenderExecutionProfile::Export,
                        memory_budget: RenderMemoryBudget::Disabled,
                        ..RenderOptions::default()
                    },
                    |_planned, _tile| Ok(()),
                )
                .expect("compose encode pipeline");
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(2));
    targets = render_benches
}
criterion_main!(benches);
