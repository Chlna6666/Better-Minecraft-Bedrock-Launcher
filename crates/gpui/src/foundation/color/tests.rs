use serde_json::json;

use super::*;

#[test]
fn test_deserialize_three_value_hex_to_rgba() {
    let actual: Rgba = serde_json::from_value(json!("#f09")).unwrap();

    assert_eq!(actual, rgba(0xff0099ff))
}

#[test]
fn test_deserialize_four_value_hex_to_rgba() {
    let actual: Rgba = serde_json::from_value(json!("#f09f")).unwrap();

    assert_eq!(actual, rgba(0xff0099ff))
}

#[test]
fn test_deserialize_six_value_hex_to_rgba() {
    let actual: Rgba = serde_json::from_value(json!("#ff0099")).unwrap();

    assert_eq!(actual, rgba(0xff0099ff))
}

#[test]
fn test_deserialize_eight_value_hex_to_rgba() {
    let actual: Rgba = serde_json::from_value(json!("#ff0099ff")).unwrap();

    assert_eq!(actual, rgba(0xff0099ff))
}

#[test]
fn test_deserialize_eight_value_hex_with_padding_to_rgba() {
    let actual: Rgba = serde_json::from_value(json!(" #f5f5f5ff   ")).unwrap();

    assert_eq!(actual, rgba(0xf5f5f5ff))
}

#[test]
fn test_deserialize_eight_value_hex_with_mixed_case_to_rgba() {
    let actual: Rgba = serde_json::from_value(json!("#DeAdbEeF")).unwrap();

    assert_eq!(actual, rgba(0xdeadbeef))
}

#[test]
fn test_background_solid() {
    let color = Hsla::from(rgba(0xff0099ff));
    let mut background = Background::from(color);
    assert_eq!(background.tag, BackgroundTag::Solid);
    assert_eq!(background.solid, color);

    assert_eq!(background.opacity(0.5).solid, color.opacity(0.5));
    assert!(!background.is_transparent());
    background.solid = hsla(0.0, 0.0, 0.0, 0.0);
    assert!(background.is_transparent());
}

#[test]
fn test_background_linear_gradient() {
    let from = linear_color_stop(rgba(0xff0099ff), 0.0);
    let to = linear_color_stop(rgba(0x00ff99ff), 1.0);
    let background = linear_gradient(90.0, from, to);
    assert_eq!(background.tag, BackgroundTag::LinearGradient);
    assert_eq!(background.colors[0], from);
    assert_eq!(background.colors[1], to);

    assert_eq!(background.opacity(0.5).colors[0], from.opacity(0.5));
    assert_eq!(background.opacity(0.5).colors[1], to.opacity(0.5));
    assert!(!background.is_transparent());
    assert!(background.opacity(0.0).is_transparent());
}
