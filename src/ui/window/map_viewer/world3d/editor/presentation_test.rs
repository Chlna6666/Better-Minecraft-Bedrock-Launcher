use super::{ThemeState, World3dEditorInit, World3dEditorView, World3dWorldKey, window_options};
use bedrock_render::{ChunkPos, Dimension};
use gpui::{
    AppContext as _, Application, FrameClock, GpuPowerPreference, RendererBackend,
    RendererFallback, RendererOptions, Timer, Window,
    diagnostics::gpu_owner::{GpuOwnerJobOutcome, metrics_snapshot, samples_since},
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn presents() -> usize {
    let metrics = gpui::diagnostics::performance_metrics::snapshot();
    metrics.direct_present_count + metrics.retained_present_count
}

fn latest_owner_job() -> Option<(u64, u64)> {
    let owner = metrics_snapshot()
        .windows
        .into_iter()
        .find(|owner| owner.window_id.is_some())?;
    let id = owner.owner_id;
    let last = samples_since(id, 0).last().map_or(0, |sample| sample.job_id);
    Some((id, last))
}

fn report_phase(label: &str, owner: Option<(u64, u64)>, frames: usize, elapsed: Duration) {
    let Some((id, job)) = owner else { return };
    let samples = samples_since(id, job);
    let submitted = samples
        .iter()
        .filter(|sample| sample.outcome == GpuOwnerJobOutcome::Submitted)
        .collect::<Vec<_>>();
    let mut gaps = submitted
        .windows(2)
        .map(|pair| pair[1].completed_at_us.saturating_sub(pair[0].completed_at_us))
        .collect::<Vec<_>>();
    gaps.sort_unstable();
    let fps = frames as f64 / elapsed.as_secs_f64();
    let max_phase = |phase: fn(&gpui::diagnostics::gpu_owner::GpuOwnerJobPhases) -> u64| {
        samples.iter().map(|sample| phase(&sample.phases)).max().unwrap_or(0) as f64 / 1000.
    };
    eprintln!(
        "world3d editor phase={label} fps={fps:.2} gaps={} gap_p95_ms={:.3} gap_max_ms={:.3} camera_max_ms={:.3} resource_max_ms={:.3} buffer_max_ms={:.3} command_max_ms={:.3} submit_max_ms={:.3}",
        gaps.len(),
        *gaps.get(gaps.len() * 95 / 100).unwrap_or(&0) as f64 / 1000.,
        *gaps.last().unwrap_or(&0) as f64 / 1000.,
        max_phase(|p| p.extension_camera_cull_us),
        max_phase(|p| p.extension_resource_sync_us),
        max_phase(|p| p.extension_buffer_encode_us),
        max_phase(|p| p.extension_command_encode_us),
        max_phase(|p| p.backend_submit_us),
    );
}

fn published_frustum_sections(
    editor: &World3dEditorView,
    window: &Window,
) -> BTreeSet<(ChunkPos, i32)> {
    let (_, _, width, height) = editor.stage_rect(window);
    let camera = editor.navigation.camera();
    let view_projection = camera
        .gpu_camera([0; 3])
        .view_projection(width / height)
        .unwrap();
    let center_x = (camera.position[0] / 16.0).floor() as i32;
    let center_z = (camera.position[2] / 16.0).floor() as i32;
    editor
        .chunk_meshes
        .iter()
        .filter_map(|(&key, section)| {
            let (pos, _) = key;
            if editor.dirty_sections.contains(&key)
                || !super::super::chunk_range::contains(
                    i64::from(pos.x) - i64::from(center_x),
                    i64::from(pos.z) - i64::from(center_z),
                    editor.view_radius,
                )
            {
                return None;
            }
            let bounds = section.mesh.bounds()?;
            let mut outside_all_corners = [true; 6];
            for x in [bounds.min.x, bounds.max.x] {
                for y in [bounds.min.y, bounds.max.y] {
                    for z in [bounds.min.z, bounds.max.z] {
                        let [x, y, z, w] = view_projection.transform4([
                            x + section.origin[0] as f32,
                            y + section.origin[1] as f32,
                            z + section.origin[2] as f32,
                            1.0,
                        ]);
                        for (outside, plane) in outside_all_corners
                            .iter_mut()
                            .zip([w + x, w - x, w + y, w - y, z, w - z])
                        {
                            *outside &= plane < 0.0;
                        }
                    }
                }
            }
            outside_all_corners.into_iter().all(|outside| !outside).then_some(key)
        })
        .collect()
}

#[test]
#[ignore = "requires a Bedrock world, the resource package and a native Windows DX11 window"]
fn streaming_editor_presents_while_moving() {
    let world_path = PathBuf::from(
        std::env::var_os("BMCBL_WORLD3D_TEST_WORLD").expect("readable Bedrock world"),
    );
    let package_roots = std::env::var_os("BMCBL_WORLD3D_TEST_PACKAGE")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    let init = World3dEditorInit {
        world_key: World3dWorldKey::from_path(&world_path),
        world_path,
        world_name: "DX11 streaming benchmark".into(),
        dimension: Dimension::Overworld,
        spawn: Some([-108.0, 144.0, 178.0]),
        spawn_source: "fixed benchmark camera".into(),
        package_roots,
        owner: None,
        initial_target: None,
        initial_selection: None,
        view_radius: 32,
    };
    let measured = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&measured);
    Application::with_renderer_options(RendererOptions {
        backend: RendererBackend::NovaDx11,
        fallback: RendererFallback::Disabled,
        power_preference: GpuPowerPreference::HighPerformance,
        frame_metrics: true,
        ..RendererOptions::default()
    })
    .run(move |cx| {
        cx.set_global(ThemeState::default());
        let options = window_options(cx, "DX11 streaming benchmark".into());
        let handle = cx
            .open_window(options, move |window, cx| {
                window.set_frame_clock(FrameClock::fixed_fps(120).unwrap());
                cx.new(|cx| {
                    let mut editor = World3dEditorView::new(init, window, cx);
                    editor.camera.speed = 60.0;
                    editor.navigation.set_camera(editor.camera);
                    editor.navigation.script_input(None, |_| {});
                    editor.request_view_update(true, cx);
                    editor
                })
            })
            .expect("open native 3D editor");
        cx.activate(true);
        cx.spawn(async move |cx| {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                let ready = cx
                    .update(|cx| {
                        handle.update(cx, |editor, _, _| {
                            editor.chunk_meshes.len() >= 1056 && editor.scene_view.is_some()
                        })
                    })
                    .unwrap()
                    .unwrap();
                if ready || Instant::now() >= deadline {
                    break;
                }
                Timer::after(Duration::from_millis(50)).await;
            }
            let idle_owner = latest_owner_job();
            let idle_frames = presents();
            let idle = Instant::now();
            Timer::after(Duration::from_secs(1)).await;
            report_phase("idle", idle_owner, presents() - idle_frames, idle.elapsed());
            let owner = latest_owner_job();
            let baseline = presents();
            let start = Instant::now();
            let (sections, initial_position, initial_scene) = cx
                .update(|cx| {
                    handle.update(cx, |editor, _, cx| {
                        // Strafe on the horizontal plane while the camera
                        // keeps looking down at the world.
                        editor.navigation.script_input(Some("d"), |_| {});
                        cx.notify();
                        (
                            editor.chunk_meshes.len(),
                            editor.navigation.camera().position,
                            editor.scene_view.as_ref().map(|view| view.scene().version()),
                        )
                    })
                })
                .unwrap()
                .unwrap();
            Timer::after(Duration::from_secs(1)).await;
            report_phase("move_start", owner, presents() - baseline, start.elapsed());
            let steady_owner = latest_owner_job();
            let steady_frames = presents();
            let steady = Instant::now();
            Timer::after(Duration::from_secs(8).saturating_sub(start.elapsed())).await;
            let fps = presents().saturating_sub(baseline) as f64 / start.elapsed().as_secs_f64();
            report_phase(
                "move_steady", steady_owner, presents() - steady_frames, steady.elapsed(),
            );
            let (end_sections, end_position, latest_scene, at_arrival, first_meshes) = cx
                .update(|cx| {
                    handle.update(cx, |editor, window, cx| {
                        editor.navigation.script_input(None, |_| {});
                        editor.stop_navigation(cx);
                        (
                            editor.chunk_meshes.len(),
                            editor.navigation.camera().position,
                            editor.scene_view.as_ref().map(|view| view.scene().version()),
                            published_frustum_sections(editor, window),
                            editor.worker_status.meshed_chunks,
                        )
                    })
                })
                .unwrap()
                .unwrap();
            let mut gaps = Vec::new();
            if let Some((id, baseline)) = owner {
                let mut previous = None;
                for sample in samples_since(id, baseline)
                    .iter()
                    .filter(|sample| sample.outcome == GpuOwnerJobOutcome::Submitted)
                {
                    if let Some(previous) = previous {
                        gaps.push(sample.completed_at_us.saturating_sub(previous) as f64 / 1000.);
                    }
                    previous = Some(sample.completed_at_us);
                }
            }
            gaps.sort_unstable_by(f64::total_cmp);
            let p99 = gaps.get(gaps.len().saturating_mul(99) / 100).copied();
            let distance = end_position[0] - initial_position[0];
            let horizontal = end_position[1..] == initial_position[1..];
            let published = initial_scene != latest_scene;
            eprintln!(
                "world3d editor streaming fps={fps:.2} submitted_gap_p99_ms={p99:?} sections={sections}->{end_sections} moved_x={distance:.1} height={:.1}->{:.1} published={published}",
                initial_position[1], end_position[1],
            );
            // Compare the same pose as more work completes, so rear sections and
            // already-evicted columns can't inflate the arrival coverage.
            Timer::after(Duration::from_secs(6)).await;
            let (after_wait, settled_meshes) = cx
                .update(|cx| {
                    handle.update(cx, |editor, window, _| {
                        (published_frustum_sections(editor, window), editor.worker_status.meshed_chunks)
                    })
                })
                .unwrap()
                .unwrap();
            let still_present = at_arrival.intersection(&after_wait).count();
            eprintln!(
                "world3d editor frustum arrival={} after_6s={} later={} removed={} meshed_columns={} -> {}",
                at_arrival.len(), after_wait.len(), after_wait.difference(&at_arrival).count(),
                at_arrival.len() - still_present, first_meshes, settled_meshes,
            );
            let turn_owner = latest_owner_job();
            let turn_frames = presents();
            let turn = Instant::now();
            for _ in 0..60 {
                cx.update(|cx| {
                    handle.update(cx, |editor, _, cx| {
                        editor.navigation.script_input(None, |camera| camera.rotate_look(0.012, 0.));
                        cx.notify();
                    })
                })
                .unwrap()
                .unwrap();
                Timer::after(Duration::from_millis(16)).await;
            }
            report_phase("turn_in_place", turn_owner, presents() - turn_frames, turn.elapsed());
            let expected_distance = 60.0 * start.elapsed().as_secs_f64().min(8.0);
            let valid = fps > 0.0
                && (distance - expected_distance).abs() <= 12.0
                && horizontal && sections >= 1056 && published;
            let enforce = std::env::var_os("BMCBL_WORLD3D_TEST_ENFORCE_GATE").is_some();
            observed.store(
                valid && (!enforce || (fps >= 120.0 && p99.is_some_and(|gap| gap <= 8.33))),
                Ordering::Release,
            );
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    assert!(
        measured.load(Ordering::Acquire),
        "editor streaming acceptance failed"
    );
}
