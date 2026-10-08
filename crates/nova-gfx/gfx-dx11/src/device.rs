use crate::{
    pipelines::{Pipeline, Shader},
    registry::Registry,
    resources::{Buffer, ResourceSet, Texture, View},
    surface::Swapchain,
};
use gfx_core::*;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{Direct3D::*, Direct3D11::*, Dxgi::*},
    },
    core::Interface,
};

static NEXT_DEVICE: AtomicU32 = AtomicU32::new(1);

/// D3D11 device and immediate context, accessed only by its graphics owner.
pub struct Dx11Device {
    pub(crate) native: ID3D11Device,
    pub(crate) context: ID3D11DeviceContext1,
    pub(crate) factory: IDXGIFactory2,
    adapter: AdapterInfo,
    pub(crate) buffers: Registry<Buffer>,
    pub(crate) textures: Registry<Texture>,
    pub(crate) views: Registry<View>,
    pub(crate) samplers: Registry<ID3D11SamplerState>,
    pub(crate) layouts: Registry<ResourceSetLayoutDescriptor>,
    pub(crate) sets: Registry<ResourceSet>,
    pub(crate) pipeline_layouts: Registry<PipelineLayoutDescriptor>,
    pub(crate) shaders: Registry<Shader>,
    pub(crate) passes: Registry<RenderPassDescriptor>,
    pub(crate) pipelines: Registry<Pipeline>,
    pub(crate) encoders: Registry<Vec<DrawDescriptor>>,
    pub(crate) surfaces: Registry<windows::Win32::Foundation::HWND>,
    pub(crate) swapchains: Registry<Swapchain>,
    pub(crate) draw_constants: ID3D11Buffer,
    pending: VecDeque<(u32, ID3D11Query)>,
    queries: Vec<ID3D11Query>,
    submission_generation: u32,
    submitted: u32,
    completed: u32,
}

pub(crate) fn backend(error: windows::core::Error) -> Error {
    Error::Backend(format!("D3D11: {error}"))
}
pub(crate) fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| Error::Backend("D3D11 returned no object".into()))
}

impl Dx11Device {
    /// Creates a hardware device supporting feature level 11.0 and D3D11.1 context calls.
    ///
    /// Adapter selection honors the requested name or power preference. Software adapters
    /// are excluded. Shader translation and native shader compilation occur at build time.
    ///
    /// # Errors
    /// Returns unavailable when no matching hardware device supports the required level,
    /// or a backend error for native device/context allocation failures.
    pub fn new(desc: &DeviceDescriptor) -> Result<Self> {
        // SAFETY: COM factory creation has no borrowed pointer arguments.
        let factory: IDXGIFactory2 =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.map_err(backend)?;
        let (native, context, info) = open_adapter(&factory, desc)?;
        let mut constants = None;
        // SAFETY: initialized descriptor, owned output; no initial data is read.
        unsafe {
            native.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )
        }
        .map_err(backend)?;
        log::info!(
            "created native D3D11 device: adapter={} feature_level=11_0",
            info.name
        );
        Ok(Self {
            native,
            context,
            factory,
            adapter: info,
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
            draw_constants: required(constants)?,
            pending: VecDeque::new(),
            queries: Vec::new(),
            submission_generation: NEXT_DEVICE.fetch_add(1, Ordering::Relaxed),
            submitted: 0,
            completed: 0,
        })
    }
    /// Name of the selected native hardware adapter.
    pub fn adapter_name(&self) -> &str {
        &self.adapter.name
    }
    /// Hardware and backend capabilities queried at device creation.
    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.adapter
    }
    /// Whether earlier immediate-context work is still executing.
    /// # Errors
    /// Returns a backend error if native query processing fails.
    pub fn has_pending_gpu_work(&mut self) -> Result<bool> {
        self.retire()?;
        Ok(!self.pending.is_empty())
    }
    /// Retires completed query tracking; live resources remain owned by callers.
    /// # Errors
    /// Returns an error if native completion queries fail.
    pub fn trim_memory(&mut self, _level: MemoryTrimLevel) -> Result<()> {
        self.retire()?;
        self.queries.clear();
        Ok(())
    }
    pub(crate) fn signal(&mut self) -> Result<SubmissionId> {
        self.retire()?;
        let serial = self
            .submitted
            .checked_add(1)
            .ok_or_else(|| Error::Unavailable("D3D11 submission IDs exhausted".into()))?;
        let query = if let Some(query) = self.queries.pop() {
            query
        } else {
            let mut query = None;
            // SAFETY: valid device, event-query descriptor and output pointer.
            unsafe {
                self.native.CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: D3D11_QUERY_EVENT,
                        MiscFlags: 0,
                    },
                    Some(&mut query),
                )
            }
            .map_err(backend)?;
            required(query)?
        };
        // SAFETY: event queries require End only, on this device's immediate context.
        unsafe {
            self.context.End(&query);
            self.context.Flush();
        }
        self.submitted = serial;
        self.pending.push_back((serial, query));
        Ok(SubmissionId::from_parts(serial, self.submission_generation))
    }
    fn retire(&mut self) -> Result<()> {
        while let Some((serial, query)) = self.pending.front() {
            let mut done = 0u32;
            // SAFETY: event result is a BOOL (4 bytes); query and writable result are live.
            unsafe {
                self.context.GetData(
                    query,
                    Some((&raw mut done).cast()),
                    4,
                    D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                )
            }
            .map_err(backend)?;
            if done == 0 {
                break;
            }
            self.completed = *serial;
            if let Some((_, query)) = self.pending.pop_front() {
                self.queries.push(query);
            }
        }
        Ok(())
    }
}

