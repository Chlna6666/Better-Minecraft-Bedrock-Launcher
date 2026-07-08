use super::*;

#[::core::prelude::v1::test]
fn collapsed_selector_range_uses_current_page() {
    assert_eq!(skin_selector_range(65, false, 0), 0..30);
    assert_eq!(skin_selector_range(65, false, 1), 30..60);
    assert_eq!(skin_selector_range(65, false, 2), 60..65);
    assert_eq!(skin_selector_range(65, false, 99), 60..65);
}

#[::core::prelude::v1::test]
fn expanded_selector_range_shows_all_skins() {
    assert_eq!(skin_selector_range(65, true, 2), 0..65);
    assert_eq!(skin_selector_range(0, true, 0), 0..0);
}
