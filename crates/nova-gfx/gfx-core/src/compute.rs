//! Ordered buffer compute and GPU-generated draw arguments.

use crate::{
    BindGroupDescriptor, BindGroupLayoutDescriptor, BindingResource, BindingType, BufferBinding,
    Error, Result, ShaderModuleId, ShaderStages,
};

/// Optional buffer-compute capabilities. Defaults advertise no compute support.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ComputeCapabilities {
    /// Maximum workgroup counts accepted by an ordered dispatch; zero means unsupported.
    pub max_workgroups: [u32; 3],
    /// Maximum total invocations in a single shader workgroup.
    ///
    /// This is separate from max_workgroups (dispatch grid dimensions).
    /// Zero means the backend does not expose a compute limit.
    pub max_invocations_per_workgroup: u32,
    /// Maximum local workgroup X, Y and Z sizes. Distinct from dispatch grid counts.
    /// Zero means the backend cannot report the limit.
    pub max_workgroup_size: [u32; 3],
    /// Maximum bytes available to WGSL `var<workgroup>` per workgroup.
    /// Zero means unavailable or unknown.
    pub max_workgroup_shared_memory_bytes: u32,
    /// Whether render passes accept GPU-generated non-indexed draw arguments.
    pub indirect_draw: bool,
}

/// Reported buffer binding limits of the selected native device.
///
/// Like wgpu::Limits, these are *capabilities*, not resource-allocation
/// defaults. Zero means the backend cannot report the relevant limit; callers
/// must not assume an arbitrary alignment or maximum range in that case.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BufferBindingLimits {
    /// Minimum valid byte offset for a storage buffer binding.
    pub storage_offset_alignment: u64,
    /// Maximum addressable byte range of a single storage binding.
    pub max_storage_binding_bytes: u64,
    /// Minimum valid byte offset for a uniform buffer binding.
    pub uniform_offset_alignment: u64,
    /// Maximum addressable byte range of a single uniform binding.
    pub max_uniform_binding_bytes: u64,
}

impl BufferBindingLimits {
    /// Validate a non-empty binding range against the actual device contract.
    /// Buffer allocation length is validated separately by the backend.
    pub fn validate_range(self, offset: u64, size: u64, uniform: bool) -> Result<()> {
        let (alignment, maximum) = if uniform {
            (
                self.uniform_offset_alignment,
                self.max_uniform_binding_bytes,
            )
        } else {
            (
                self.storage_offset_alignment,
                self.max_storage_binding_bytes,
            )
        };
        if alignment == 0 || maximum == 0 {
            return Err(Error::Unavailable(
                "buffer binding limits were not reported".into(),
            ));
        }
        if size == 0
            || size > maximum
            || offset % alignment != 0
            || offset.checked_add(size).is_none()
        {
            return Err(Error::InvalidInput(
                "buffer binding violates device limits".into(),
            ));
        }
        Ok(())
    }

    /// Compute an aligned storage suballocation size without overflow.
    pub fn storage_allocation_size(self, bytes: u64) -> Result<u64> {
        let align = self.storage_offset_alignment;
        if align == 0 || self.max_storage_binding_bytes == 0 {
            return Err(Error::Unavailable(
                "storage binding limits were not reported".into(),
            ));
        }
        let requested = bytes.max(1);
        if requested > self.max_storage_binding_bytes {
            return Err(Error::InvalidInput(
                "storage allocation exceeds device binding limit".into(),
            ));
        }
        requested
            .checked_add(align - 1)
            .map(|value| value / align * align)
            .filter(|value| *value <= self.max_storage_binding_bytes)
            .ok_or_else(|| {
                Error::InvalidInput("aligned storage allocation exceeds device limits".into())
            })
    }
}

/// Backend-neutral selection of the voxel rendering pipeline.
///
/// All four desktop APIs (D3D11, D3D12, Vulkan and OpenGL) use the same
/// decoded-section inputs and the same WGSL programs. Native backends only
/// advertise compute/indirect capabilities, never select visible chunks on CPU.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VoxelExecutionPath {
    /// Compute/indirect support is unavailable; use a lazy CPU geometry fallback.
    #[default]
    CpuFallback,
    /// Compute-generated triangles, GPU frustum/Hi-Z culling, indirect draws.
    GpuDriven,
}

