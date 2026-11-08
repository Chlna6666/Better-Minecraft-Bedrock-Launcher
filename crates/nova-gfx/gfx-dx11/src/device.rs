use crate::{
    commands::indirect::IndirectConstants,
    pipelines::{Pipeline, Shader},
    registry::Registry,
    resources::{BindGroup, Buffer, Texture, View},
    surface::Swapchain,
};
use gfx_core::*;
use std::{
    cell::Cell,
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
    pub(crate) layouts: Registry<BindGroupLayoutDescriptor>,
    pub(crate) sets: Registry<BindGroup>,
    pub(crate) pipeline_layouts: Registry<PipelineLayoutDescriptor>,
    pub(crate) shaders: Registry<Shader>,
    pub(crate) passes: Registry<RenderPassDescriptor>,
    pub(crate) pipelines: Registry<Pipeline>,
    pub(crate) encoders: Registry<Vec<DrawDescriptor>>,
    pub(crate) surfaces: Registry<windows::Win32::Foundation::HWND>,
    pub(crate) swapchains: Registry<Swapchain>,
    pub(crate) draw_constants: ID3D11Buffer,
    pub(crate) indirect_constants: IndirectConstants,
    // Exact SRV register masks for the current immediate context. Explicitly unbind
    // only registers that were populated instead of clearing all 128 on each blur pass.
    pub(crate) vs_srv_slots: Cell<u128>,
    pub(crate) ps_srv_slots: Cell<u128>,
    pending: VecDeque<(u32, ID3D11Query)>,
    queries: Vec<ID3D11Query>,
    frame_timestamp_queries: Vec<Dx11TimestampQueries>,
    pending_frame_timestamps: Vec<(u32, Dx11TimestampQueries)>,
    /// Probes opened by `present_steps` and consumed by the next `signal`.
    next_frame_timestamps: Option<Dx11TimestampQueries>,
    last_frame_gpu_timestamps: Option<FrameGpuTimestamps>,
    timestamp_queries_supported: bool,
    submission_generation: u32,
    submitted: u32,
    completed: u32,
}

struct Dx11TimestampQueries {
    disjoint: ID3D11Query,
    start: ID3D11Query,
    end: ID3D11Query,
}

impl Dx11TimestampQueries {
    fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            disjoint: create_dx11_query(device, D3D11_QUERY_TIMESTAMP_DISJOINT)?,
            start: create_dx11_query(device, D3D11_QUERY_TIMESTAMP)?,
            end: create_dx11_query(device, D3D11_QUERY_TIMESTAMP)?,
        })
    }
}

fn create_dx11_query(device: &ID3D11Device, query: D3D11_QUERY) -> Result<ID3D11Query> {
    let mut native = None;
    // SAFETY: The descriptor is initialized, the output slot is valid, and the
    // device is live.
    unsafe {
        device.CreateQuery(
            &D3D11_QUERY_DESC {
                Query: query,
                MiscFlags: 0,
            },
            Some(&mut native),
        )
    }
    .map_err(backend)?;
    required(native)
}

fn read_dx11_frame_timestamps(
    context: &ID3D11DeviceContext1,
    queries: &Dx11TimestampQueries,
) -> Option<FrameGpuTimestamps> {
    let mut disjoint = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
    let mut start = 0u64;
    let mut end = 0u64;
    // SAFETY: All three queries completed before their submission event query;
    // DONOTFLUSH keeps this a non-blocking owner-thread read.
    unsafe {
        context
            .GetData(
                &queries.disjoint,
                Some((&raw mut disjoint).cast()),
                u32::try_from(std::mem::size_of_val(&disjoint)).ok()?,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            )
            .ok()?;
        context
            .GetData(
                &queries.start,
                Some((&raw mut start).cast()),
                u32::try_from(std::mem::size_of::<u64>()).ok()?,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            )
            .ok()?;
        context
            .GetData(
                &queries.end,
                Some((&raw mut end).cast()),
                u32::try_from(std::mem::size_of::<u64>()).ok()?,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            )
            .ok()?;
    }
    if disjoint.Disjoint.as_bool() || disjoint.Frequency == 0 {
        return None;
    }
    Some(FrameGpuTimestamps {
        duration_ticks: end.saturating_sub(start),
        frequency: disjoint.Frequency,
    })
}

