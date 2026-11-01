/// Linear distance fog in scene units. RGB is in the scene shader's output color space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistanceFog {
    pub(super) color: [f32; 3],
    pub(super) start: f32,
    pub(super) end: f32,
}

impl DistanceFog {
    /// Creates fog fading from zero at `start` to full coverage at `end`.
    /// Returns `None` for non-finite values, RGB outside 0..=1 or invalid distances.
    #[must_use]
    pub fn new(color: [f32; 3], start: f32, end: f32) -> Option<Self> {
        (color
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && start.is_finite()
            && end.is_finite()
            && start >= 0.0
            && end > start)
            .then_some(Self { color, start, end })
    }

    pub(super) fn encoded(self) -> [f32; 8] {
        [
            self.color[0],
            self.color[1],
            self.color[2],
            1.0,
            self.start,
            self.end,
            0.0,
            0.0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::DistanceFog;

    #[test]
    fn fog_rejects_invalid_ranges_and_colors() {
        assert!(DistanceFog::new([0.5; 3], 32.0, 128.0).is_some());
        for (start, end) in [(2.0, 1.0), (1.0, 1.0), (-1.0, 4.0), (0.0, f32::INFINITY)] {
            assert!(DistanceFog::new([0.5; 3], start, end).is_none());
        }
        assert!(DistanceFog::new([f32::NAN, 0.0, 0.0], 0.0, 1.0).is_none());
        assert!(DistanceFog::new([1.1; 3], 0.0, 1.0).is_none());
    }

    #[test]
    fn fog_survives_camera_scene_and_sampler_updates() {
        use crate::{Camera, Scene, SceneView, TextureSampling, Vec3};
        use std::sync::Arc;
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 128.0);
        let fog = DistanceFog::new([0.5; 3], 32.0, 64.0);
        let scene = Arc::new(Scene::new());
        let view = SceneView::new(scene.clone(), camera).with_fog(fog);
        let updated = view
            .with_camera(camera)
            .with_sampling(TextureSampling::PixelArt)
            .with_voxel_shader();
        assert_eq!(updated.fog, fog);
        assert!(Arc::ptr_eq(&updated.scene, &scene));
        assert_eq!(updated.with_scene(Arc::new(Scene::new())).fog, fog);
        assert!(updated.with_fog(None).fog.is_none());
    }
}