/// Requirements of one *precompiled* compute shader entry point.
///
/// Workgroup size is baked into WGSL. Changing the dispatch grid does not change
/// the entry point's local thread count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeKernelProfile {
    /// Shader-local workgroup dimensions: WGSL `@workgroup_size(x, y, z)`.
    pub workgroup_size: [u32; 3],
    /// Shader-local workgroup shared memory footprint, in bytes.
    pub shared_memory_bytes: u32,
}

impl ComputeKernelProfile {
    /// Construct a simple one-dimensional shader profile with no shared memory.
    #[must_use]
    pub const fn one_dimensional(threads: u32) -> Self {
        Self {
            workgroup_size: [threads, 1, 1],
            shared_memory_bytes: 0,
        }
    }

    /// Return the total number of shader invocations, or zero if invalid.
    #[must_use]
    pub fn invocations(self) -> u32 {
        self.workgroup_size
            .iter()
            .try_fold(1u32, |count, &size| count.checked_mul(size))
            .unwrap_or(0)
    }
}

impl ComputeCapabilities {
    /// Resolve the same geometry execution policy on D3D11, D3D12, Vulkan and
    /// OpenGL, using real device limits instead of choosing by backend name.
    /// The current compute mesher and frustum culler both use 64 threads;
    /// shader variants must remain consistent with this requirement.
    #[must_use]
    pub fn voxel_execution_path(self) -> VoxelExecutionPath {
        if self.indirect_draw && self.supports_kernel(ComputeKernelProfile::one_dimensional(64)) {
            VoxelExecutionPath::GpuDriven
        } else {
            VoxelExecutionPath::CpuFallback
        }
    }

    /// Whether ordered buffer compute is supported by this device.
    #[must_use]
    pub fn supports_compute(self) -> bool {
        !self.max_workgroups.contains(&0)
    }

    /// Check per-axis limits, combined thread count and shared memory.
    ///
    /// Unknown limits are rejected instead of silently assuming that a shader
    /// will compile. This validates *legality*, not GPU occupancy/performance.
    #[must_use]
    pub fn supports_kernel(self, kernel: ComputeKernelProfile) -> bool {
        let threads = kernel.invocations();
        self.supports_compute()
            && threads > 0
            && self.max_invocations_per_workgroup > 0
            && threads <= self.max_invocations_per_workgroup
            && kernel
                .workgroup_size
                .iter()
                .zip(self.max_workgroup_size)
                .all(|(&requested, limit)| requested > 0 && requested <= limit)
            && self.max_workgroup_shared_memory_bytes > 0
            && kernel.shared_memory_bytes <= self.max_workgroup_shared_memory_bytes
    }

    /// Select the nearest legal *compiled* shader variant, returning its index.
    ///
    /// A tie prefers the smaller workgroup. This is a deterministic heuristic,
    /// not a substitute for benchmarking on the actual GPU.
    #[must_use]
    pub fn select_precompiled_kernel(
        self,
        profiles: &[ComputeKernelProfile],
        preferred_threads: u32,
    ) -> Option<usize> {
        profiles
            .iter()
            .enumerate()
            .filter(|(_, profile)| self.supports_kernel(**profile))
            .min_by_key(|(_, profile)| {
                let threads = profile.invocations();
                (threads.abs_diff(preferred_threads), threads)
            })
            .map(|(index, _)| index)
    }

