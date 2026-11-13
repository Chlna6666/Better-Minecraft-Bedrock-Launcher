use super::model::*;
use super::prelude::*;

pub(super) fn new_field(
    window: &mut Window,
    cx: &mut Context<View>,
    placeholder: &'static str,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_placeholder(SharedString::from(placeholder), cx);
        input
    })
}

pub(super) fn subscriptions(
    input_fields: &InputFields,
    cx: &mut Context<View>,
) -> Vec<Subscription> {
    [
        (InputField::CenterX, input_fields.center_x.clone()),
        (InputField::CenterZ, input_fields.center_z.clone()),
        (
            InputField::ZoomPercent,
            input_fields.zoom_percent.clone(),
        ),
        (
            InputField::DimensionId,
            input_fields.dimension_id.clone(),
        ),
        (
            InputField::SelectionMinY,
            input_fields.selection_min_y.clone(),
        ),
        (
            InputField::SelectionMaxY,
            input_fields.selection_max_y.clone(),
        ),
        (
            InputField::PasteOriginY,
            input_fields.paste_origin_y.clone(),
        ),
    ]
    .into_iter()
    .map(|(field, input)| {
        cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
            this.handle_input_event(field, input, event, cx);
        })
    })
    .collect()
}

pub(super) fn initialize_values(
    input_fields: &InputFields,
    values: [(InputField, String); 7],
    window: &mut Window,
    cx: &mut Context<View>,
) {
    for (field, value) in values {
        input_fields.entity(field).update(cx, |input, cx| {
            input.set_value(SharedString::from(value), window, cx);
        });
    }
}

pub(super) fn integer(value: &str, label: &'static str) -> Result<i32, SharedString> {
    if value.is_empty() {
        return Err(SharedString::from(format!("{label} 不能为空")));
    }
    value
        .parse::<i32>()
        .map_err(|_| SharedString::from(format!("{label} 必须是整数")))
}

pub(super) fn optional_integer(
    value: &str,
    label: &'static str,
    fallback: i32,
) -> Result<i32, SharedString> {
    let value = value.trim();
    if value.is_empty() {
        Ok(fallback)
    } else {
        integer(value, label)
    }
}

pub(super) fn zoom_scale(value: &str) -> Result<f32, SharedString> {
    let value = value.trim_end_matches('%').trim();
    if value.is_empty() {
        return Err(SharedString::from("缩放不能为空"));
    }
    let percent = value
        .parse::<f32>()
        .map_err(|_| SharedString::from("缩放必须是数字或百分比"))?;
    if !percent.is_finite() || percent <= 0.0 {
        return Err(SharedString::from("缩放必须大于 0"));
    }
    Ok((percent / 100.0).clamp(MIN_VIEWPORT_SCALE, MAX_VIEWPORT_SCALE))
}
