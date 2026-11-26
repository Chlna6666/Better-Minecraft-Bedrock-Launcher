use anyhow::{Context as _, Result};
use windows::Win32::{
    Foundation::{COLORREF, HWND, RECT},
    Graphics::Gdi::{
        CreateSolidBrush, DeleteObject, FillRect, GetDC, HBRUSH, HDC, HGDIOBJ, ReleaseDC,
    },
    UI::WindowsAndMessaging::GetClientRect,
};

use crate::{Rgba, WindowBackgroundAppearance};

pub(super) struct StartupBackground(HBRUSH);

impl StartupBackground {
    pub(super) fn prepare(
        hwnd: HWND,
        appearance: WindowBackgroundAppearance,
        color: Option<Rgba>,
    ) -> Result<Option<Self>> {
        let Some(color) = color.filter(|color| supports_background(appearance, *color)) else {
            return Ok(None);
        };
        // COLORREF stores RGB in the low bytes, unlike GPUI's RRGGBBAA representation.
        let [red, green, blue, _] = u32::from(color).to_be_bytes();
        let color = COLORREF(u32::from_le_bytes([red, green, blue, 0]));
        // SAFETY: The returned brush is owned by this native-window helper until Drop.
        let brush = unsafe { CreateSolidBrush(color) };
        anyhow::ensure!(
            !brush.is_invalid(),
            "creating Windows startup background brush"
        );
        let background = Self(brush);
        // SAFETY: This live HWND is owned by the native event loop. The DC is released below.
        let dc = unsafe { GetDC(Some(hwnd)) };
        anyhow::ensure!(!dc.is_invalid(), "acquiring Windows startup background DC");
        let result = background.paint(hwnd, dc);
        // SAFETY: Release exactly the DC acquired above for this HWND.
        if unsafe { ReleaseDC(Some(hwnd), dc) } == 0 {
            log::warn!("failed to release Windows startup background DC");
        }
        result?;
        Ok(Some(background))
    }

    pub(super) fn paint(&self, hwnd: HWND, dc: HDC) -> Result<()> {
        let mut rect = RECT::default();
        // SAFETY: The HWND and DC are live on the native thread; rect is a stack output and the
        // brush stays owned by self throughout this synchronous client-area fill.
        unsafe { GetClientRect(hwnd, &mut rect) }.context("reading startup client rectangle")?;
        anyhow::ensure!(
            unsafe { FillRect(dc, &rect, self.0) } != 0,
            "painting Windows startup background"
        );
        Ok(())
    }
}

impl Drop for StartupBackground {
    fn drop(&mut self) {
        // SAFETY: This helper uniquely owns the brush and never leaves it selected into a DC.
        if !unsafe { DeleteObject(HGDIOBJ(self.0.0)) }.as_bool() {
            log::warn!("failed to release Windows startup background brush");
        }
    }
}

fn supports_background(appearance: WindowBackgroundAppearance, color: Rgba) -> bool {
    appearance == WindowBackgroundAppearance::Opaque && color.a == 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rgb, rgba};

    #[test]
    fn only_opaque_surfaces_and_colors_allow_early_background() {
        assert!(supports_background(
            WindowBackgroundAppearance::Opaque,
            rgb(0x101214)
        ));
        assert!(!supports_background(
            WindowBackgroundAppearance::Transparent,
            rgb(0x101214)
        ));
        assert!(!supports_background(
            WindowBackgroundAppearance::Opaque,
            rgba(0x10121480)
        ));
    }
}