    /// Plan a linear set of logical workgroups on an X/Y grid.
    ///
    /// Shader index: `workgroup_id.x + workgroup_id.y * num_workgroups.x`.
    /// The shader must reject padded indices at the end of the grid.
    /// Zero logical groups returns [0,1,1]; skip dispatch in that case.
    ///
    /// # Errors
    /// Returns an error when compute is unavailable or two dimensions cannot
    /// represent the requested number of groups.
    pub fn plan_linear_dispatch_2d(self, logical_groups: u32) -> Result<[u32; 3]> {
        if !self.supports_compute() {
            return Err(Error::Unavailable("buffer compute is unsupported".into()));
        }
        if logical_groups == 0 {
            return Ok([0, 1, 1]);
        }
        let x = logical_groups.min(self.max_workgroups[0]);
        let y = logical_groups.div_ceil(x);
        if y > self.max_workgroups[1] {
            return Err(Error::InvalidInput(
                "2D compute dispatch exceeds device limits".into(),
            ));
        }
        Ok([x, y, 1])
    }
}

/// Access used by one compute buffer binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeBufferAccess {
    /// Read-only storage, mapped to the shader's storage binding.
    ReadOnly,
    /// Writable storage; the buffer must include `STORAGE_WRITE` usage.
    ReadWrite,
    /// Read-only uniform buffer.
    Uniform,
}

/// A buffer bound to a logical group-zero compute slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeBufferBinding {
    /// Logical shader binding number.
    pub binding: u32,
    /// Buffer range.
    pub buffer: BufferBinding,
    /// Shader access to the range.
    pub access: ComputeBufferAccess,
}

/// A compute dispatch ordered after earlier device work and before subsequent draws.
///
/// This does not wait for completion or read results back to the CPU. Backends must make
/// writes visible to subsequent compute/storage/indirect consumers before returning.
#[derive(Clone, Debug)]
pub struct ComputeDispatch<'a> {
    /// A compute-stage shader module; its entry point is fixed at module creation.
    pub shader: ShaderModuleId,
    /// Group-zero buffer bindings. A resource cannot be writable and simultaneously read
    /// through another slot, even if the ranges do not overlap.
    pub bindings: &'a [ComputeBufferBinding],
    /// Workgroup counts, not invocation counts.
    pub workgroups: [u32; 3],
}

/// Compute dispatch that binds one full resource set instead of individual buffers.
///
/// Use this when the shader needs textures, storage images or samplers; the buffer-only
/// [`ComputeDispatch`] path cannot express those bindings. Backends that cannot bind a
/// compute resource set report [`crate::Error::Unavailable`].
#[derive(Clone, Copy, Debug)]
pub struct ComputeBindGroupDispatch {
    /// A compute-stage shader module; its entry point is fixed at module creation.
    pub shader: crate::ShaderModuleId,
    /// Resource set bound before the dispatch. Its layout must match the shader.
    pub bind_group: crate::BindGroupId,
    /// Workgroup counts, not invocation counts.
    pub workgroups: [u32; 3],
}

impl ComputeBindGroupDispatch {
    /// Maps a validated compute resource set to the existing ordered buffer
    /// dispatch path. Texture/sampler bindings cannot be silently dropped.
    ///
    /// StorageBuffer remains read-only; writable buffers must use ComputeDispatch
    /// until the resource-set layout models read/write access explicitly.
    pub fn buffer_bindings(
        &self,
        set: &BindGroupDescriptor,
        layout: &BindGroupLayoutDescriptor,
    ) -> Result<Vec<ComputeBufferBinding>> {
        set.validate_against(layout)?;
        let mut bindings = Vec::with_capacity(layout.entries.len());
        for entry in &layout.entries {
            if !entry.stages.contains(ShaderStages::COMPUTE) {
                return Err(Error::InvalidInput(format!(
                    "binding {} is not visible to compute shaders",
                    entry.binding
                )));
            }
            let access = match entry.binding_type {
                BindingType::UniformBuffer => ComputeBufferAccess::Uniform,
                BindingType::StorageBuffer => ComputeBufferAccess::ReadOnly,
                BindingType::SampledTexture
                | BindingType::StorageTexture { .. }
                | BindingType::Sampler => {
                    return Err(Error::Unavailable(
                        "compute texture/sampler resource sets require a native implementation"
                            .into(),
                    ));
                }
            };
            let resource = set
                .entries
                .iter()
                .find(|binding| binding.binding == entry.binding)
                .ok_or_else(|| Error::InvalidInput("missing compute binding".into()))?;
            let BindingResource::Buffer(buffer) = resource.resource else {
                return Err(Error::InvalidInput("compute binding is not a buffer".into()));
            };
            bindings.push(ComputeBufferBinding {
                binding: entry.binding,
                buffer,
                access,
            });
        }
        Ok(bindings)
    }