fn open_adapter(
    factory: &IDXGIFactory2,
    desc: &DeviceDescriptor,
) -> Result<(ID3D11Device, ID3D11DeviceContext1, AdapterInfo)> {
    let preferred = factory.cast::<IDXGIFactory6>().ok();
    let preference = match desc.power_preference {
        PowerPreference::LowPower => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
        PowerPreference::HighPerformance => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
    };
    for index in 0.. {
        // SAFETY: enumeration outputs owned adapter references from a live factory.
        let result = unsafe {
            match &preferred {
                Some(factory) => {
                    factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(index, preference)
                }
                None => factory.EnumAdapters1(index),
            }
        };
        let adapter = match result {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(backend(error)),
        };
        // SAFETY: live adapter owns its descriptor.
        let properties = unsafe { adapter.GetDesc1() }.map_err(backend)?;
        if properties.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = properties
            .Description
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(properties.Description.len());
        let name = String::from_utf16_lossy(&properties.Description[..end]);
        if desc
            .adapter_name
            .as_ref()
            .is_some_and(|requested| !requested.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let mut native = None;
        let mut context = None;
        // SAFETY: adapter and output locals live throughout device creation. This is the
        // selected device itself, so adapter probing does not allocate a second device.
        if let Err(error) = unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut native),
                None,
                Some(&mut context),
            )
        } {
            log::debug!("D3D11 adapter {name} unavailable: {error}");
            continue;
        }
        let context = required(context)?.cast().map_err(backend)?;
        return Ok((
            required(native)?,
            context,
            AdapterInfo {
                backend: BackendKind::Dx11,
                name,
                vendor_id: properties.VendorId,
                device_id: properties.DeviceId,
                capabilities: BackendCapabilities {
                    surface: true,
                    cpu_visible_memory: true,
                    gpu_only_memory: true,
                },
            },
        ));
    }
    Err(Error::Unavailable(
        "no matching D3D11 feature-level 11.0 hardware adapter".into(),
    ))
}

/// Enumerates native DXGI hardware adapters for D3D11 selection.
///
/// The native device constructor verifies feature-level support. This inexpensive list
/// describes adapter identity and excludes WARP/software devices.
/// # Errors
/// Returns a backend error if DXGI factory creation or adapter enumeration fails.
pub fn enumerate_adapter_info() -> Result<Vec<AdapterInfo>> {
    // SAFETY: factory and adapters are owned COM references; descriptors are copied.
    let factory: IDXGIFactory2 =
        unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.map_err(backend)?;
    let mut adapters = Vec::new();
    for index in 0.. {
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(backend(error)),
        };
        let properties = unsafe { adapter.GetDesc1() }.map_err(backend)?;
        if properties.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = properties
            .Description
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(properties.Description.len());
        adapters.push(AdapterInfo {
            backend: BackendKind::Dx11,
            name: String::from_utf16_lossy(&properties.Description[..end]),
            vendor_id: properties.VendorId,
            device_id: properties.DeviceId,
            capabilities: BackendCapabilities {
                surface: true,
                cpu_visible_memory: true,
                gpu_only_memory: true,
            },
        });
    }
    Ok(adapters)
}

impl Backend for Dx11Device {
    const BACKEND_KIND: BackendKind = BackendKind::Dx11;
}
impl SubmissionDevice for Dx11Device {
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
        if id.generation() != self.submission_generation
            || id.index() == 0
            || id.index() > self.submitted
        {
            return Err(Error::InvalidInput(
                "unknown or foreign D3D11 submission".into(),
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
impl DiagnosticsDevice for Dx11Device {
    fn resource_stats(&self) -> ResourceStats {
        let allocated_bytes = self
            .buffers
            .values()
            .map(|value| u64::from(value.native_size))
            .sum::<u64>()
            + self
                .textures
                .values()
                .map(|value| value.bytes())
                .sum::<u64>();
        ResourceStats {
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
            allocated_bytes,
            reserved_bytes: allocated_bytes,
        }
    }
}
impl TextureTransferDevice for Dx11Device {
    fn texture_transfer_timestamps_supported(&self) -> bool {
        false
    }
    fn last_texture_transfer_time(&self) -> Option<Duration> {
        None
    }
    fn wait_texture_transfers(&mut self) -> Result<()> {
        let submission = self.signal()?;
        SubmissionDevice::wait_submission(self, submission)
    }
    fn read_texture(&mut self, texture: TextureId) -> Result<TextureReadback> {
        self.readback(texture)
    }
}
