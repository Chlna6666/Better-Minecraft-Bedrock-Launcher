use crate::ui::components::markdown_renderer::{MarkdownDocument, parse_markdown_document};
use gpui::{Global, ScrollHandle, px};
use std::sync::Arc;

const AGREEMENT_ZH_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/locales/agreement/zh.md"
));
const AGREEMENT_EN_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/locales/agreement/en.md"
));

#[derive(Default)]
pub struct AgreementState {
    pub accepted: bool,
    pub show_modal: bool,
    pub agreement_scroll_handle: ScrollHandle,
    pub accept_unlocked: bool,
    cached_locale_key: Option<String>,
    cached_document: Arc<MarkdownDocument>,
}

impl Global for AgreementState {}

impl AgreementState {
    pub fn initialize(&mut self, accepted: bool) {
        self.accepted = accepted;
        self.show_modal = !accepted;
        self.agreement_scroll_handle = ScrollHandle::new();
        self.accept_unlocked = accepted;
    }

    pub fn accept(&mut self) {
        self.accepted = true;
        self.show_modal = false;
        self.accept_unlocked = true;
    }

    pub fn is_visible(&self) -> bool {
        self.show_modal && !self.accepted
    }

    pub fn get_or_cache_document(&mut self, locale_code: &str) -> Arc<MarkdownDocument> {
        let locale_key = if locale_code.to_ascii_lowercase().starts_with("zh") {
            "zh"
        } else {
            "en"
        };

        let needs_cache = self.cached_locale_key.as_deref() != Some(locale_key);
        if needs_cache {
            let markdown = if locale_key == "zh" {
                AGREEMENT_ZH_MD
            } else {
                AGREEMENT_EN_MD
            };
            self.cached_document = Arc::new(parse_markdown_document(markdown));
            self.cached_locale_key = Some(locale_key.to_string());
        }

        self.cached_document.clone()
    }

    pub fn cached_document(&self) -> Arc<MarkdownDocument> {
        self.cached_document.clone()
    }

    pub fn unlock_accept_if_scrolled_to_end(&mut self) -> bool {
        if self.accepted || self.accept_unlocked || !self.show_modal {
            return false;
        }

        let viewport_height = self.agreement_scroll_handle.bounds().size.height / px(1.);
        if viewport_height <= 1.0 {
            return false;
        }

        let max_scroll_y = self.agreement_scroll_handle.max_offset().height / px(1.);
        let current_scroll_y = -(self.agreement_scroll_handle.offset().y / px(1.));
        let reached_bottom = max_scroll_y <= 1.0 || current_scroll_y + 2.0 >= max_scroll_y;

        if reached_bottom {
            self.accept_unlocked = true;
            return true;
        }

        false
    }
}
