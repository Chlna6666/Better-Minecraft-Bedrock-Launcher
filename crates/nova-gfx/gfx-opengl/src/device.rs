use crate::{
    pipelines::{Pipeline, Shader},
    registry::Registry,
    resources::{Buffer, ResourceSet, Texture, View},
    surface::Swapchain,
};
use gfx_core::*;
use glow::HasContext as _;
use glutin::{
    config::{Config, ConfigSurfaceTypes, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext, Version},
    display::{Display, DisplayApiPreference},
    prelude::*,
    surface::{PbufferSurface, Surface, SurfaceAttributesBuilder},
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::{
    collections::VecDeque,
    ffi::CString,
    num::NonZeroU32,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

static NEXT_DEVICE: AtomicU32 = AtomicU32::new(1);
pub(crate) fn native(error: impl std::fmt::Display) -> Error {
    Error::Backend(format!("OpenGL: {error}"))
}
pub(crate) fn one() -> NonZeroU32 {
    NonZeroU32::MIN
}

pub(crate) fn native_extent(size: Extent2d) -> Result<(i32, i32)> {
    let width = i32::try_from(size.width())
        .map_err(|_| Error::InvalidInput("OpenGL width exceeds signed 32-bit range".into()))?;
    let height = i32::try_from(size.height())
        .map_err(|_| Error::InvalidInput("OpenGL height exceeds signed 32-bit range".into()))?;
    Ok((width, height))
}

/// Desktop GL4.5 context and resources owned by one graphics thread.
/// The native Linux display connection must outlive its device resources. WGL owns
/// its bootstrap window independently of application windows.
pub struct OpenGlDevice {
    pub(crate) gl: glow::Context,
    pub(crate) context: PossiblyCurrentContext,
    pub(crate) parking: Surface<PbufferSurface>,
    pub(crate) display: Display,
    pub(crate) config: Config,
    pub(crate) view_texture: unsafe extern "system" fn(u32, u32, u32, u32, u32, u32, u32, u32),
    adapter: AdapterInfo,
    pub(crate) vao: glow::NativeVertexArray,
    pub(crate) framebuffer: glow::NativeFramebuffer,
    pub(crate) buffers: Registry<Buffer>,
    pub(crate) textures: Registry<Texture>,
    pub(crate) views: Registry<View>,
    pub(crate) samplers: Registry<glow::NativeSampler>,
    pub(crate) layouts: Registry<ResourceSetLayoutDescriptor>,
    pub(crate) sets: Registry<ResourceSet>,
    pub(crate) pipeline_layouts: Registry<PipelineLayoutDescriptor>,
    pub(crate) shaders: Registry<Shader>,
    pub(crate) passes: Registry<RenderPassDescriptor>,
    pub(crate) pipelines: Registry<Pipeline>,
    pub(crate) encoders: Registry<Vec<DrawDescriptor>>,
    pub(crate) surfaces: Registry<RawWindowHandle>,
    pub(crate) swapchains: Registry<Swapchain>,
    pending: VecDeque<(u32, glow::NativeFence)>,
    submitted: u32,
    completed: u32,
    generation: u32,
    pub(crate) uniform_alignment: u64,
    pub(crate) storage_alignment: u64,
    // Last field: the config's native DC must outlive every GL/glutin object.
    #[cfg(target_os = "windows")]
    _bootstrap: crate::bootstrap::Bootstrap,
}

impl OpenGlDevice {
    /// Opens WGL on Windows or EGL on Linux, requesting OpenGL 4.5 core.
    ///
    /// WGL retains a private bootstrap HWND. EGL uses the initial window to select
    /// a compatible configuration; the driver selects its native GPU.
    /// A requested adapter name is checked against the driver's renderer string.
    /// # Errors
    /// Returns unavailable for missing versions, formats or buffer limits, or a native
    /// error for context/display allocation. Unsupported platforms have no device export.
    /// # Safety
    /// Native handles must be valid during initialization. On Linux, the native
    /// display connection must also remain valid until this device is dropped.
    pub unsafe fn new(
        desc: &DeviceDescriptor,
        display_handle: RawDisplayHandle,
        window_handle: RawWindowHandle,
    ) -> Result<Self> {
        #[cfg(target_os = "windows")]
        let bootstrap = crate::bootstrap::Bootstrap::new()?;
        #[cfg(target_os = "windows")]
        let window_handle = {
            if !matches!(window_handle, RawWindowHandle::Win32(_)) {
                return Err(Error::InvalidInput("WGL requires a Win32 window".into()));
            }
            bootstrap.window_handle()?
        };
        #[cfg(windows)]
        let preference = DisplayApiPreference::Wgl(Some(window_handle));
        #[cfg(target_os = "linux")]
        let preference = DisplayApiPreference::Egl;
        // SAFETY: caller retains the native connection and window for this owner device.
        let display = unsafe { Display::new(display_handle, preference) }.map_err(native)?;
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_depth_size(0)
            .with_surface_type(ConfigSurfaceTypes::WINDOW | ConfigSurfaceTypes::PBUFFER)
            .compatible_with_native_window(window_handle)
            .build();
        let config = unsafe { display.find_configs(template) }
            .map_err(native)?
            .min_by_key(|config| config.num_samples())
            .ok_or_else(|| {
                Error::Unavailable("no compatible OpenGL window/pbuffer format".into())
            })?;
        let attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(4, 5))))
            .build(Some(window_handle));
        // SAFETY: these owned objects share one display and a live native window.
        let context = unsafe { display.create_context(&config, &attributes) }.map_err(native)?;
        let parking = unsafe {
            display.create_pbuffer_surface(
                &config,
                &SurfaceAttributesBuilder::<PbufferSurface>::new().build(one(), one()),
            )
        }
        .map_err(native)?;
        let context = context.make_current(&parking).map_err(native)?;
        // SAFETY: loader addresses have the ABI of this current owned desktop context.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                display.get_proc_address(&CString::new(name).expect("GL procedure name"))
            })
        };
        let clip_control = display.get_proc_address(c"glClipControl");
        let texture_view = display.get_proc_address(c"glTextureView");
        if clip_control.is_null() || texture_view.is_null() {
            return Err(Error::Unavailable(
                "OpenGL 4.5 clip-control/texture-view procedures missing".into(),
            ));
        }
        let clip_control: unsafe extern "system" fn(u32, u32) =
            unsafe { std::mem::transmute(clip_control) };
        let view_texture = unsafe {
            std::mem::transmute::<
                *const std::ffi::c_void,
                unsafe extern "system" fn(u32, u32, u32, u32, u32, u32, u32, u32),
            >(texture_view)
        };
        // SAFETY: GL calls use the current context; integer capabilities describe this driver.
        let (name, vao, framebuffer, uniform_alignment, storage_alignment) = unsafe {
            if gl.get_parameter_i32(glow::MAJOR_VERSION) * 10
                + gl.get_parameter_i32(glow::MINOR_VERSION)
                < 45
                || gl.get_parameter_i32(glow::MAX_VERTEX_SHADER_STORAGE_BLOCKS) < 6
                || gl.get_parameter_i32(glow::MAX_FRAGMENT_SHADER_STORAGE_BLOCKS) < 6
            {
                return Err(Error::Unavailable(
                    "OpenGL 4.5 with six vertex/fragment storage blocks required".into(),
                ));
            }
            clip_control(glow::UPPER_LEFT, glow::ZERO_TO_ONE);
            gl.disable(glow::DITHER);
            (
                gl.get_parameter_string(glow::RENDERER),
                gl.create_vertex_array().map_err(native)?,
                gl.create_framebuffer().map_err(native)?,
                gl.get_parameter_i32(glow::UNIFORM_BUFFER_OFFSET_ALIGNMENT) as u64,
                gl.get_parameter_i32(glow::SHADER_STORAGE_BUFFER_OFFSET_ALIGNMENT) as u64,
            )
        };
        if desc
            .adapter_name
            .as_ref()
            .is_some_and(|requested| !name.to_lowercase().contains(&requested.to_lowercase()))
        {
            unsafe {
                gl.delete_vertex_array(vao);
                gl.delete_framebuffer(framebuffer);
            }
            return Err(Error::Unavailable(format!(
                "OpenGL renderer {name} does not match requested adapter"
            )));
        }
        log::info!("created native OpenGL 4.5 device: renderer={name}");
        Ok(Self {
            gl,
            context,
            parking,
            display,
            config,
            view_texture,
            adapter: AdapterInfo {
                backend: BackendKind::OpenGl,
                name,
                vendor_id: 0,
                device_id: 0,
                capabilities: BackendCapabilities {
                    surface: true,
                    cpu_visible_memory: true,
                    gpu_only_memory: true,
                },
            },
            vao,
            framebuffer,
            buffers: Registry::default(),
            textures: Registry::default(),
            views: Registry::default(),
            samplers: Registry::default(),
            layouts: Registry::default(),
            sets: Registry::default(),
            pipeline_layouts: Registry::default(),
            shaders: Registry::default(),
            passes: Registry::default(),
            pipelines: Registry::default(),
            encoders: Registry::default(),
            surfaces: Registry::default(),
            swapchains: Registry::default(),
            pending: VecDeque::new(),
            submitted: 0,
            completed: 0,
            generation: NEXT_DEVICE.fetch_add(1, Ordering::Relaxed),
            uniform_alignment,
            storage_alignment,
            #[cfg(target_os = "windows")]
            _bootstrap: bootstrap,
        })
    }
    /// Renderer string returned by the selected native OpenGL driver.
    pub fn adapter_name(&self) -> &str {
        &self.adapter.name
    }
    /// Driver identity; PCI IDs are unavailable through the native GL interface.
    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.adapter
    }
    pub(crate) fn check(&self) -> Result<()> {
        // SAFETY: this graphics owner keeps its device context current.
        let error = unsafe { self.gl.get_error() };
        if error == glow::NO_ERROR {
            Ok(())
        } else {
            Err(Error::Backend(format!("OpenGL error 0x{error:04x}")))
        }
    }
    pub(crate) fn park(&self) -> Result<()> {
        self.context.make_current(&self.parking).map_err(native)
    }
    pub(crate) fn signal(&mut self) -> Result<SubmissionId> {
        self.retire()?;
        let serial = self
            .submitted
            .checked_add(1)
            .ok_or_else(|| Error::Unavailable("OpenGL submission IDs exhausted".into()))?;
        // SAFETY: fence covers prior commands on the graphics owner's current context.
        let fence =
            unsafe { self.gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) }.map_err(native)?;
        unsafe {
            self.gl.flush();
        }
        self.submitted = serial;
        self.pending.push_back((serial, fence));
        Ok(SubmissionId::from_parts(serial, self.generation))
    }
    fn retire(&mut self) -> Result<()> {
        self.park()?;
        while let Some((serial, fence)) = self.pending.front() {
            // SAFETY: only this owner polls and deletes its pending sync objects.
            let status = unsafe { self.gl.client_wait_sync(*fence, 0, 0) };
            if status == glow::TIMEOUT_EXPIRED {
                break;
            }
            if status == glow::WAIT_FAILED {
                return Err(Error::Backend("OpenGL GPU fence wait failed".into()));
            }
            self.completed = *serial;
            if let Some((_, fence)) = self.pending.pop_front() {
                unsafe {
                    self.gl.delete_sync(fence);
                }
            }
        }
        Ok(())
    }
    /// Whether earlier work remains in flight.
    /// # Errors
    /// Returns native completion errors.
    pub fn has_pending_gpu_work(&mut self) -> Result<bool> {
        self.retire()?;
        Ok(!self.pending.is_empty())
    }
    /// Retires completed fences without releasing application-owned resources.
    ///
    /// Moderate pressure trims command storage. Aggressive pressure also compacts CPU registries
    /// without changing
    /// live resource IDs. Native storage placement and fragmentation remain driver-managed.
    /// # Errors
    /// Returns native completion errors.
    pub fn trim_memory(&mut self, level: MemoryTrimLevel) -> Result<()> {
        self.retire()?;
        if level != MemoryTrimLevel::Light {
            for commands in self.encoders.values_mut() {
                commands.shrink_to_fit();
            }
        }
        if level == MemoryTrimLevel::Aggressive {
            self.trim_registries();
            self.pending.shrink_to_fit();
        }
        Ok(())
    }

    fn trim_registries(&mut self) {
        self.buffers.trim();
        self.textures.trim();
        self.views.trim();
        self.samplers.trim();
        self.layouts.trim();
        self.sets.trim();
        self.pipeline_layouts.trim();
        self.shaders.trim();
        self.passes.trim();
        self.pipelines.trim();
        self.encoders.trim();
        self.surfaces.trim();
        self.swapchains.trim();
    }
}
impl Backend for OpenGlDevice {
    const BACKEND_KIND: BackendKind = BackendKind::OpenGl;
}
impl SubmissionDevice for OpenGlDevice {
    fn async_capabilities(&self) -> AsyncCapabilities {
        AsyncCapabilities {
            threading_mode: ThreadingMode::OwnerThreadOnly,
            async_submission: true,
            async_wait: false,
            async_presentation: true,
        }
    }
    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        self.execute_encoder(encoder)?;
        self.signal()
    }
    fn poll_submission(&mut self, id: SubmissionId) -> Result<SubmissionStatus> {
        if id.generation() != self.generation || id.index() == 0 || id.index() > self.submitted {
            return Err(Error::InvalidInput(
                "unknown or foreign OpenGL submission".into(),
            ));
        }
        self.retire()?;
        Ok(if id.index() <= self.completed {
            SubmissionStatus::Complete
        } else {
            SubmissionStatus::Pending
        })
    }
    fn wait_submission(&mut self, id: SubmissionId) -> Result<()> {
        while SubmissionDevice::poll_submission(self, id)? == SubmissionStatus::Pending {
            std::thread::yield_now();
        }
        Ok(())
    }
}
impl DiagnosticsDevice for OpenGlDevice {
    fn resource_stats(&self) -> ResourceStats {
        let binding_mirror_bytes = self
            .sets
            .values()
            .flat_map(|set| &set.mirrors)
            .map(|mirror| mirror.size)
            .sum::<u64>();
        let bytes = self
            .buffers
            .values()
            .map(|buffer| buffer.native_size as u64)
            .sum::<u64>()
            + self.textures.values().map(Texture::bytes).sum::<u64>()
            + binding_mirror_bytes;
        ResourceStats {
            memory_accounting: MemoryAccounting::ResourceSizes,
            binding_mirror_bytes,
            buffers: self.buffers.len(),
            textures: self.textures.len(),
            texture_views: self.views.len(),
            samplers: self.samplers.len(),
            resource_set_layouts: self.layouts.len(),
            resource_sets: self.sets.len(),
            pipeline_layouts: self.pipeline_layouts.len(),
            shader_modules: self.shaders.len(),
            render_passes: self.passes.len(),
            render_pipelines: self.pipelines.len(),
            command_encoders: self.encoders.len(),
            submissions: self.pending.len(),
            surfaces: self.surfaces.len(),
            swapchains: self.swapchains.len(),
            allocated_bytes: bytes,
            reserved_bytes: bytes,
            ..ResourceStats::default()
        }
    }
}
impl TextureTransferDevice for OpenGlDevice {
    fn texture_transfer_timestamps_supported(&self) -> bool {
        false
    }
    fn last_texture_transfer_time(&self) -> Option<Duration> {
        None
    }
    fn wait_texture_transfers(&mut self) -> Result<()> {
        let id = self.signal()?;
        SubmissionDevice::wait_submission(self, id)
    }
    fn read_texture(&mut self, id: TextureId) -> Result<TextureReadback> {
        self.readback(id)
    }
}
impl Drop for OpenGlDevice {
    fn drop(&mut self) {
        if let Err(error) = self.park() {
            log::warn!("OpenGL cleanup context failed: {error}");
            return;
        }
        // SAFETY: context is current; release each remaining owned GL object once.
        unsafe {
            self.gl.finish();
            for (_, fence) in &self.pending {
                self.gl.delete_sync(*fence);
            }
            for pipeline in self.pipelines.values() {
                self.gl.delete_program(pipeline.native);
            }
            for shader in self.shaders.values() {
                self.gl.delete_shader(shader.native);
            }
            for set in self.sets.values() {
                for buffer in &set.mirrors {
                    self.gl.delete_buffer(buffer.native);
                }
            }
            for sampler in self.samplers.values() {
                self.gl.delete_sampler(*sampler);
            }
            for view in self.views.values() {
                self.gl.delete_texture(view.native);
            }
            for texture in self.textures.values() {
                self.gl.delete_texture(texture.native);
            }
            for buffer in self.buffers.values() {
                self.gl.delete_buffer(buffer.native);
            }
            for chain in self.swapchains.values() {
                self.gl.delete_texture(chain.color);
                self.gl.delete_framebuffer(chain.framebuffer);
            }
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_framebuffer(self.framebuffer);
        }
    }
}
