//! Core Vulkan 1.3 queue submission; ordinary batches stay on the stack.
use super::{Result, VulkanError, vk};

pub(super) fn binary(
    device: &ash::Device, queue: vk::Queue, buffers: &[vk::CommandBuffer],
    waits: &[vk::Semaphore], signals: &[vk::Semaphore], wait_stage: vk::PipelineStageFlags2,
    fence: vk::Fence,
) -> Result<()> {
    with_timeline(device, queue, buffers, waits, signals, wait_stage, fence, None)
}

pub(super) fn with_timeline(
    device: &ash::Device, queue: vk::Queue, buffers: &[vk::CommandBuffer],
    waits: &[vk::Semaphore], signals: &[vk::Semaphore], wait_stage: vk::PipelineStageFlags2,
    fence: vk::Fence, timeline: Option<(vk::Semaphore, u64)>,
) -> Result<()> {
    with_timeline_dependency(device, queue, buffers, waits, signals, wait_stage, fence, timeline, None)
}

pub(super) fn with_timeline_dependency(
    device: &ash::Device, queue: vk::Queue, buffers: &[vk::CommandBuffer],
    waits: &[vk::Semaphore], signals: &[vk::Semaphore], wait_stage: vk::PipelineStageFlags2,
    fence: vk::Fence, timeline: Option<(vk::Semaphore, u64)>,
    dependency: Option<(vk::Semaphore, u64)>,
) -> Result<()> {
    let mut wait_stack = [vk::SemaphoreSubmitInfo::default(); 4];
    let mut signal_stack = [vk::SemaphoreSubmitInfo::default(); 4];
    let mut buffer_stack = [vk::CommandBufferSubmitInfo::default(); 4];
    let mut wait_heap;
    let mut signal_heap;
    let mut buffer_heap;
    let wait_count = waits.len() + usize::from(dependency.is_some());
    let wait_infos = if wait_count <= wait_stack.len() {
        &mut wait_stack[..wait_count]
    } else {
        wait_heap = vec![vk::SemaphoreSubmitInfo::default(); wait_count];
        wait_heap.as_mut_slice()
    };
    let signal_count = signals.len() + usize::from(timeline.is_some());
    let signal_infos = if signal_count <= signal_stack.len() {
        &mut signal_stack[..signal_count]
    } else {
        signal_heap = vec![vk::SemaphoreSubmitInfo::default(); signal_count];
        signal_heap.as_mut_slice()
    };
    let buffer_infos = if buffers.len() <= buffer_stack.len() {
        &mut buffer_stack[..buffers.len()]
    } else {
        buffer_heap = vec![vk::CommandBufferSubmitInfo::default(); buffers.len()];
        buffer_heap.as_mut_slice()
    };
    for (info, semaphore) in wait_infos.iter_mut().zip(waits) {
        *info = vk::SemaphoreSubmitInfo::default().semaphore(*semaphore).stage_mask(wait_stage);
    }
    if let Some((semaphore, value)) = dependency {
        wait_infos[waits.len()] = vk::SemaphoreSubmitInfo::default()
            .semaphore(semaphore).value(value).stage_mask(wait_stage);
    }
    for (info, semaphore) in signal_infos.iter_mut().zip(signals) {
        *info = vk::SemaphoreSubmitInfo::default().semaphore(*semaphore)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
    }
    if let Some((semaphore, value)) = timeline {
        signal_infos[signals.len()] = vk::SemaphoreSubmitInfo::default()
            .semaphore(semaphore).value(value).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
    }
    for (info, buffer) in buffer_infos.iter_mut().zip(buffers) {
        *info = vk::CommandBufferSubmitInfo::default().command_buffer(*buffer).device_mask(1);
    }
    let submit = vk::SubmitInfo2::default()
        .wait_semaphore_infos(wait_infos).signal_semaphore_infos(signal_infos)
        .command_buffer_infos(buffer_infos);
    // SAFETY: callers own the queues and externally synchronize submissions.
    // ALL_COMMANDS signals preserve the original full completion scope.
    unsafe { device.queue_submit2(queue, &[submit], fence) }.map_err(VulkanError::from)?;
    Ok(())
}
