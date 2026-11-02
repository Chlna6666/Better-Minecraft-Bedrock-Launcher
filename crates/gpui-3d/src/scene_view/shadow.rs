//! Bounded opaque-face sun shadows; shares the conservative occluder source, not its depth test.
use crate::{Camera, Mat4, Occluder, Vec3};
use gfx_core::*;
use gpui::ExtensionDevice;

pub(super) struct Shadow {
    pub(super) raster: super::hiz::HiZ,
    frame: BufferId,
    pub(super) matrix: Mat4,
    key: Option<[i32; 6]>,
}
impl Shadow {
    pub(super) fn invalidate(&mut self) {
        self.key = None;
    }
    pub(super) fn create(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
    ) -> gpui::Result<Self> {
        let raster = super::hiz::HiZ::create_shadow(device, backend)?;
        let frame = match device.create_buffer(&BufferDescriptor {
            label: Some("gpui-3d sun shadow camera".into()),
            size: super::FRAME_PARAMS_STRIDE as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: super::gpu::buffer_memory_location(backend),
        }) {
            Ok(frame) => frame,
            Err(error) => {
                let _ = raster.destroy(device);
                return Err(error.into());
            }
        };
        Ok(Self {
            raster,
            frame,
            matrix: Mat4::IDENTITY,
            key: None,
        })
    }
    pub(super) fn update(
        &mut self,
        device: &mut dyn ExtensionDevice,
        bounds: impl Iterator<Item = Occluder>,
        eye: Vec3,
        sun: Vec3,
        changed: bool,
    ) -> gpui::Result<()> {
        let key = [
            (eye.x / 4.).floor() as i32,
            (eye.y / 4.).floor() as i32,
            (eye.z / 4.).floor() as i32,
            (sun.x * 256.).round() as i32,
            (sun.y * 256.).round() as i32,
            (sun.z * 256.).round() as i32,
        ];
        if !changed && self.key == Some(key) {
            return Ok(());
        }
        let center = Vec3::new(key[0] as f32 * 4., key[1] as f32 * 4., key[2] as f32 * 4.);
        let direction = Vec3::new(key[3] as f32, key[4] as f32, key[5] as f32)
            .normalized()
            .ok_or_else(|| anyhow::anyhow!("invalid sun direction"))?;
        let camera = Camera::orthographic(
            center + direction * 128.,
            center,
            if direction.y.abs() > 0.99 {
                Vec3::Z
            } else {
                Vec3::Y
            },
            96.,
            0.1,
            256.,
        );
        self.matrix = camera.projection_matrix(1.)? * camera.view()?;
        let mut bytes = vec![0; super::FRAME_PARAMS_STRIDE];
        for (dst, value) in bytes[..64]
            .chunks_exact_mut(4)
            .zip(self.matrix.columns().into_iter().flatten())
        {
            dst.copy_from_slice(&value.to_le_bytes());
        }
        for (dst, value) in
            bytes[64..76]
                .chunks_exact_mut(4)
                .zip([camera.eye.x, camera.eye.y, camera.eye.z])
        {
            dst.copy_from_slice(&value.to_le_bytes());
        }
        device.write_buffer(self.frame, 0, &bytes)?;
        self.raster
            .update(device, bounds, center, -direction, true)?;
        self.raster.dispatch(device, self.frame)?;
        self.key = Some(key);
        Ok(())
    }
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        self.raster.destroy(device)?;
        device.destroy_buffer(self.frame)?;
        Ok(())
    }
}
