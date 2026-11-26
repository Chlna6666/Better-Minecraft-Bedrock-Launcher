use super::*;

type SlotBinding<'a> = (&'a mut BindGroupId, BindGroupLayoutId, Vec<BindGroupEntry>);

fn slot_bindings(
    renderer: &mut NovaRenderer,
    buffers: FrameResourceBuffers,
) -> Vec<SlotBinding<'_>> {
    let slot = renderer.current_frame_resource_index;
    let sampler = renderer.atlas_sampler;
    // Borrow each owner field independently so all replacements can commit together.
    let mut bindings = frame_bindings(
        &mut renderer.frame_resources[slot],
        &buffers,
        (
            renderer.quad_bind_group_layout,
            renderer.shadow_bind_group_layout,
            renderer.underline_bind_group_layout,
            renderer.path_bind_group_layout,
            renderer.path_rasterization_bind_group_layout,
        ),
        (renderer.path_texture_view, sampler),
    );
    append_atlas_bindings(
        &mut bindings,
        &mut renderer.gpu_atlas_textures,
        (slot, buffers),
        sampler,
        (
            renderer.mono_sprite_bind_group_layout,
            renderer.poly_sprite_bind_group_layout,
        ),
    );
    if let Some(targets) = renderer.filters.targets.as_mut() {
        append_blur_bindings(
            &mut bindings,
            targets,
            (slot, &buffers),
            sampler,
            (
                renderer.backdrop_blur_pass_bind_group_layout,
                renderer.backdrop_blur_bind_group_layout,
            ),
        );
    }
    if let Some(root) = renderer.root_color.target.as_mut() {
        bindings.push((
            &mut root.composite_bind_groups[slot],
            renderer.backdrop_blur_bind_group_layout,
            backdrop_blur_resource_bindings(&buffers, root.target.texture_view, sampler),
        ));
        bindings.push((
            &mut root.pass_bind_groups[slot],
            renderer.backdrop_blur_pass_bind_group_layout,
            backdrop_blur_pass_resource_bindings(&buffers, root.target.texture_view, sampler),
        ));
    }
    bindings
}

fn frame_bindings<'a>(
    frame: &'a mut FrameResources,
    buffers: &FrameResourceBuffers,
    layouts: (
        BindGroupLayoutId,
        BindGroupLayoutId,
        BindGroupLayoutId,
        BindGroupLayoutId,
        BindGroupLayoutId,
    ),
    path: (TextureViewId, SamplerId),
) -> Vec<SlotBinding<'a>> {
    vec![
        (
            &mut frame.bind_groups.path_rasterization_bind_group,
            layouts.4,
            path_rasterization_resource_bindings(buffers),
        ),
        (
            &mut frame.bind_groups.quad_bind_group,
            layouts.0,
            quad_resource_bindings(buffers),
        ),
        (
            &mut frame.bind_groups.shadow_bind_group,
            layouts.1,
            shadow_resource_bindings(buffers),
        ),
        (
            &mut frame.bind_groups.underline_bind_group,
            layouts.2,
            underline_resource_bindings(buffers),
        ),
        (
            &mut frame.path_bind_group,
            layouts.3,
            path_resource_bindings(buffers, path.0, path.1),
        ),
    ]
}

fn append_atlas_bindings<'a>(
    bindings: &mut Vec<SlotBinding<'a>>,
    textures: &'a mut FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    frame: (usize, FrameResourceBuffers),
    sampler: SamplerId,
    layouts: (BindGroupLayoutId, BindGroupLayoutId),
) {
    let (slot, buffers) = frame;
    for texture in textures.values_mut() {
        if let Some(bind_group) = texture.mono_bind_groups.get_mut(slot) {
            bindings.push((
                bind_group,
                layouts.0,
                mono_atlas_resource_bindings(buffers, texture.texture_view, sampler),
            ));
        }
        if let Some(bind_group) = texture.poly_bind_groups.get_mut(slot) {
            bindings.push((
                bind_group,
                layouts.1,
                poly_atlas_resource_bindings(buffers, texture.texture_view, sampler),
            ));
        }
    }
}

fn append_blur_bindings<'a>(
    bindings: &mut Vec<SlotBinding<'a>>,
    targets: &'a mut BackdropBlurTargets,
    frame: (usize, &FrameResourceBuffers),
    sampler: SamplerId,
    layouts: (BindGroupLayoutId, BindGroupLayoutId),
) {
    let (slot, buffers) = frame;
    if let Some(source) = targets.source {
        bindings.push((
            &mut targets.source_pass_bind_groups[slot],
            layouts.0,
            backdrop_blur_pass_resource_bindings(buffers, source.texture_view, sampler),
        ));
    }
    for source in &mut targets.isolated_sources {
        bindings.push((
            &mut source.pass_bind_groups[slot],
            layouts.0,
            backdrop_blur_pass_resource_bindings(buffers, source.target.texture_view, sampler),
        ));
    }
    for variant in &mut targets.variants {
        let source_view = variant
            .levels
            .last()
            .expect("every blur variant owns at least one target")
            .texture_view;
        bindings.push((
            &mut variant.target_bind_groups[slot],
            layouts.1,
            backdrop_blur_resource_bindings(buffers, source_view, sampler),
        ));
        for level in &mut variant.levels {
            bindings.push((
                &mut level.pass_bind_groups[slot],
                layouts.0,
                backdrop_blur_pass_resource_bindings(buffers, level.texture_view, sampler),
            ));
        }
    }
}

fn release_sets<D: ResourceDevice>(device: &mut D, sets: impl IntoIterator<Item = BindGroupId>) {
    for set in sets {
        if let Err(error) = device.destroy_bind_group(set) {
            log::error!("failed to retire nova grown bind group: {error}");
        }
    }
}

pub(in crate::platform::nova::renderer) fn rebind_slot<D: ResourceDevice>(
    renderer: &mut NovaRenderer,
    device: &mut D,
    buffers: FrameResourceBuffers,
) -> Result<()> {
    let bindings = slot_bindings(renderer, buffers);
    let mut created = Vec::with_capacity(bindings.len());
    for (target, layout, resources) in bindings {
        match device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui nova grown frame binding".to_owned()),
            layout,
            entries: resources,
        }) {
            Ok(set) => created.push((target, set)),
            Err(error) => {
                release_sets(device, created.into_iter().map(|(_, set)| set));
                return Err(error.into());
            }
        }
    }
    // Publish only after every dependent set was created successfully.
    let mut replaced = Vec::with_capacity(created.len());
    for (target, new) in created {
        replaced.push((std::mem::replace(target, new), new));
    }
    let frame = &mut renderer.frame_resources[renderer.current_frame_resource_index];
    for (old, new) in &replaced {
        if frame.mono_sprite_bind_group == *old {
            frame.mono_sprite_bind_group = *new;
        }
        if frame.poly_sprite_bind_group == *old {
            frame.poly_sprite_bind_group = *new;
        }
    }
    // Root descriptors belong exclusively to RootColorTarget. The matching
    // Filter slot keeps only its native Texture/View identity, so no borrowed
    // descriptor mirrors survive this atomic binding replacement.
    release_sets(device, replaced.into_iter().map(|(old, _)| old));
    Ok(())
}