    /// Validates the workgroup counts against a device's compute capabilities.
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidInput`] when a count is zero or the total exceeds a
    /// capability, and [`crate::Error::Unavailable`] when the device cannot dispatch.
    pub fn validate(&self, capabilities: crate::ComputeCapabilities) -> crate::Result<()> {
        crate::ComputeDispatch {
            shader: self.shader,
            bindings: &[],
            workgroups: self.workgroups,
        }
        .validate(capabilities)
    }
}

impl ComputeDispatch<'_> {
    /// Validate one compute submission against both the shader execution
    /// capabilities and native buffer binding limits. Shared across all four
    /// backends, analogous to wgpu's device-limit validation.
    pub fn validate_with_limits(
        &self,
        capabilities: ComputeCapabilities,
        limits: BufferBindingLimits,
    ) -> Result<()> {
        self.validate(capabilities)?;
        for binding in self.bindings {
            limits.validate_range(
                binding.buffer.offset,
                binding.buffer.size,
                binding.access == ComputeBufferAccess::Uniform,
            )?;
        }
        Ok(())
    }

    /// Validates workgroup counts and binding aliasing against advertised capabilities.
    ///
    /// # Errors
    /// Returns `Unavailable` without compute support, or `InvalidInput` for oversized
    /// workgroups, duplicate slots, or a writable resource aliased through another slot.
    pub fn validate(&self, capabilities: ComputeCapabilities) -> Result<()> {
        if capabilities.max_workgroups.contains(&0) {
            return Err(Error::Unavailable("buffer compute is unsupported".into()));
        }
        if self.workgroups.contains(&0) {
            return Err(Error::InvalidInput(
                "compute workgroup counts must be nonzero".into(),
            ));
        }
        if self
            .workgroups
            .iter()
            .zip(capabilities.max_workgroups)
            .any(|(&n, max)| n > max)
        {
            return Err(Error::InvalidInput(
                "compute workgroup count exceeds device limit".into(),
            ));
        }
        for (index, binding) in self.bindings.iter().enumerate() {
            for previous in &self.bindings[..index] {
                if binding.binding == previous.binding {
                    return Err(Error::InvalidInput("duplicate compute binding slot".into()));
                }
                if binding.buffer.buffer == previous.buffer.buffer
                    && (binding.access == ComputeBufferAccess::ReadWrite
                        || previous.access == ComputeBufferAccess::ReadWrite)
                {
                    return Err(Error::InvalidInput(
                        "writable compute buffer aliases another binding".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BindGroupEntry, BindGroupLayoutEntry, BindGroupLayoutId, BufferId, TextureBinding,
        TextureViewId,
    };

    #[test]
    fn compute_bind_group_lowers_buffers_but_rejects_invisible_and_texture_bindings() {
        let mut layout = BindGroupLayoutDescriptor {
            label: None,
            entries: vec![BindGroupLayoutEntry {
                binding: 2,
                binding_type: BindingType::StorageBuffer,
                stages: ShaderStages::COMPUTE,
            }],
        };
        let mut set = BindGroupDescriptor {
            label: None,
            layout: BindGroupLayoutId::new(1),
            entries: vec![BindGroupEntry {
                binding: 2,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: BufferId::new(3), offset: 0, size: 64, stride: None,
                }),
            }],
        };
        let dispatch = ComputeBindGroupDispatch {
            shader: ShaderModuleId::new(4),
            bind_group: crate::BindGroupId::new(5),
            workgroups: [1, 1, 1],
        };
        let converted = dispatch.buffer_bindings(&set, &layout).unwrap();
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].access, ComputeBufferAccess::ReadOnly);
        layout.entries[0].stages = ShaderStages::VERTEX;
        assert!(matches!(dispatch.buffer_bindings(&set, &layout), Err(Error::InvalidInput(_))));
        layout.entries[0].stages = ShaderStages::COMPUTE;
        layout.entries[0].binding_type = BindingType::SampledTexture;
        set.entries[0].resource = BindingResource::Texture(TextureBinding {
            texture_view: TextureViewId::new(6),
        });
        assert!(matches!(dispatch.buffer_bindings(&set, &layout), Err(Error::Unavailable(_))));
    }

    #[test]
    fn device_binding_limits_validate_offsets_and_aligned_allocations() {
        let limits = BufferBindingLimits {
            storage_offset_alignment: 512,
            max_storage_binding_bytes: 2048,
            uniform_offset_alignment: 256,
            max_uniform_binding_bytes: 1024,
        };
        assert!(limits.validate_range(1024, 500, false).is_ok());
        assert!(limits.validate_range(768, 500, false).is_err());
        assert!(limits.validate_range(0, 2049, false).is_err());
        assert!(limits.validate_range(256, 1024, true).is_ok());
        assert!(limits.validate_range(512, 1025, true).is_err());
        assert_eq!(limits.storage_allocation_size(1).unwrap(), 512);
        assert_eq!(limits.storage_allocation_size(700).unwrap(), 1024);
        assert_eq!(limits.storage_allocation_size(2048).unwrap(), 2048);
        assert!(limits.storage_allocation_size(2049).is_err());
        assert!(
            BufferBindingLimits::default()
                .validate_range(0, 4, false)
                .is_err()
        );
    }

    #[test]
    fn compute_dispatch_requires_native_buffer_limits() {
        let caps = mock_caps();
        let limits = BufferBindingLimits {
            storage_offset_alignment: 128,
            max_storage_binding_bytes: 4096,
            uniform_offset_alignment: 256,
            max_uniform_binding_bytes: 1024,
        };
        let binding = ComputeBufferBinding {
            binding: 0,
            buffer: BufferBinding {
                buffer: BufferId::new(2),
                offset: 256,
                size: 1024,
                stride: None,
            },
            access: ComputeBufferAccess::ReadWrite,
        };
        let bindings = [binding];
        let dispatch = ComputeDispatch {
            shader: ShaderModuleId::new(3),
            bindings: &bindings,
            workgroups: [1; 3],
        };
        assert!(dispatch.validate_with_limits(caps, limits).is_ok());
        assert!(
            dispatch
                .validate_with_limits(caps, BufferBindingLimits::default())
                .is_err()
        );
        let bad = [ComputeBufferBinding {
            buffer: BufferBinding {
                offset: 192,
                ..binding.buffer
            },
            ..binding
        }];
        let dispatch = ComputeDispatch {
            bindings: &bad,
            ..dispatch
        };
        assert!(dispatch.validate_with_limits(caps, limits).is_err());
    }

    #[test]
    fn dispatch_rejects_aliases_and_oversized_groups() {
        let caps = ComputeCapabilities {
            max_workgroups: [65535; 3],
            max_invocations_per_workgroup: 128,
            max_workgroup_size: [128, 128, 64],
            max_workgroup_shared_memory_bytes: 32768,
            indirect_draw: true,
        };
        let binding = ComputeBufferBinding {
            binding: 0,
            buffer: BufferBinding {
                buffer: BufferId::new(1),
                offset: 0,
                size: 16,
                stride: None,
            },
            access: ComputeBufferAccess::ReadWrite,
        };
        let aliases = [
            binding,
            ComputeBufferBinding {
                binding: 1,
                ..binding
            },
        ];
        let mut dispatch = ComputeDispatch {
            shader: ShaderModuleId::new(1),
            bindings: &aliases,
            workgroups: [1; 3],
        };
        assert!(dispatch.validate(caps).is_err());
        dispatch.bindings = &aliases[..1];
        assert!(dispatch.validate(caps).is_ok());
        dispatch.workgroups = [1, 0, 1];
        assert!(matches!(dispatch.validate(caps), Err(Error::InvalidInput(_))));
        dispatch.workgroups = [1, 1, 1];
        dispatch.workgroups[0] = 65536;
        assert!(dispatch.validate(caps).is_err());
        assert!(matches!(
            dispatch.validate(ComputeCapabilities::default()),
            Err(Error::Unavailable(_))
        ));
    }
    fn mock_caps() -> ComputeCapabilities {
        ComputeCapabilities {
            max_workgroups: [7, 4, 1],
            max_invocations_per_workgroup: 128,
            max_workgroup_size: [128, 8, 4],
            max_workgroup_shared_memory_bytes: 16 * 1024,
            indirect_draw: false,
        }
    }

    #[test]
    fn compiled_kernel_limits_are_independent_of_dispatch_grid() {
        let caps = mock_caps();
        assert!(caps.supports_kernel(ComputeKernelProfile {
            workgroup_size: [32, 4, 1],
            shared_memory_bytes: 1024,
        }));
        for kernel in [
            ComputeKernelProfile::one_dimensional(0),
            ComputeKernelProfile::one_dimensional(129),
            ComputeKernelProfile {
                workgroup_size: [32, 8, 1],
                shared_memory_bytes: 0,
            },
            ComputeKernelProfile {
                workgroup_size: [32, 4, 1],
                shared_memory_bytes: 20000,
            },
        ] {
            assert!(!caps.supports_kernel(kernel));
        }
        assert!(
            !ComputeCapabilities::default()
                .supports_kernel(ComputeKernelProfile::one_dimensional(32))
        );
    }

    #[test]
    fn precompiled_shader_selection_falls_back_without_exceeding_limits() {
        let variants = [32, 64, 128, 256].map(ComputeKernelProfile::one_dimensional);
        let caps = mock_caps();
        assert_eq!(caps.select_precompiled_kernel(&variants, 256), Some(2));
        assert_eq!(caps.select_precompiled_kernel(&variants, 48), Some(0));
        assert_eq!(caps.select_precompiled_kernel(&variants, 96), Some(1));
        assert_eq!(
            ComputeCapabilities::default().select_precompiled_kernel(&variants, 64),
            None
        );
    }

    #[test]
    fn linear_dispatch_plan_respects_y_limit_and_padding() {
        let caps = mock_caps();
        for (count, expected) in [
            (0, [0, 1, 1]),
            (7, [7, 1, 1]),
            (20, [7, 3, 1]),
            (28, [7, 4, 1]),
        ] {
            assert_eq!(caps.plan_linear_dispatch_2d(count).unwrap(), expected);
        }
        assert!(caps.plan_linear_dispatch_2d(29).is_err());
        assert!(
            ComputeCapabilities::default()
                .plan_linear_dispatch_2d(1)
                .is_err()
        );
    }

    #[test]
    fn voxel_execution_selects_gpu_by_capabilities_not_api_name() {
        let capable = ComputeCapabilities {
            max_workgroups: [65535; 3],
            max_invocations_per_workgroup: 1024,
            max_workgroup_size: [1024, 1024, 64],
            max_workgroup_shared_memory_bytes: 32 * 1024,
            indirect_draw: true,
        };
        // This can represent any supported backend, including D3D11 and
        // OpenGL 4.3+, without a platform-specific branch.
        assert_eq!(
            capable.voxel_execution_path(),
            VoxelExecutionPath::GpuDriven
        );
        assert_eq!(
            ComputeCapabilities {
                indirect_draw: false,
                ..capable
            }
            .voxel_execution_path(),
            VoxelExecutionPath::CpuFallback,
        );
        assert_eq!(
            ComputeCapabilities {
                max_invocations_per_workgroup: 32,
                ..capable
            }
            .voxel_execution_path(),
            VoxelExecutionPath::CpuFallback,
        );
        assert_eq!(
            ComputeCapabilities::default().voxel_execution_path(),
            VoxelExecutionPath::CpuFallback,
        );
    }
}
