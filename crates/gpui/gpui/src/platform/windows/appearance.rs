use windows::UI::{
    Color,
    ViewManagement::{UIColorType, UISettings},
};

use crate::WindowAppearance;

#[inline]
pub(crate) fn system_appearance() -> anyhow::Result<WindowAppearance> {
    let settings = UISettings::new()?;
    let foreground = settings.GetColorValue(UIColorType::Foreground)?;
    if is_light(&foreground) {
        Ok(WindowAppearance::Dark)
    } else {
        Ok(WindowAppearance::Light)
    }
}

#[inline(always)]
fn is_light(color: &Color) -> bool {
    ((5 * u32::from(color.G)) + (2 * u32::from(color.R)) + u32::from(color.B)) > (8 * 128)
}
