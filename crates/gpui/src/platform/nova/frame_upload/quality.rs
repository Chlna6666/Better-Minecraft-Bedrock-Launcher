use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::nova) enum BackdropBlurQuality {
    #[default]
    Full,
    /// Keep the established target layout during live resize.
    ///
    /// BMCBL already controls blur downsampling and pass count at the style layer. Mutating those
    /// values here changes the renderer target descriptor and forces the full blur texture chain to
    /// be destroyed and recreated around every resize boundary, which is especially expensive for
    /// Vulkan where descriptor/image-view retirement can synchronize with outstanding GPU work.
    Interactive,
    Disabled,
}

impl BackdropBlurQuality {
    pub(in crate::platform::nova) fn adjusted_blur<'a>(
        self,
        blur: &'a crate::PaintBackdropBlur,
    ) -> Option<Cow<'a, crate::PaintBackdropBlur>> {
        match self {
            Self::Full | Self::Interactive => Some(Cow::Borrowed(blur)),
            Self::Disabled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_blur() -> crate::PaintBackdropBlur {
        crate::PaintBackdropBlur {
            order: Default::default(),
            animation_id: None,
            bounds: Default::default(),
            content_mask: Default::default(),
            corner_radii: Default::default(),
            radius: ScaledPixels(6.0),
            downsample: 2,
            levels: 3,
            saturation: 1.0,
            opacity: 1.0,
            tint: None,
            recompute_overlap: false,
        }
    }

    #[test]
    fn interactive_quality_keeps_target_geometry_stable() {
        let blur = sample_blur();
        let adjusted = BackdropBlurQuality::Interactive
            .adjusted_blur(&blur)
            .expect("interactive quality should keep blur");
        assert_eq!(adjusted.radius, blur.radius);
        assert_eq!(adjusted.downsample, blur.downsample);
        assert_eq!(adjusted.levels, blur.levels);
    }
}
