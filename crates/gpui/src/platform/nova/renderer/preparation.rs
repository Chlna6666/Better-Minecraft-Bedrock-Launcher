use super::*;

impl NovaRenderer {
    /// Prepares the shared device and BGRA renderer core on their GPU owner without a window.
    ///
    /// # Errors
    ///
    /// Returns device or pipeline initialization errors. Only successful cache entries are
    /// retained, so regular window initialization can retry an unfinished preparation.
    #[cfg(target_os = "windows")]
    pub(crate) fn prepare_renderer(options: &RendererOptions) -> Result<()> {
        if options.backend == RendererBackend::NovaOpenGl {
            return Ok(());
        }
        let started_at = Instant::now();
        let backend = shared_device(options.backend, options)?;
        let device_elapsed = started_at.elapsed();
        let mut backend = lock_backend(&backend);
        // Core pipelines use dynamic viewport/scissor; only the attachment format is keyed.
        // Window-sized textures and buffers are deliberately left to window initialization.
        let config = SurfaceConfig::new(1, 1, Format::Bgra8Unorm)?;
        let key = DeviceKey {
            backend: options.backend,
            adapter_name: options.adapter_name.clone(),
            power_preference: nova_power_preference(options),
            pipeline_cache_dir: options.pipeline_cache_dir.clone(),
            native_display: None,
        };
        let core_started_at = Instant::now();
        match &mut *backend {
            #[cfg(feature = "nova-gfx-dx11")]
            NovaBackend::Dx11(device) => {
                shared_renderer_core(key, config.format, || {
                    create_renderer_core(
                        device,
                        config,
                        "gpui nova dx11",
                        cached_nova_dx11_shader_binaries()?,
                    )
                })?;
            }
            #[cfg(feature = "nova-gfx-dx12")]
            NovaBackend::Dx12(device) => {
                shared_renderer_core(key, config.format, || {
                    create_renderer_core(
                        device,
                        config,
                        "gpui nova dx12",
                        cached_nova_dx12_shader_binaries()?,
                    )
                })?;
            }
            #[cfg(feature = "nova-gfx-vulkan")]
            NovaBackend::Vulkan(device) => {
                shared_renderer_core(key, config.format, || {
                    create_renderer_core(
                        device,
                        config,
                        "gpui nova vulkan",
                        cached_nova_vulkan_shader_binaries()?,
                    )
                })?;
            }
            _ => anyhow::bail!("{} cannot prepare a Windows renderer", options.backend),
        }
        log::info!(
            "GPUI renderer preparation: backend={} total_ms={} device_ms={} core_ms={}",
            options.backend,
            started_at.elapsed().as_millis(),
            device_elapsed.as_millis(),
            core_started_at.elapsed().as_millis(),
        );
        Ok(())
    }
}
