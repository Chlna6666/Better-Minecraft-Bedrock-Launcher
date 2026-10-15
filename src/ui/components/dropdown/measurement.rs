use super::DropdownOption;
use gpui::*;
use std::rc::Rc;

pub(super) struct LabelMeasurement {
    font: Font,
    options: Rc<Vec<DropdownOption>>,
    width: Pixels,
}

impl LabelMeasurement {
    pub(super) fn width(
        cache: &mut Option<Self>,
        window: &Window,
        options: &Rc<Vec<DropdownOption>>,
    ) -> Pixels {
        let font = Self::measurement_font(window);
        if let Some(measurement) = cache {
            if measurement.matches(&font, options) {
                return measurement.width;
            }
        }
        let width = options.iter().fold(px(0.0), |widest, option| {
            widest.max(measure_label(window, &font, &option.label))
        });
        *cache = Some(Self {
            font,
            options: options.clone(),
            width,
        });
        width
    }

    pub(super) fn cached_width(
        &self,
        window: &Window,
        options: &[DropdownOption],
    ) -> Option<Pixels> {
        self.matches(&Self::measurement_font(window), options)
            .then_some(self.width)
    }

    fn measurement_font(window: &Window) -> Font {
        let style = window.text_style();
        Font {
            family: style.font_family,
            features: style.font_features,
            fallbacks: style.font_fallbacks,
            weight: FontWeight::MEDIUM,
            style: style.font_style,
        }
    }

    fn matches(&self, font: &Font, options: &[DropdownOption]) -> bool {
        self.font == *font && self.options.as_slice() == options
    }
}

fn measure_label(window: &Window, font: &Font, label: &SharedString) -> Pixels {
    if label.is_empty() {
        return px(0.0);
    }
    window
        .text_system()
        .shape_line(
            label.clone(),
            px(13.0),
            &[TextRun {
                len: label.len(),
                font: font.clone(),
                color: window.text_style().color,
                background_color: None,
                background_corner_radius: None,
                background_padding: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn measurement_key_tracks_labels_and_font() {
        let font = font("test-font");
        let measurement = LabelMeasurement {
            font: font.clone(),
            options: Rc::new(vec!["first".into(), "second".into()]),
            width: px(42.0),
        };
        assert!(measurement.matches(&font, &["first".into(), "second".into()]));
        assert!(!measurement.matches(&font, &["other".into(), "second".into()]));
        assert!(!measurement.matches(&font, &["second".into(), "first".into()]));
        assert!(!measurement.matches(&font, &["first".into()]));
        assert!(!measurement.matches(&font.bold(), &["first".into(), "second".into()]));
    }
}
