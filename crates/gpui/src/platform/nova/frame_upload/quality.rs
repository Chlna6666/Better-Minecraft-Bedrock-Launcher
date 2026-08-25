use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::nova) enum BackdropBlurQuality {
    #[default]
    Full,
    Disabled,
}

impl BackdropBlurQuality {
    pub(in crate::platform::nova) fn adjusted_blur<'a>(
        self,
        blur: &'a crate::PaintBackdropBlur,
    ) -> Option<Cow<'a, crate::PaintBackdropBlur>> {
        match self {
            Self::Full => Some(Cow::Borrowed(blur)),
            Self::Disabled => None,
        }
    }
}
