use super::*;

fn position_array(position: Vec3) -> [f32; 3] {
    [position.x, position.y, position.z]
}

fn test_chunk_mesh(mesh: &SceneMesh) -> &bedrock_scene::mesh::ChunkMesh {
    mesh.chunk_meshes
        .first()
        .expect("test fixture produced a mesh")
}

fn test_selection_mesh(bounds: SlimeChunkBounds, positions: &[[i32; 3]]) -> SceneMesh {
    let mut builder = SceneBuilder::new_selection(bounds, 0, false);
    let mut by_chunk = std::collections::BTreeMap::<ChunkPos, Vec<[i32; 3]>>::new();
    for &[x, y, z] in positions {
        let pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
            dimension: bounds.dimension,
        };
        by_chunk.entry(pos).or_default().push([x, y, z]);
    }
    for pos in chunk_positions(bounds) {
        let cubes = by_chunk.remove(&pos).unwrap_or_default();
        builder.insert_stream_chunk(pos, chunk::ChunkBlocks::test_cubes(&cubes));
    }
    builder.build_selection().expect("test selection mesh")
}

#[test]
fn map_viewer_preview_3d_chunk_positions_cover_full_selection_without_span_limit() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_map_render::Dimension::Overworld,
        min_chunk_x: -8,
        max_chunk_x: 7,
        min_chunk_z: -6,
        max_chunk_z: 9,
    };

    let positions = chunk_positions(bounds);

    assert_eq!(bounds_width(bounds), 16);
    assert_eq!(bounds_depth(bounds), 16);
    assert_eq!(positions.len(), 256);
    assert_eq!(
        positions.first().map(|position| (position.x, position.z)),
        Some((-8, -6))
    );
    assert_eq!(
        positions.last().map(|position| (position.x, position.z)),
        Some((7, 9))
    );
}

#[test]
fn map_viewer_preview_3d_sorted_positions_cover_large_selection() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_map_render::Dimension::Overworld,
        min_chunk_x: -40,
        max_chunk_x: 39,
        min_chunk_z: -32,
        max_chunk_z: 31,
    };
    let mut positions = chunk_positions(bounds);

    sort_positions_by_distance(&mut positions, 0, 0);

    assert_eq!(positions.len(), bounds_chunk_count(bounds));
    assert_eq!(
        positions.first().map(|position| (position.x, position.z)),
        Some((0, 0))
    );
    assert!(
        positions.iter().any(|position| {
            position.x == bounds.min_chunk_x && position.z == bounds.min_chunk_z
        })
    );
    assert!(
        positions.iter().any(|position| {
            position.x == bounds.max_chunk_x && position.z == bounds.max_chunk_z
        })
    );
}

#[test]
fn map_viewer_preview_3d_selection_positions_cover_the_full_selection() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_map_render::Dimension::Overworld,
        min_chunk_x: -40,
        max_chunk_x: 39,
        min_chunk_z: -32,
        max_chunk_z: 31,
    };

    let (positions, truncated_chunk_count) = selection_chunk_positions(bounds);

    assert_eq!(positions.len(), bounds_chunk_count(bounds));
    assert_eq!(truncated_chunk_count, 0);
    assert_eq!(
        positions.first().map(|position| (position.x, position.z)),
        Some((0, 0))
    );
    assert!(
        positions.iter().any(|position| {
            position.x == bounds.min_chunk_x && position.z == bounds.min_chunk_z
        })
    );
}

#[test]
fn map_viewer_preview_3d_starts_large_selection_with_one_visible_chunk() {
    let total_chunks = 625;

    assert_eq!(incremental_batch_size(0, total_chunks), 4);
    assert_eq!(incremental_batch_size(4, total_chunks), 156);
}

