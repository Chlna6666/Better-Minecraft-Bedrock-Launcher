//! Ordered buffer compute and GPU-generated draw arguments.

use crate::{BufferBinding, Error, Result, ShaderModuleId};

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
    /// Whether render passes accept GPU-generated non-indexed draw arguments.
    pub indirect_draw: bool,
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

impl ComputeDispatch<'_> {
    /// Validates workgroup counts and binding aliasing against advertised capabilities.
    ///
    /// # Errors
    /// Returns `Unavailable` without compute support, or `InvalidInput` for oversized
    /// workgroups, duplicate slots, or a writable resource aliased through another slot.
    pub fn validate(&self, capabilities: ComputeCapabilities) -> Result<()> {
        if capabilities.max_workgroups.contains(&0) {
            return Err(Error::Unavailable("buffer compute is unsupported".into()));
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
    use crate::BufferId;

    #[test]
    fn dispatch_rejects_aliases_and_oversized_groups() {
        let caps = ComputeCapabilities {
            max_workgroups: [65535; 3],
            max_invocations_per_workgroup: 128,
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
        dispatch.workgroups[0] = 65536;
        assert!(dispatch.validate(caps).is_err());
        assert!(matches!(
            dispatch.validate(ComputeCapabilities::default()),
            Err(Error::Unavailable(_))
        ));
    }
}
