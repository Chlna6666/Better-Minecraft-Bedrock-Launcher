//! Native ordered compute. A command buffer owns the entire validated batch;
//! separate encoders let Metal track producer/consumer hazards without CPU waits.
use super::*;
use gfx_core::{ComputeDispatch, ComputeBindGroupDispatch, ShaderStages};
use objc2_metal::{MTLComputeCommandEncoder, MTLComputePipelineState};
use std::rc::Rc;

struct PreparedCompute {
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    group: Rc<NativeBindGroup>,
    sizes: Vec<u32>,
    grid: MTLSize,
    local: MTLSize,
}

impl MetalDevice {
    fn prepare_compute(&mut self, shader_id: ShaderModuleId, workgroups: [u32; 3], group: Rc<NativeBindGroup>) -> Result<PreparedCompute> {
        let shader = self.shader_modules.get(shader_id)?;
        if shader.stage != ShaderStage::Compute { return Err(Error::InvalidInput("Metal compute requires a compute shader".into())); }
        let metadata = shader.bindings.as_ref().ok_or_else(|| Error::Unavailable("Metal compute requires reflected workgroup ABI".into()))?;
        let local = metadata.workgroup_size;
        let threads = local.iter().try_fold(1u32, |total, &dimension| total.checked_mul(dimension))
            .filter(|&total| total > 0).ok_or_else(|| Error::InvalidInput("invalid Metal local workgroup dimensions".into()))?;
        let limits = self.device.maxThreadsPerThreadgroup();
        if local[0] as usize > limits.width || local[1] as usize > limits.height || local[2] as usize > limits.depth {
            return Err(Error::InvalidInput("Metal workgroup exceeds device dimensions".into()));
        }
        let sizes = bindings::size_table(Some(metadata), &group)?;
        let pipeline = if let Some(pipeline) = self.compute_pipelines.get(&shader_id) { pipeline.clone() } else {
            let entry = NSString::from_str(metadata.native_entry.as_ref());
            let function = shader.library.newFunctionWithName(&entry)
                .ok_or_else(|| Error::Shader("Metal compute entry point not found".into()))?;
            let pipeline = self.device.newComputePipelineStateWithFunction_error(&function)
                .map_err(|error| Error::Shader(nserror_message(&error)))?;
            self.compute_pipelines.insert(shader_id, pipeline.clone());
            pipeline
        };
        if threads as usize > pipeline.maxTotalThreadsPerThreadgroup()
            || pipeline.staticThreadgroupMemoryLength() > self.device.maxThreadgroupMemoryLength()
        {
            return Err(Error::InvalidInput("Metal compute pipeline workgroup limits exceeded".into()));
        }
        let size = |v: [u32; 3]| MTLSize { width: v[0] as usize, height: v[1] as usize, depth: v[2] as usize };
        Ok(PreparedCompute { pipeline, group, sizes, grid: size(workgroups), local: size(local) })
    }

    pub(super) fn submit_compute(&mut self, dispatches: &[ComputeDispatch<'_>], tracked: bool) -> Result<Option<SubmissionId>> {
        let caps = self.compute_capabilities();
        let limits = self.buffer_binding_limits();
        let mut prepared = Vec::with_capacity(dispatches.len());
        for dispatch in dispatches {
            if dispatch.workgroups.contains(&0) { continue; }
            dispatch.validate_dispatch(caps, limits)?;
            let shader = self.shader_modules.get(dispatch.shader)?;
            let metadata = shader.bindings.as_ref().ok_or_else(|| Error::Unavailable("Metal compute requires reflected bindings".into()))?;
            let group = Rc::new(self.prepare_compute_buffers(dispatch, metadata)?);
            prepared.push(self.prepare_compute(dispatch.shader, dispatch.workgroups, group)?);
        }
        self.commit_compute(&prepared, tracked)
    }

    pub(super) fn submit_compute_set(&mut self, dispatch: &ComputeBindGroupDispatch) -> Result<()> {
        if dispatch.workgroups.contains(&0) { return Ok(()); }
        dispatch.validate(self.compute_capabilities())?;
        let group = self.bind_groups.get(dispatch.bind_group)?;
        let shader = self.shader_modules.get(dispatch.shader)?;
        self.validate_shader_bindings(Some(group.layout), shader.bindings.as_ref(), ShaderStages::COMPUTE)?;
        let group = group.native.clone();
        let prepared = self.prepare_compute(dispatch.shader, dispatch.workgroups, group)?;
        self.commit_compute(std::slice::from_ref(&prepared), false).map(|_| ())
    }

    fn commit_compute(&mut self, prepared: &[PreparedCompute], tracked: bool) -> Result<Option<SubmissionId>> {
        if prepared.is_empty() { return Ok(None); }
        let commands = objc2::rc::autoreleasepool(|_| -> Result<_> {
            let commands = self.command_queue.commandBuffer()
                .ok_or_else(|| Error::Backend("Metal compute command buffer unavailable".into()))?;
            for dispatch in prepared {
                let encoder = commands.computeCommandEncoder()
                    .ok_or_else(|| Error::Backend("Metal compute encoder unavailable".into()))?;
                encoder.setComputePipelineState(&dispatch.pipeline);
                dispatch.group.encode_compute(&encoder, &dispatch.sizes);
                // SAFETY: both grid and exact baked local dimensions were
                // validated, and native resources are retained by the command buffer.
                unsafe { encoder.dispatchThreadgroups_threadsPerThreadgroup(dispatch.grid, dispatch.local); }
                encoder.endEncoding();
            }
            commands.commit();
            Ok(commands)
        })?;
        Ok(if tracked { Some(self.submissions.insert(MetalSubmission::new(commands, false))) } else { None })
    }
}