#[test]
fn progressive_meshing_never_creates_one_chunk_tail_rebuilds() {
    for total in [1, 4, 5, 12, 120, 625, 1000] {
        let mut completed = 0usize;
        let mut updates = 0usize;
        while completed < total {
            let batch = incremental_batch_size(completed, total);
            assert!(batch > 0);
            assert!(batch <= total - completed);
            completed += batch;
            updates += 1;
        }
        assert_eq!(completed, total);
        assert!(updates <= INCREMENTAL_TARGET_UPDATES);
    }
    // At 120 chunks, publish 4 early chunks, then four equal batches;
    // never remesh 100+ finished chunks for a 1-chunk tail update.
    let mut completed = 0;
    let mut stages = Vec::new();
    while completed < 120 {
        completed += incremental_batch_size(completed, 120);
        stages.push(completed);
    }
    assert_eq!(stages, vec![4, 33, 62, 91, 120]);
}

#[test]
fn map_viewer_preview_3d_projection_uses_wgsl_zero_to_one_depth() {
    let near = 0.1;
    let far = 100.0;
    let projection = mat4_perspective(1.0, 1.0, near, far);
    let near_clip = mat4_mul_vec4(projection, [0.0, 0.0, -near, 1.0]);
    let far_clip = mat4_mul_vec4(projection, [0.0, 0.0, -far, 1.0]);

    assert!(near_clip[2].abs() < 0.0001);
    assert!((far_clip[2] - far_clip[3]).abs() < 0.0001);
}

#[test]
fn preview_orbit_keeps_camera_looking_at_model_center() {
    let mut camera = Preview3dCamera::default();
    for (dx, dy) in [(40.0, 10.0), (200.0, -30.0), (-320.0, 75.0)] {
        camera.rotate_orbit(dx, dy);
        let forward = camera.forward();
        let aimed = vec3_add(camera.position, vec3_scale(forward, DEFAULT_DISTANCE));
        assert!(
            vec3_length_squared(aimed) < 1.0e-4,
            "orbit camera should keep the model center on its forward ray"
        );
    }
}

#[test]
fn preview_orbit_preserves_free_camera_translation() {
    let mut camera = Preview3dCamera::default();
    let displacement = [0.35, -0.22, 0.81];
    camera.position = vec3_add(camera.position, displacement);
    camera.rotate_orbit(91.0, -23.0);
    let expected = vec3_add(
        default_camera_position(camera.yaw, camera.pitch),
        displacement,
    );
    assert!(vec3_length_squared(vec3_sub(camera.position, expected)) < 1.0e-5);
}

#[test]
fn map_viewer_preview_3d_camera_wraps_rotation_and_clamps_zoom() {
    let mut camera = Preview3dCamera::default();
    let original_zoom = camera.zoom;

    camera.rotate_orbit(48.0, 24.0);
    assert_ne!(camera.yaw, Preview3dCamera::default().yaw);
    assert_ne!(camera.pitch, Preview3dCamera::default().pitch);
    assert_eq!(camera.zoom, original_zoom);
    let raised_pitch = camera.pitch;
    camera.rotate_orbit(0.0, -240.0);
    assert!(camera.pitch < raised_pitch);
    camera.rotate_orbit(0.0, 10_000.0);
    assert!(camera.pitch >= MIN_PITCH);
    assert!(camera.pitch <= MAX_PITCH);
    let pitch = camera.pitch;
    let yaw = camera.yaw;
    camera.zoom_by(0.001);
    assert_eq!(camera.pitch, pitch);
    assert_eq!(camera.yaw, yaw);
    assert_eq!(camera.zoom, MIN_ZOOM);
    camera.zoom_by(10_000.0);
    assert_eq!(camera.zoom, MAX_ZOOM);
}

