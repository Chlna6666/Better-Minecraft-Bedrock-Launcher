use uuid::Uuid;
use winit::monitor::MonitorHandle;

use crate::{Bounds, DevicePixels, DisplayId, Pixels, PlatformDisplay, logical_point, size};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplaySnapshotKey {
    name: Option<String>,
    position: (i32, i32),
    size: (u32, u32),
    scale_factor_bits: u64,
}

impl DisplaySnapshotKey {
    fn from_monitor_handle(handle: &MonitorHandle) -> Self {
        let position = handle.position();
        let size = handle.size();
        Self {
            name: handle.name(),
            position: (position.x, position.y),
            size: (size.width, size.height),
            scale_factor_bits: handle.scale_factor().to_bits(),
        }
    }

    fn uuid(&self) -> Uuid {
        let mut bytes = Vec::new();
        if let Some(name) = self.name.as_deref() {
            bytes.extend_from_slice(name.as_bytes());
        }
        bytes.push(0);
        bytes.extend_from_slice(&self.position.0.to_le_bytes());
        bytes.extend_from_slice(&self.position.1.to_le_bytes());
        bytes.extend_from_slice(&self.size.0.to_le_bytes());
        bytes.extend_from_slice(&self.size.1.to_le_bytes());
        bytes.extend_from_slice(&self.scale_factor_bits.to_le_bytes());
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, &bytes)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WindowsDisplay {
    pub display_id: DisplayId,
    bounds: Bounds<Pixels>,
    uuid: Uuid,
    key: DisplaySnapshotKey,
}

impl WindowsDisplay {
    pub(crate) fn from_monitor_handle(display_id: DisplayId, monitor: &MonitorHandle) -> Self {
        let key = DisplaySnapshotKey::from_monitor_handle(monitor);
        let position = monitor.position();
        let monitor_size = monitor.size();
        let scale_factor = monitor.scale_factor() as f32;
        let physical_size = size(
            DevicePixels(monitor_size.width as i32),
            DevicePixels(monitor_size.height as i32),
        );

        Self {
            display_id,
            bounds: Bounds {
                origin: logical_point(position.x as f32, position.y as f32, scale_factor),
                size: physical_size.to_pixels(scale_factor),
            },
            uuid: key.uuid(),
            key,
        }
    }

    pub(crate) fn from_window_monitor(window: &winit::window::Window) -> Option<Self> {
        let current_monitor = window.current_monitor()?;
        let current_key = DisplaySnapshotKey::from_monitor_handle(&current_monitor);
        window
            .available_monitors()
            .enumerate()
            .find_map(|(index, monitor)| {
                let display = Self::from_monitor_handle(DisplayId(index as u32), &monitor);
                (display.key == current_key).then_some(display)
            })
    }

    pub(crate) fn matches_monitor(&self, monitor: &MonitorHandle) -> bool {
        self.key == DisplaySnapshotKey::from_monitor_handle(monitor)
    }
}

impl PlatformDisplay for WindowsDisplay {
    fn id(&self) -> DisplayId {
        self.display_id
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}
