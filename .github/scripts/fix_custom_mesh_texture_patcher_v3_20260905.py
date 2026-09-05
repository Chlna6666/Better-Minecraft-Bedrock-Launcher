from pathlib import Path

path = Path('.github/scripts/apply_custom_mesh_texture_binding_20260905.py')
s = path.read_text(encoding='utf-8')
start = s.index('path = "crates/gpui/src/platform/nova/draw.rs"')
end = s.index('# Renderer draw-step preparation', start)
replacement = r'''path = "crates/gpui/src/platform/nova/draw.rs"
s = read(path)
# Keep the test helper's public argument as a concrete resource set, while the production
# draw-step builder gets a texture-aware lookup callback.
s = replace_once(
    s,
    """    mut backdrop_blur_resource_set: impl FnMut(BackdropBlurConfig) -> Option<ResourceSetId>,\n    custom_mesh_3d_resource_set: ResourceSetId,\n    custom_mesh_3d_indices_buffer: BufferId,\n""",
    """    mut backdrop_blur_resource_set: impl FnMut(BackdropBlurConfig) -> Option<ResourceSetId>,\n    mut custom_mesh_3d_resource_set: impl FnMut(Option<GpuTexture2dId>, u64) -> Option<ResourceSetId>,\n    custom_mesh_3d_indices_buffer: BufferId,\n""",
    "draw production texture resource callback",
)
s = replace_once(
    s,
    """        |_| Some(backdrop_blur_resource_set),\n        custom_mesh_3d_resource_set,\n        custom_mesh_3d_indices_buffer,\n""",
    """        |_| Some(backdrop_blur_resource_set),\n        |_, _| Some(custom_mesh_3d_resource_set),\n        custom_mesh_3d_indices_buffer,\n""",
    "draw test helper texture callback",
)
s = replace_once(
    s,
    """                shader_id,\n                range,\n                first_parameter_index,\n""",
    """                shader_id,\n                sampled_texture_id,\n                sampled_texture_generation,\n                range,\n                first_parameter_index,\n""",
    "draw custom batch texture identity",
)
s = replace_once(
    s,
    """                if let Some(pipeline) = custom_mesh_3d_pipeline(shader_id) {\n                    steps.push(RenderStepDescriptor::DrawIndexed(\n                        DrawIndexedStepDescriptor {\n                            pipeline,\n                            resource_sets: resource_set_list([custom_mesh_3d_resource_set]),\n""",
    """                if let Some(pipeline) = custom_mesh_3d_pipeline(shader_id)\n                    && let Some(resource_set) = custom_mesh_3d_resource_set(\n                        sampled_texture_id,\n                        sampled_texture_generation,\n                    )\n                {\n                    steps.push(RenderStepDescriptor::DrawIndexed(\n                        DrawIndexedStepDescriptor {\n                            pipeline,\n                            resource_sets: resource_set_list([resource_set]),\n""",
    "draw custom batch texture resource selection",
)
write(path, s)

'''
s = s[:start] + replacement + s[end:]
path.write_text(s, encoding='utf-8')
print('patched both custom-mesh draw resource paths')