#[test]
fn map_viewer_preview_3d_model_rotation_keeps_center_and_moves_off_axis_point() {
    let camera = Preview3dCamera::default();
    let model_rotation = Preview3dModelRotation {
        yaw: 0.6,
        pitch: 0.0,
        mirror_x: false,
        mirror_z: false,
    };
    let center = [0.0, 64.0, 0.0];
    let off_axis_point = [16.0, 64.0, 0.0];
    let center_before = project_preview_point(center, center, 0.01, camera);
    let center_after = {
        let view_proj_model = view_proj_model(1.0, center, 0.01, camera, model_rotation);
        let projected = mat4_mul_vec4(view_proj_model, [center[0], center[1], center[2], 1.0]);
        let reciprocal_w = 1.0 / projected[3].max(0.0001);
        (
            projected[0] * reciprocal_w,
            projected[1] * reciprocal_w,
            projected[2] * reciprocal_w,
        )
    };
    let point_before = project_preview_point(off_axis_point, center, 0.01, camera);
    let point_after = {
        let view_proj_model = view_proj_model(1.0, center, 0.01, camera, model_rotation);
        let projected = mat4_mul_vec4(
            view_proj_model,
            [off_axis_point[0], off_axis_point[1], off_axis_point[2], 1.0],
        );
        let reciprocal_w = 1.0 / projected[3].max(0.0001);
        (
            projected[0] * reciprocal_w,
            projected[1] * reciprocal_w,
            projected[2] * reciprocal_w,
        )
    };

    assert!(
        center_before.0.abs() < 0.05,
        "model center should start near screen center: {center_before:?}"
    );
    assert!(
        center_after.0.abs() < 0.05,
        "model rotation should keep model center near screen center: {center_after:?}"
    );
    let screen_delta = ((point_after.0 - point_before.0).powi(2)
        + (point_after.1 - point_before.1).powi(2))
    .sqrt();
    assert!(
        screen_delta > 0.05,
        "dragging model should move off-axis point on screen: before={point_before:?} after={point_after:?}"
    );
}

#[test]
fn map_viewer_preview_3d_observer_vertical_controls_move_camera_height() {
    let mut camera = Preview3dCamera::default();
    let initial_y = camera.position[1];

    camera.move_from_input(
        Preview3dMovementInput {
            ascend: true,
            ..Preview3dMovementInput::default()
        },
        0.5,
    );
    assert!(camera.position[1] > initial_y);

    let raised_y = camera.position[1];
    camera.move_from_input(
        Preview3dMovementInput {
            descend: true,
            ..Preview3dMovementInput::default()
        },
        0.5,
    );
    assert!(camera.position[1] < raised_y);
}

#[test]
fn map_viewer_preview_3d_drag_down_raises_elevation_view() {
    let mut camera = Preview3dCamera::default();
    let initial_pitch = camera.pitch;

    camera.rotate_orbit(0.0, 80.0);

    assert!(camera.pitch > initial_pitch);
    assert!(camera.pitch <= MAX_PITCH);
}

#[test]
fn map_viewer_preview_3d_pitch_midrange_projection_stays_continuous() {
    let center = [0.0, 64.0, 0.0];
    let scale = 0.01;
    let points = [
        [-8.0, 0.0, -8.0],
        [8.0, 0.0, -8.0],
        [-8.0, 128.0, 8.0],
        [8.0, 128.0, 8.0],
    ];
    let camera_at_44 = Preview3dCamera::new(-0.65, 44.8_f32.to_radians(), 1.0);
    let camera_at_50 = Preview3dCamera::new(-0.65, 50.0_f32.to_radians(), 1.0);

    for point in points {
        let before = project_preview_point(point, center, scale, camera_at_44);
        let after = project_preview_point(point, center, scale, camera_at_50);

        assert!(before.0.is_finite() && before.1.is_finite() && before.2.is_finite());
        assert!(after.0.is_finite() && after.1.is_finite() && after.2.is_finite());
        assert!(
            before.0.abs() < 0.9 && before.1.abs() < 0.9,
            "44.8deg projection should stay in view: {before:?}"
        );
        assert!(
            after.0.abs() < 0.9 && after.1.abs() < 0.9,
            "50deg projection should stay in view: {after:?}"
        );
        assert!(
            (after.0 - before.0).abs() < 0.25 && (after.1 - before.1).abs() < 0.25,
            "pitch 44.8deg to 50deg should be continuous: before={before:?} after={after:?}"
        );
    }
}