pub(crate) fn backend(error: windows::core::Error) -> Error {
    Error::Backend(format!("D3D11: {error}"))
}
pub(crate) fn backend_with_device_reason(
    device: &ID3D11Device,
    operation: &str,
    error: windows::core::Error,
) -> Error {
    // SAFETY: The device is a live COM object owned by this backend.
    match unsafe { device.GetDeviceRemovedReason() } {
        Ok(()) => Error::Backend(format!(
            "D3D11 {operation}: {error}; device_removed_reason=S_OK"
        )),
        // A failed removal query means the device is gone; every resource and
        // submission is invalid and the caller must recreate the device.
        Err(reason) => Error::DeviceLost(format!(
            "D3D11 {operation}: {error}; device_removed_reason={reason}"
        )),
    }
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
        let indirect_constants = IndirectConstants::new(&native);
        let (frame_timestamp_queries, timestamp_queries_supported) =
            match Dx11TimestampQueries::new(&native) {
                Ok(queries) => (vec![queries], true),
                Err(error) => {
                    log::warn!("nova-gfx DX11 frame timestamps unavailable: {error}");
                    (Vec::new(), false)
                }
            };
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
            indirect_constants,
            vs_srv_slots: Cell::new(0),
            ps_srv_slots: Cell::new(0),
            pending: VecDeque::new(),
            queries: Vec::new(),
            frame_timestamp_queries,
            pending_frame_timestamps: Vec::new(),
            next_frame_timestamps: None,
            last_frame_gpu_timestamps: None,
            timestamp_queries_supported,
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
    /// Retires completed queries and releases idle CPU tracking capacity under memory pressure.
    ///
    /// Light pressure preserves caches. Moderate pressure trims command storage and keeps one query;
    /// aggressive pressure also compacts registries without changing live resource IDs.
    /// Native resource placement and video-memory fragmentation are managed by the D3D11 driver.
    /// # Errors
    /// Returns an error if native completion queries fail.
    pub fn trim_memory(&mut self, level: MemoryTrimLevel) -> Result<()> {
        self.retire()?;
        if level == MemoryTrimLevel::Light {
            return Ok(());
        }
        self.indirect_constants.trim(level);
        self.queries
            .truncate(usize::from(level == MemoryTrimLevel::Moderate));
        for commands in self.encoders.values_mut() {
            commands.shrink_to_fit();
        }
        if level == MemoryTrimLevel::Aggressive {
            self.trim_registries();
            self.pending.shrink_to_fit();
            self.queries.shrink_to_fit();
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
    /// Opens the timestamp-disjoint scope and start timestamp for the frame
    /// about to be recorded by `present_steps`. The matching end timestamps are
    /// closed by [`Self::end_frame_timestamps`] and attached to the next
    /// `signal` submission.
    pub(crate) fn begin_frame_timestamps(&mut self) {
        if self.next_frame_timestamps.is_some() {
            // A previous frame failed before signaling; drop its unrecycled
            // probes instead of attaching them to this frame.
            self.next_frame_timestamps = None;
        }
        if !self.timestamp_queries_supported {
            return;
        }
        let queries = match self.frame_timestamp_queries.pop() {
            Some(queries) => queries,
            None => match Dx11TimestampQueries::new(&self.native) {
                Ok(queries) => queries,
                Err(error) => {
                    log::warn!("nova-gfx DX11 frame timestamps disabled: {error}");
                    self.timestamp_queries_supported = false;
                    return;
                }
            },
        };
        // SAFETY: The immediate context is owned by this device; one disjoint
        // scope is open at a time and the start timestamp belongs to it.
        unsafe {
            self.context.Begin(&queries.disjoint);
            self.context.End(&queries.start);
        }
        self.next_frame_timestamps = Some(queries);
    }

    /// Closes the frame timestamp queries opened by [`Self::begin_frame_timestamps`].
    pub(crate) fn end_frame_timestamps(&mut self) {
        let Some(queries) = self.next_frame_timestamps.as_ref() else {
            return;
        };
        // SAFETY: These calls close the disjoint scope opened for this frame.
        unsafe {
            self.context.End(&queries.end);
            self.context.End(&queries.disjoint);
        }
    }

    /// Drops the probes of a frame that failed before submission.
    pub(crate) fn discard_frame_timestamps(&mut self) {
        self.next_frame_timestamps = None;
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
        if let Some(timestamps) = self.next_frame_timestamps.take() {
            self.pending_frame_timestamps.push((serial, timestamps));
        }
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
            .map_err(|error| backend_with_device_reason(&self.native, "GetData", error))?;
            if done == 0 {
                break;
            }
            self.completed = *serial;
            if let Some((_, query)) = self.pending.pop_front() {
                self.queries.push(query);
            }
        }
        self.read_completed_frame_timestamps();
        Ok(())
    }

    fn read_completed_frame_timestamps(&mut self) {
        let completed = self.completed;
        let mut index = 0;
        while index < self.pending_frame_timestamps.len() {
            if self.pending_frame_timestamps[index].0 <= completed {
                let (_, queries) = self.pending_frame_timestamps.remove(index);
                if let Some(sample) = read_dx11_frame_timestamps(&self.context, &queries) {
                    self.last_frame_gpu_timestamps = Some(sample);
                }
                self.frame_timestamp_queries.push(queries);
            } else {
                index += 1;
            }
        }
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

    fn adapter_name(&self) -> &str {
        &self.adapter.name
    }
}
impl SubmissionDevice for Dx11Device {
    fn wait_queue_idle(&mut self) -> Result<()> {
        let submission = self.signal()?;
        SubmissionDevice::wait_submission(self, submission)
    }

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
            memory_accounting: MemoryAccounting::ResourceSizes,
            cpu_shadow_bytes: self.buffers.values().map(Buffer::cpu_shadow_bytes).sum(),
            buffers: self.buffers.len(),
            textures: self.textures.len(),
            texture_views: self.views.len(),
            samplers: self.samplers.len(),
            bind_group_layouts: self.layouts.len(),
            bind_groups: self.sets.len(),
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
            ..ResourceStats::default()
        }
    }

    fn memory_budget(&self) -> Result<Option<DeviceMemoryBudget>> {
        let Ok(device) = self.native.cast::<IDXGIDevice>() else {
            return Ok(None);
        };
        // SAFETY: the adapter belongs to this live device; no GPU wait is performed.
        let adapter = unsafe { device.GetAdapter() }.map_err(backend)?;
        let Ok(adapter) = adapter.cast::<IDXGIAdapter3>() else {
            return Ok(None);
        };
        let query = |segment| {
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            unsafe { adapter.QueryVideoMemoryInfo(0, segment, &mut info) }.map_err(backend)?;
            Ok(MemoryBudget {
                usage_bytes: info.CurrentUsage,
                budget_bytes: info.Budget,
            })
        };
        Ok(Some(DeviceMemoryBudget {
            local: Some(query(DXGI_MEMORY_SEGMENT_GROUP_LOCAL)?),
            non_local: Some(query(DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)?),
        }))
    }

    fn frame_gpu_timestamps_supported(&self) -> bool {
        self.timestamp_queries_supported
    }

    fn last_frame_gpu_timestamps(&mut self) -> Result<Option<FrameGpuTimestamps>> {
        self.retire()?;
        Ok(self.last_frame_gpu_timestamps)
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

#[cfg(test)]
mod conformance_tests {
    use super::*;

    #[test]
    #[ignore = "requires D3D11 hardware"]
    fn msaa_resolve_roundtrip() {
        let mut device =
            Dx11Device::new(&gfx_core::DeviceDescriptor::default()).expect("hardware device");
        gfx_core::conformance::msaa_resolve_roundtrip(&mut device).expect("MSAA resolve roundtrip");
    }

    #[test]
    #[ignore = "requires GPU hardware"]
    fn format_allocation_roundtrip() {
        let mut device =
            Dx11Device::new(&gfx_core::DeviceDescriptor::default()).expect("hardware device");
        gfx_core::conformance::format_allocation_roundtrip(&mut device)
            .expect("format allocation roundtrip");
    }

    #[test]
    #[ignore = "requires GPU hardware"]
    fn texture_buffer_roundtrip() {
        let mut device =
            Dx11Device::new(&gfx_core::DeviceDescriptor::default()).expect("hardware device");
        gfx_core::conformance::texture_buffer_roundtrip(&mut device)
            .expect("texture buffer roundtrip");
    }
}
