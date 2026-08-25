use anyhow::Result;

use crate::Task;

use super::App;

impl App {
    /// Open a URL in the platform's default browser.
    pub fn open_url(&self, url: &str) {
        self.platform.open_url(url);
    }

    /// Register a URL scheme to be opened by the current application.
    pub fn register_url_scheme(&self, scheme: &str) -> Task<Result<()>> {
        self.platform.register_url_scheme(scheme)
    }
}
