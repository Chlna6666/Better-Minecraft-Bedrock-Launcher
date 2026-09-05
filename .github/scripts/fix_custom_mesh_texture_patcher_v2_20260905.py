from pathlib import Path

path = Path('.github/scripts/apply_custom_mesh_texture_binding_20260905.py')
s = path.read_text(encoding='utf-8')

start = s.index('path = "crates/gpui/src/platform/nova/resource_bindings.rs"')
end = s.index('path = "crates/gpui/src/platform/nova/resources/resource_sets.rs"', start)
section = s[start:end]
section = section.replace('    vertices_buffer: BufferId,\\n', '    vertex_buffer: BufferId,\\n')
section = section.replace('                buffer: vertices_buffer,', '                buffer: vertex_buffer,')
s = s[:start] + section + s[end:]

# Avoid borrowing fields from self while `self.backend` is mutably borrowed during buffer promotion.
# Teach the generated source patch to snapshot the fallback sampled resources before entering match.
mesh_start = s.index('path = "crates/gpui/src/platform/nova/renderer/mesh_cache.rs"')
mesh_end = s.index('# Tests constructing PaintGpuMesh3d', mesh_start)
mesh_section = s[mesh_start:mesh_end]
old = '''# The helper call inside promotion is patched by extending the helper signature and existing calls.\ns = s.replace(\n    """                old_vertices,\\n                old_indices,\\n            )?""",'''
new = '''# Snapshot fallback sampled resources before borrowing the backend mutably.\ns = s.replace(\n    """        let old_vertices = self.custom_mesh_3d_vertices_buffer;\\n        let old_indices = self.custom_mesh_3d_indices_buffer;\\n        let (vertices, indices) = match &mut self.backend {""",\n    """        let old_vertices = self.custom_mesh_3d_vertices_buffer;\\n        let old_indices = self.custom_mesh_3d_indices_buffer;\\n        let sampled_texture_view = self.path_texture_view;\\n        let sampler = self.atlas_sampler;\\n        let (vertices, indices) = match &mut self.backend {""",\n)\n# The helper call inside promotion is patched by extending the helper signature and existing calls.\ns = s.replace(\n    """                old_vertices,\\n                old_indices,\\n            )?""",'''
if old not in mesh_section:
    raise RuntimeError('mesh cache patcher anchor missing')
mesh_section = mesh_section.replace(old, new, 1)
mesh_section = mesh_section.replace('                self.path_texture_view,\\n                self.atlas_sampler,', '                sampled_texture_view,\\n                sampler,')
s = s[:mesh_start] + mesh_section + s[mesh_end:]

path.write_text(s, encoding='utf-8')
print('aligned custom mesh texture patcher with current Nova resource API')