#[test]
fn map_viewer_preview_3d_gpu_projection_keeps_typical_meshes_visible() {
    for span in [1, 4] {
        let bounds = SlimeChunkBounds {
            dimension: bedrock_map_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: span - 1,
            min_chunk_z: 0,
            max_chunk_z: span - 1,
        };
        let mesh = test_selection_mesh(bounds, &[[span * 8, 64, span * 8]]);
        let gpu_chunk_mesh = test_chunk_mesh(&mesh);
        let gpu_mesh = gpu_chunk_mesh.mesh.as_ref();
        let camera = Preview3dCamera::default();
        let projected = gpu_mesh
            .vertices()
            .iter()
            .map(|vertex| {
                project_preview_point(
                    position_array(vertex.position),
                    gpu_chunk_mesh.center,
                    gpu_chunk_mesh.fit_scale,
                    camera,
                )
            })
            .collect::<Vec<_>>();

        assert!(!projected.is_empty());
        assert!(projected.iter().any(|position| {
            position.0 >= -1.0 && position.0 <= 1.0 && position.1 >= -1.0 && position.1 <= 1.0
        }));
    }
}

#[test]
fn map_viewer_preview_3d_projection_keeps_y_up_and_x_right() {
    let camera = Preview3dCamera::new(0.0, 0.68, 1.0);
    let center = [0.0, 64.0, 0.0];
    let base = project_preview_point([0.0, 64.0, 0.0], center, 0.1, camera);
    let higher = project_preview_point([0.0, 80.0, 0.0], center, 0.1, camera);
    let right = project_preview_point([8.0, 64.0, 0.0], center, 0.1, camera);

    assert!(
        higher.1 > base.1,
        "positive Y should move upward in NDC coordinates"
    );
    assert!(
        right.0 > base.0,
        "positive X should remain rightward in NDC coordinates"
    );
}

#[test]
fn map_viewer_preview_3d_projection_keeps_vertical_world_edges_upright() {
    let center = [8.0, 64.0, 8.0];
    let camera = Preview3dCamera::new(-0.65, 50.0_f32.to_radians(), 1.0);
    let bottom = project_preview_point([8.0, 0.0, 8.0], center, 0.01, camera);
    let top = project_preview_point([8.0, 128.0, 8.0], center, 0.01, camera);

    assert!(
        (top.0 - bottom.0).abs() < 0.001,
        "world Y edges should not lean horizontally: bottom={bottom:?} top={top:?}"
    );
    assert!(
        top.1 > bottom.1,
        "positive world Y should remain upward: bottom={bottom:?} top={top:?}"
    );
}

#[test]
fn map_viewer_preview_3d_projection_accepts_near_vertical_angles() {
    let center = [8.0, 64.0, 8.0];
    for pitch in [MIN_PITCH, MAX_PITCH] {
        let camera = Preview3dCamera::new(-0.65, pitch, 1.0);
        let projected = project_preview_point([8.0, 64.0, 8.0], center, 0.01, camera);

        assert!(
            projected.0.is_finite() && projected.1.is_finite() && projected.2.is_finite(),
            "near-vertical camera should project finite coordinates: {projected:?}"
        );
    }
}

#[test]
fn map_viewer_preview_3d_mesh_center_keeps_low_and_high_structures_centered() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_map_render::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    for y in [0, 64, 220] {
        let mesh = test_selection_mesh(bounds, &[[8, y, 8]]);
        let gpu_chunk_mesh = test_chunk_mesh(&mesh);
        let gpu_mesh = gpu_chunk_mesh.mesh.as_ref();
        let camera = Preview3dCamera::default();
        let average_y = gpu_mesh
            .vertices()
            .iter()
            .map(|vertex| {
                project_preview_point(
                    position_array(vertex.position),
                    gpu_chunk_mesh.center,
                    gpu_chunk_mesh.fit_scale,
                    camera,
                )
                .1
            })
            .sum::<f32>()
            / gpu_mesh.vertices().len() as f32;

        assert!(
            average_y.abs() < 0.35,
            "y={y} should stay centered, got average NDC y={average_y}"
        );
    }
}

#[test]
fn map_viewer_preview_3d_uses_optical_zoom_before_model_enters_camera() {
    let (geometric_zoom, vertical_fov) = zoom_projection(8.0);

    assert!(geometric_zoom <= MAX_GEOMETRIC_ZOOM);
    assert!(vertical_fov < BASE_FOV_Y_RADIANS);
    assert!(vertical_fov >= MIN_FOV_Y_RADIANS);
}

