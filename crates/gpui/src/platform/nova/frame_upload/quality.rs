use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::nova) enum BackdropBlurQuality {
    #[default]
    Full,
    /// Preserve the visual blur radius while reducing offscreen bandwidth during live resize.
    Interactive,
    Disabled,
}

impl BackdropBlurQuality {
    pub(in crate::platform::nova) fn adjusted_blur<'a>(
        self,
        blur: &'a crate::PaintBackdropBlur,
    ) -> Option<Cow<'a, crate::PaintBackdropBlur>> {
        match self {
            Self::Full => Some(Cow::Borrowed(blur)),
            Self::Interactive => {
                let mut adjusted = blur.clone();
                adjusted.downsample = adjusted.downsample.saturating_mul(2).clamp(2, 4);
                adjusted.levels = adjusted.levels.clamp(1, 2);
                Some(Cow::Owned(adjusted))
            }
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
    fn interactive_quality_preserves_radius_and_reduces_filter_bandwidth() {
        let blur = sample_blur();
        let adjusted = BackdropBlurQuality::Interactive
            .adjusted_blur(&blur)
            .expect("interactive quality should keep blur");
        assert_eq!(adjusted.radius, blur.radius);
        assert_eq!(adjusted.downsample, 4);
        assert_eq!(adjusted.levels, 2);
    }
}
