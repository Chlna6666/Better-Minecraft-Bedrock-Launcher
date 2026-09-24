use uuid::Uuid;
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromPoint,
    },
};
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
    visible_bounds: Bounds<Pixels>,
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

        let bounds = Bounds {
            origin: logical_point(position.x as f32, position.y as f32, scale_factor),
            size: physical_size.to_pixels(scale_factor),
        };
        let visible_bounds = windows_visible_bounds(position, monitor_size, scale_factor)
            .unwrap_or(bounds);

        Self {
            display_id,
            bounds,
            visible_bounds,
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

    fn visible_bounds(&self) -> Bounds<Pixels> {
        self.visible_bounds
    }
}

fn windows_visible_bounds(
    position: winit::dpi::PhysicalPosition<i32>,
    monitor_size: winit::dpi::PhysicalSize<u32>,
    scale_factor: f32,
) -> Option<Bounds<Pixels>> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }

    let center = POINT {
        x: position
            .x
            .saturating_add(i32::try_from(monitor_size.width / 2).unwrap_or(i32::MAX)),
        y: position
            .y
            .saturating_add(i32::try_from(monitor_size.height / 2).unwrap_or(i32::MAX)),
    };
    let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }

    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe {
        GetMonitorInfoW(
            monitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    };
    if !ok.as_bool() {
        return None;
    }

    let work = info.monitorInfo.rcWork;
    Some(Bounds {
        origin: logical_point(work.left as f32, work.top as f32, scale_factor),
        size: size(
            Pixels((work.right - work.left) as f32 / scale_factor),
            Pixels((work.bottom - work.top) as f32 / scale_factor),
        ),
    })
}