#[test]
fn map_viewer_preview_3d_clear_resources_cancels_task_and_clears_state() {
    let cancel = CancelFlag::new();
    let cancel_probe = cancel.clone();
    let mut state = Preview3dState {
        status: Preview3dStatus::Ready,
        mesh: Some(Arc::new(test_selection_mesh(
            SlimeChunkBounds {
                dimension: bedrock_map_render::Dimension::Overworld,
                min_chunk_x: 0,
                max_chunk_x: 0,
                min_chunk_z: 0,
                max_chunk_z: 0,
            },
            &[],
        ))),
        signature: Some(Preview3dSelectionSignature {
            bounds: SlimeChunkBounds {
                dimension: bedrock_map_render::Dimension::Overworld,
                min_chunk_x: 0,
                max_chunk_x: 0,
                min_chunk_z: 0,
                max_chunk_z: 0,
            },
        }),
        movement_input: Preview3dMovementInput {
            forward: true,
            ..Preview3dMovementInput::default()
        },
        render_in_flight: true,
        cancel: Some(cancel),
        ..Preview3dState::default()
    };

    state.clear_resources(true);

    assert!(state.mesh.is_none());
    assert!(state.signature.is_none());
    assert!(!state.render_in_flight);
    assert!(state.cancel.is_none());
    assert!(!state.movement_input.any_active());
    assert!(cancel_probe.is_cancelled());
}

#[test]
fn map_viewer_preview_3d_perspective_keeps_large_selection_depth_in_range() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_map_render::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 3,
        min_chunk_z: 0,
        max_chunk_z: 3,
    };
    let mesh = test_selection_mesh(bounds, &[[0, 0, 0], [63, 96, 63]]);
    let gpu_chunk_mesh = test_chunk_mesh(&mesh);
    let gpu_mesh = gpu_chunk_mesh.mesh.as_ref();
    let camera = Preview3dCamera::new(0.55, 1.35, 1.0);
    let device_depths = gpu_mesh
        .vertices()
        .iter()
        .map(|vertex| {
            project_preview_point(
                position_array(vertex.position),
                gpu_chunk_mesh.center,
                gpu_chunk_mesh.fit_scale,
                camera,
            )
            .2
        })
        .collect::<Vec<_>>();

    assert!(
        device_depths
            .iter()
            .all(|depth| *depth > 0.0 && *depth < 1.0)
    );
}

#[test]
fn map_viewer_preview_3d_shader_projection_matches_rust_mirror() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_map_render::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mesh = test_selection_mesh(bounds, &[[8, 72, 8]]);
    let gpu_chunk_mesh = test_chunk_mesh(&mesh);
    let gpu_mesh = gpu_chunk_mesh.mesh.as_ref();
    let camera = Preview3dCamera::new(0.25, Preview3dCamera::default().pitch, 1.7);
    let vertex = gpu_mesh.vertices()[0];
    let projected = project_preview_point(
        position_array(vertex.position),
        gpu_chunk_mesh.center,
        gpu_chunk_mesh.fit_scale,
        camera,
    );
    let parameters = draw_parameters(
        1.0,
        gpu_chunk_mesh.center,
        gpu_chunk_mesh.fit_scale,
        camera,
        Preview3dModelRotation::default(),
    );
    let mut moved_camera = camera;
    moved_camera.position[0] += 2.0;
    let moved_parameters = draw_parameters(
        1.0,
        gpu_chunk_mesh.center,
        gpu_chunk_mesh.fit_scale,
        moved_camera,
        Preview3dModelRotation::default(),
    );
    let rotated_parameters = draw_parameters(
        1.0,
        gpu_chunk_mesh.center,
        gpu_chunk_mesh.fit_scale,
        camera,
        Preview3dModelRotation {
            yaw: 0.5,
            pitch: 0.2,
            mirror_x: false,
            mirror_z: false,
        },
    );

    assert_ne!(parameters, moved_parameters);
    assert_ne!(parameters, rotated_parameters);
    assert!(projected.0.is_finite());
    assert!(projected.1.is_finite());
    assert!(projected.2.is_finite());
}
