use serde_json::Value;

pub(super) fn point3_array(value: Option<&Value>) -> Vec<[f32; 3]> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| array3(Some(value)))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn point2_array(value: Option<&Value>) -> Vec<[f32; 2]> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| array2(Some(value)))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn index_triplet(value: &Value) -> Option<[usize; 3]> {
    let values = value.as_array()?;
    Some([
        usize::try_from(values.first()?.as_u64()?).ok()?,
        usize::try_from(values.get(1)?.as_u64()?).ok()?,
        usize::try_from(values.get(2)?.as_u64()?).ok()?,
    ])
}

pub(super) fn array2(value: Option<&Value>) -> Option<[f32; 2]> {
    let values = value?.as_array()?;
    Some([
        values.first()?.as_f64()? as f32,
        values.get(1)?.as_f64()? as f32,
    ])
}

pub(super) fn array3(value: Option<&Value>) -> Option<[f32; 3]> {
    let values = value?.as_array()?;
    Some([
        values.first()?.as_f64()? as f32,
        values.get(1)?.as_f64()? as f32,
        values.get(2)?.as_f64()? as f32,
    ])
}

pub(super) fn array4(value: Option<&Value>) -> Option<[f32; 4]> {
    let values = value?.as_array()?;
    Some([
        values.first()?.as_f64()? as f32,
        values.get(1)?.as_f64()? as f32,
        values.get(2)?.as_f64()? as f32,
        values.get(3)?.as_f64()? as f32,
    ])
}

pub(super) fn first_number(values: &[Option<&Value>], fallback: f32) -> f32 {
    values
        .iter()
        .find_map(|value| value.and_then(Value::as_f64))
        .map(|value| value as f32)
        .unwrap_or(fallback)
}
