//! Per-native-queue timeline signals are attached to existing submissions.
//! Binary WSI semaphores keep their existing synchronization contract.
use super::{Arc, Error, Result, VulkanError, vk};
use std::cell::Cell;

pub(super) struct QueueTimeline {
    device: Arc<ash::Device>,
    semaphore: vk::Semaphore,
    last: Cell<u64>,
}

pub(super) struct TimelinePoint {
    device: Arc<ash::Device>,
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

impl QueueTimeline {
    pub(super) fn new(device: &Arc<ash::Device>) -> Result<Self> {
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
        wait_stage: vk::PipelineStageFlags2, fence: vk::Fence,
    ) -> Result<()> {
        let value = self.last.get().checked_add(1)
            .ok_or_else(|| Error::Unavailable("Vulkan queue timeline exhausted".into()))?;
        super::submit::with_timeline(device, queue, buffers, waits, signals,
            wait_stage, fence, Some((self.semaphore, value)))?;
        self.last.set(value);
        Ok(())
    }

    pub(super) fn enqueue_wait(&self, device: &ash::Device, target: vk::Queue, value: u64) -> Result<()> {
        if value == 0 || value > self.last.get() {
            return Err(Error::InvalidInput("Vulkan timeline wait references an unsubmitted value".into()));
        }
        let waits = [vk::SemaphoreSubmitInfo::default().semaphore(self.semaphore)
            .value(value).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let info = vk::SubmitInfo2::default().wait_semaphore_infos(&waits);
        // SAFETY: the producer signal was submitted; this device-owned
        // semaphore remains live until all queues are idle at teardown.
        unsafe { device.queue_submit2(target, &[info], vk::Fence::null()) }
            .map_err(VulkanError::from)?;
        Ok(())
    }

    pub(super) fn destroy(self, device: &ash::Device) {
        // SAFETY: device idle and all host waits joined before destruction.
        unsafe { device.destroy_semaphore(self.semaphore, None) };
    }
}
