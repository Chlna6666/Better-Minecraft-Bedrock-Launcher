use super::*;

impl NovaRenderer {
    pub(super) fn ensure_custom_mesh_3d_pipelines_for_current_backend(&mut self) -> Result<()> {
        let shaders = self.frame_upload.custom_mesh_3d_shaders.clone();
        for shader in shaders {
            if self.custom_mesh_3d_pipelines.contains_key(&shader.id)
                || self.custom_mesh_3d_pipeline_failures.contains(&shader.id)
            {
                continue;
            }
            match self.create_custom_mesh_3d_pipeline_for_current_backend(&shader) {
                Ok(pipeline) => {
                    self.custom_mesh_3d_pipelines.insert(shader.id, pipeline);
                }
                Err(error) => {
                    self.custom_mesh_3d_pipeline_failures.insert(shader.id);
                    log::warn!(
                        "failed to create nova custom GPU mesh 3D pipeline label={} vertex={} fragment={}: {error:#}",
                        shader.source.label(),
                        shader.vertex_entry_point,
                        shader.fragment_entry_point
                    );
                }
            }
        }
        Ok(())
    }

    fn create_custom_mesh_3d_pipeline_for_current_backend(
        &mut self,
        shader: &GpuMesh3dShader,
    ) -> Result<RenderPipelineId> {
        match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let vertex_shader = compile_wgsl_to_hlsl(
                    shader.source.source(),
                    ShaderStage::Vertex,
                    &shader.vertex_entry_point,
                )
                .context("compiling nova custom GPU mesh 3D vertex shader to HLSL")?;
                let fragment_shader = compile_wgsl_to_hlsl(
                    shader.source.source(),
                    ShaderStage::Fragment,
                    &shader.fragment_entry_point,
                )
                .context("compiling nova custom GPU mesh 3D fragment shader to HLSL")?;
                create_custom_mesh_3d_pipeline(
                    device,
                    "gpui nova dx12",
                    self.render_pass,
                    self.custom_mesh_3d_pipeline_layout,
                    self.surface_config,
                    vertex_shader,
                    fragment_shader,
                    &shader.vertex_entry_point,
                    &shader.fragment_entry_point,
                )
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                let vertex_shader = compile_wgsl_to_msl(
                    shader.source.source(),
                    ShaderStage::Vertex,
                    &shader.vertex_entry_point,
                )
                .context("compiling nova custom GPU mesh 3D vertex shader to MSL")?;
                let fragment_shader = compile_wgsl_to_msl(
                    shader.source.source(),
                    ShaderStage::Fragment,
                    &shader.fragment_entry_point,
                )
                .context("compiling nova custom GPU mesh 3D fragment shader to MSL")?;
                create_custom_mesh_3d_pipeline(
                    device,
                    "gpui nova metal",
                    self.render_pass,
                    self.custom_mesh_3d_pipeline_layout,
                    self.surface_config,
                    vertex_shader,
                    fragment_shader,
                    &shader.vertex_entry_point,
                    &shader.fragment_entry_point,
                )
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                let vertex_shader = compile_wgsl_to_spirv(
                    shader.source.source(),
                    ShaderStage::Vertex,
                    &shader.vertex_entry_point,
                )
                .context("compiling nova custom GPU mesh 3D vertex shader to SPIR-V")?;
                let fragment_shader = compile_wgsl_to_spirv(
                    shader.source.source(),
                    ShaderStage::Fragment,
                    &shader.fragment_entry_point,
                )
                .context("compiling nova custom GPU mesh 3D fragment shader to SPIR-V")?;
                create_custom_mesh_3d_pipeline(
                    device,
                    "gpui nova vulkan",
                    self.render_pass,
                    self.custom_mesh_3d_pipeline_layout,
                    self.surface_config,
                    vertex_shader,
                    fragment_shader,
                    &shader.vertex_entry_point,
                    &shader.fragment_entry_point,
                )
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx custom GPU mesh 3D renderer is unavailable")
            }
        }
    }
}
