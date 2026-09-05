from pathlib import Path

path = Path('.github/scripts/apply_custom_mesh_texture_binding_20260905.py')
s = path.read_text(encoding='utf-8')
start = s.index('path = "crates/gpui/src/window/paint_resources.rs"')
end = s.index('# -----------------------------------------------------------------------------\n# Nova custom-mesh layout', start)
replacement = r'''path = "crates/gpui/src/window/paint_resources.rs"
s = read(path)
import re
pattern = re.compile(
    r"    pub fn paint_gpu_mesh_3d\(\n"
    r"        &mut self,\n"
    r"        bounds: Bounds<Pixels>,\n"
    r"        mesh: Arc<GpuMesh3d>,\n"
    r"        parameters: GpuMesh3dDrawParameters,\n"
    r"    \) \{\n"
    r".*?"
    r"        self\.next_frame\.scene\.insert_primitive\(PaintGpuMesh3d \{\n"
    r".*?"
    r"        \}\);\n"
    r"    \}\n",
    re.S,
)
matches = list(pattern.finditer(s))
if len(matches) != 1:
    raise RuntimeError(f"window paint gpu mesh resources: expected one function, got {len(matches)}")
new = """    pub fn paint_gpu_mesh_3d(
        &mut self,
        bounds: Bounds<Pixels>,
        mesh: Arc<GpuMesh3d>,
        parameters: GpuMesh3dDrawParameters,
    ) {
        self.paint_gpu_mesh_3d_with_resources(
            bounds,
            mesh,
            parameters,
            GpuMesh3dDrawResources::default(),
        );
    }

    /// Paint a GPU mesh with application-owned sampled resources.
    ///
    /// Sampled textures are uploaded once per `(texture id, generation)` by Nova and are exposed to
    /// runtime WGSL as `@group(0) @binding(4)` with the shared linear sampler at binding 5.
    pub fn paint_gpu_mesh_3d_with_resources(
        &mut self,
        bounds: Bounds<Pixels>,
        mesh: Arc<GpuMesh3d>,
        parameters: GpuMesh3dDrawParameters,
        resources: GpuMesh3dDrawResources,
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = self.visual_bounds(bounds).scale(scale_factor);
        let content_mask = self.visual_content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintGpuMesh3d {
            order: 0,
            bounds,
            content_mask,
            mesh,
            parameters,
            resources,
        });
    }
"""
s = pattern.sub(new, s, count=1)
write(path, s)

'''
s = s[:start] + replacement + s[end:]
path.write_text(s, encoding='utf-8')
print('fixed texture binding patcher window anchor')
