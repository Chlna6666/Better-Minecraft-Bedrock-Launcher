//! Primary queue timeline signals are attached to existing submissions.
//! Binary WSI semaphores keep their existing synchronization contract.
use super::{Error, Result, VulkanError, Instance, khr, vk};
use std::cell::Cell;

pub(super) struct PrimaryTimeline {
    loader: khr::timeline_semaphore::Device,
    semaphore: vk::Semaphore,
    last: Cell<u64>,
}

pub(super) struct TimelinePoint {
    loader: khr::timeline_semaphore::Device,
    semaphore: vk::Semaphore,
    value: u64,
}

impl TimelinePoint {
    pub(super) fn wait(self) -> std::result::Result<(), vk::Result> {
        let semaphores = [self.semaphore];
        let values = [self.value];
        let info = vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values);
        // SAFETY: device teardown joins every worker before destroying the semaphore.
        unsafe { self.loader.wait_semaphores(&info, u64::MAX) }
    }
}

impl PrimaryTimeline {
    pub(super) fn new(instance: &Instance, device: &ash::Device) -> Result<Self> {
        let mut kind = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE).initial_value(0);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut kind);
        // SAFETY: the extension and feature were enabled on this device.
        let semaphore = unsafe { device.create_semaphore(&info, None) }.map_err(VulkanError::from)?;
        Ok(Self {
            loader: khr::timeline_semaphore::Device::new(instance, device),
            semaphore, last: Cell::new(0),
        })
    }

    pub(super) fn last(&self) -> u64 { self.last.get() }

    pub(super) fn wait_point(&self, value: u64) -> TimelinePoint {
        TimelinePoint { loader: self.loader.clone(), semaphore: self.semaphore, value }
    }

    pub(super) fn submit(
        &self, device: &ash::Device, queue: vk::Queue,
        buffers: &[vk::CommandBuffer], waits: &[vk::Semaphore], signals: &[vk::Semaphore],
        fence: vk::Fence,
    ) -> Result<()> {
        let value = self.last.get().checked_add(1)
            .ok_or_else(|| Error::Unavailable("Vulkan primary timeline exhausted".into()))?;
        // GPUI uses zero or one WSI wait/signal. Keep that common path
        // entirely on the stack; only larger externally supplied lists spill.
        let mut signal_stack = [vk::Semaphore::null(); 4];
        let mut value_stack = [0u64; 4];
        let mut signal_heap;
        let mut value_heap;
        let (signal_semaphores, signal_values) = if signals.len() < signal_stack.len() {
            signal_stack[..signals.len()].copy_from_slice(signals);
            signal_stack[signals.len()] = self.semaphore;
            value_stack[signals.len()] = value;
            (&signal_stack[..signals.len() + 1], &value_stack[..signals.len() + 1])
        } else {
            signal_heap = signals.to_vec();
            signal_heap.push(self.semaphore);
            value_heap = vec![0; signals.len()];
            value_heap.push(value);
            (signal_heap.as_slice(), value_heap.as_slice())
        };
        let wait_stack = [0u64; 4];
        let stage_stack = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT; 4];
        let wait_heap;
        let stage_heap;
        let (wait_values, wait_stages) = if waits.len() <= wait_stack.len() {
            (&wait_stack[..waits.len()], &stage_stack[..waits.len()])
        } else {
            wait_heap = vec![0; waits.len()];
            stage_heap = vec![vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT; waits.len()];
            (wait_heap.as_slice(), stage_heap.as_slice())
        };
        let mut timeline = vk::TimelineSemaphoreSubmitInfo::default()
            .wait_semaphore_values(wait_values).signal_semaphore_values(signal_values);
        let info = vk::SubmitInfo::default()
            .wait_semaphores(waits).wait_dst_stage_mask(wait_stages)
            .command_buffers(buffers).signal_semaphores(signal_semaphores)
            .push_next(&mut timeline);
        // SAFETY: primary queue is externally synchronized by its owner.
        // Binary semaphore values are ignored; all arrays match native counts.
        unsafe { device.queue_submit(queue, &[info], fence) }.map_err(VulkanError::from)?;
        self.last.set(value);
        Ok(())
    }

    pub(super) fn destroy(self, device: &ash::Device) {
        // SAFETY: device idle and all host waits joined before destruction.
        unsafe { device.destroy_semaphore(self.semaphore, None) };
    }
}
