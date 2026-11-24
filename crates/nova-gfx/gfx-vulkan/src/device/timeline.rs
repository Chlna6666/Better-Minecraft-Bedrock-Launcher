//! Primary queue timeline signals are attached to existing submissions.
//! Binary WSI semaphores keep their existing synchronization contract.
use super::{Error, Result, VulkanError, vk};
use std::cell::Cell;

pub(super) struct PrimaryTimeline {
    device: ash::Device,
    semaphore: vk::Semaphore,
    last: Cell<u64>,
}

pub(super) struct TimelinePoint {
    device: ash::Device,
    semaphore: vk::Semaphore,
    value: u64,
}

impl TimelinePoint {
    pub(super) fn wait(self) -> std::result::Result<(), vk::Result> {
        let semaphores = [self.semaphore];
        let values = [self.value];
        let info = vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values);
        // SAFETY: device teardown joins every worker before destroying the semaphore.
        unsafe { self.device.wait_semaphores(&info, u64::MAX) }
    }
}

impl PrimaryTimeline {
    pub(super) fn new(device: &ash::Device) -> Result<Self> {
        let mut kind = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE).initial_value(0);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut kind);
        // SAFETY: Vulkan 1.2 core timelineSemaphore is enabled on this 1.3 device.
        let semaphore = unsafe { device.create_semaphore(&info, None) }.map_err(VulkanError::from)?;
        Ok(Self {
            device: device.clone(),
            semaphore, last: Cell::new(0),
        })
    }

    pub(super) fn last(&self) -> u64 { self.last.get() }

    pub(super) fn wait_point(&self, value: u64) -> TimelinePoint {
        TimelinePoint { device: self.device.clone(), semaphore: self.semaphore, value }
    }

    pub(super) fn submit(
        &self, device: &ash::Device, queue: vk::Queue,
        buffers: &[vk::CommandBuffer], waits: &[vk::Semaphore], signals: &[vk::Semaphore],
        fence: vk::Fence,
    ) -> Result<()> {
        let value = self.last.get().checked_add(1)
            .ok_or_else(|| Error::Unavailable("Vulkan primary timeline exhausted".into()))?;
        super::submit::with_timeline(device, queue, buffers, waits, signals,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, fence, Some((self.semaphore, value)))?;
        self.last.set(value);
        Ok(())
    }

    pub(super) fn destroy(self, device: &ash::Device) {
        // SAFETY: device idle and all host waits joined before destruction.
        unsafe { device.destroy_semaphore(self.semaphore, None) };
    }
}
