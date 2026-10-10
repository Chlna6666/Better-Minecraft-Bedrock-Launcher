use super::*;

type SlotBinding<'a> = (
    &'a mut ResourceSetId,
    ResourceSetLayoutId,
    Vec<ResourceBinding>,
);

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
            renderer.quad_resource_set_layout,
            renderer.shadow_resource_set_layout,
            renderer.underline_resource_set_layout,
            renderer.path_resource_set_layout,
            renderer.path_rasterization_resource_set_layout,
        ),
        (renderer.path_texture_view, sampler),
    );
    append_atlas_bindings(
        &mut bindings,
        &mut renderer.gpu_atlas_textures,
        (slot, buffers),
        sampler,
        (
            renderer.mono_sprite_resource_set_layout,
            renderer.poly_sprite_resource_set_layout,
        ),
    );
    if let Some(targets) = renderer.filters.targets.as_mut() {
        append_blur_bindings(
            &mut bindings,
            targets,
            (slot, &buffers),
            sampler,
            (
                renderer.backdrop_blur_pass_resource_set_layout,
                renderer.backdrop_blur_resource_set_layout,
            ),
        );
    }
    bindings
}

fn frame_bindings<'a>(
    frame: &'a mut FrameResources,
    buffers: &FrameResourceBuffers,
    layouts: (
        ResourceSetLayoutId,
        ResourceSetLayoutId,
        ResourceSetLayoutId,
        ResourceSetLayoutId,
        ResourceSetLayoutId,
    ),
    path: (TextureViewId, SamplerId),
) -> Vec<SlotBinding<'a>> {
    vec![
        (
            &mut frame.resource_sets.path_rasterization_resource_set,
            layouts.4,
            path_rasterization_resource_bindings(buffers),
        ),
        (
            &mut frame.resource_sets.quad_resource_set,
            layouts.0,
            quad_resource_bindings(buffers),
        ),
        (
            &mut frame.resource_sets.shadow_resource_set,
            layouts.1,
            shadow_resource_bindings(buffers),
        ),
        (
            &mut frame.resource_sets.underline_resource_set,
            layouts.2,
            underline_resource_bindings(buffers),
        ),
        (
            &mut frame.path_resource_set,
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
    layouts: (ResourceSetLayoutId, ResourceSetLayoutId),
) {
    let (slot, buffers) = frame;
    for texture in textures.values_mut() {
        if let Some(resource_set) = texture.mono_resource_sets.get_mut(slot) {
            bindings.push((
                resource_set,
                layouts.0,
                mono_atlas_resource_bindings(buffers, texture.texture_view, sampler),
            ));
        }
        if let Some(resource_set) = texture.poly_resource_sets.get_mut(slot) {
            bindings.push((
                resource_set,
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
    layouts: (ResourceSetLayoutId, ResourceSetLayoutId),
) {
    let (slot, buffers) = frame;
    bindings.push((
        &mut targets.source_pass_resource_sets[slot],
        layouts.0,
        backdrop_blur_pass_resource_bindings(buffers, targets.source.texture_view, sampler),
    ));
    for source in &mut targets.isolated_sources {
        bindings.push((
            &mut source.pass_resource_sets[slot],
            layouts.0,
            backdrop_blur_pass_resource_bindings(buffers, source.target.texture_view, sampler),
        ));
    }
    for variant in &mut targets.variants {
        let source_view = variant
            .levels
            .last()
            .map_or(targets.source.texture_view, |level| level.texture_view);
        bindings.push((
            &mut variant.target_resource_sets[slot],
            layouts.1,
            backdrop_blur_resource_bindings(buffers, source_view, sampler),
        ));
        for level in &mut variant.levels {
            bindings.push((
                &mut level.pass_resource_sets[slot],
                layouts.0,
                backdrop_blur_pass_resource_bindings(buffers, level.texture_view, sampler),
            ));
        }
    }
}

fn release_sets<D: BackendResources>(
    device: &mut D,
    sets: impl IntoIterator<Item = ResourceSetId>,
) {
    for set in sets {
        if let Err(error) = device.destroy_resource_set(set) {
            log::error!("failed to retire nova grown resource set: {error}");
        }
    }
}

pub(in crate::platform::nova::renderer) fn rebind_slot<D: BackendResources>(
    renderer: &mut NovaRenderer,
    device: &mut D,
    buffers: FrameResourceBuffers,
) -> Result<()> {
    let bindings = slot_bindings(renderer, buffers);
    let mut created = Vec::with_capacity(bindings.len());
    for (target, layout, resources) in bindings {
        match device.create_resource_set(&ResourceSetDescriptor {
            label: Some("gpui nova grown frame binding".to_owned()),
            layout,
            bindings: resources,
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
        if frame.mono_sprite_resource_set == *old {
            frame.mono_sprite_resource_set = *new;
        }
        if frame.poly_sprite_resource_set == *old {
            frame.poly_sprite_resource_set = *new;
        }
    }
    release_sets(device, replaced.into_iter().map(|(old, _)| old));
    Ok(())
}
