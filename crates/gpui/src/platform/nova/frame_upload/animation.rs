#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(in crate::platform::nova) enum AnimatedPrimitiveKind {
    Quad = 0,
    Shadow = 1,
    MonochromeSprite = 2,
    PolychromeSprite = 3,
    /// Shared filtered-composite buffer kind. Root backdrop and retained element blur composites
    /// both use the same 136-byte GPU record; their Primitive variant determines filter semantics.
    BackdropBlur = 4,
}

impl AnimatedPrimitiveKind {
    #[inline]
    pub(in crate::platform::nova) const fn stride(self) -> usize {
        match self {
            Self::Quad => super::PACKED_QUAD_BYTES,
            Self::Shadow => super::PACKED_SHADOW_BYTES,
            Self::MonochromeSprite => super::PACKED_MONO_SPRITE_BYTES,
            Self::PolychromeSprite => super::PACKED_POLY_SPRITE_BYTES,
            Self::BackdropBlur => super::PACKED_BACKDROP_BLUR_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(in crate::platform::nova) enum AnimationProperty {
    Opacity = 0,
    Transform = 1,
    Translation = 2,
    Scale = 3,
    Rotation = 4,
    SolidColor = 5,
    BlurRadius = 6,
    Shadow = 7,
}

impl AnimationProperty {
    pub(in crate::platform::nova) fn from_transition_property(
        property: crate::TransitionProperty,
    ) -> Option<Self> {
        match property {
            crate::TransitionProperty::Opacity => Some(Self::Opacity),
            crate::TransitionProperty::Transform => Some(Self::Transform),
            crate::TransitionProperty::Translation => Some(Self::Translation),
            crate::TransitionProperty::Scale => Some(Self::Scale),
            crate::TransitionProperty::Rotation => Some(Self::Rotation),
            crate::TransitionProperty::Color => Some(Self::SolidColor),
            crate::TransitionProperty::Blur => Some(Self::BlurRadius),
            crate::TransitionProperty::Shadow => Some(Self::Shadow),
            crate::TransitionProperty::Width
            | crate::TransitionProperty::Height
            | crate::TransitionProperty::Inset
            | crate::TransitionProperty::Margin
            | crate::TransitionProperty::Padding
            | crate::TransitionProperty::Gap
            | crate::TransitionProperty::BorderWidth => None,
        }
    }
}
