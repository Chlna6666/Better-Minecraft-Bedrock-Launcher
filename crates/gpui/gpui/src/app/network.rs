use std::sync::Arc;

use http_client::HttpClient;

use crate::SvgRenderer;

use super::App;

impl App {
    /// Return the application's HTTP client.
    pub fn http_client(&self) -> Arc<dyn HttpClient> {
        self.http_client.clone()
    }

    /// Replace the application's HTTP client.
    pub fn set_http_client(&mut self, new_client: Arc<dyn HttpClient>) {
        self.http_client = new_client;
    }

    /// Return the application's SVG renderer.
    pub fn svg_renderer(&self) -> SvgRenderer {
        self.svg_renderer.clone()
    }
}
