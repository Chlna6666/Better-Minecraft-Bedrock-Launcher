use std::{any::type_name, borrow::Cow, path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Result;
use futures::FutureExt;
use http_client::{HttpClient, Url};

use crate::{
    AssetSource, BackgroundExecutor, ForegroundExecutor, ImagePipelineConfig, RendererBackend,
    RendererOptions, SharedString, SvgRenderer, TextSystem, current_platform,
    record_renderer_backend,
};

use super::{App, AppCell};

/// A reference to a GPUI application, typically constructed in the `main` function of your app.
/// You won't interact with this type much outside of initial configuration and startup.
pub struct Application(Rc<AppCell>);

/// A font source used by [`DefaultFontConfig`].
#[derive(Clone)]
pub enum FontSource {
    /// Use a font family already installed on the operating system.
    SystemFamily(SharedString),
    /// Load a font file from disk before the application starts.
    Path(PathBuf),
    /// Load font bytes embedded in the application binary.
    Embedded(Cow<'static, [u8]>),
}

/// Application-wide default text style configuration.
#[derive(Clone)]
pub struct DefaultFontConfig {
    /// The family name used by `TextStyle::default()` and new windows.
    pub family: SharedString,
    /// Fonts to register before the default family is used.
    pub sources: Vec<FontSource>,
}

impl DefaultFontConfig {
    /// Use a system font family as the application default.
    pub fn system_family(family: impl Into<SharedString>) -> Self {
        Self {
            family: family.into(),
            sources: Vec::new(),
        }
    }

    /// Use embedded font bytes as the application default family.
    pub fn embedded(
        family: impl Into<SharedString>,
        fonts: impl IntoIterator<Item = Cow<'static, [u8]>>,
    ) -> Self {
        Self {
            family: family.into(),
            sources: fonts.into_iter().map(FontSource::Embedded).collect(),
        }
    }

    /// Add a font file to load before selecting the default family.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.sources.push(FontSource::Path(path.into()));
        self
    }

    /// Add embedded font bytes to load before selecting the default family.
    pub fn with_embedded(mut self, bytes: impl Into<Cow<'static, [u8]>>) -> Self {
        self.sources.push(FontSource::Embedded(bytes.into()));
        self
    }
}

/// Represents an application before it is fully launched. Once your app is
/// configured, you'll start the app with `App::run`.
impl Application {
    /// Builds an app with the given asset source.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::new_with_renderer_options(RendererOptions::default())
    }

    /// Builds an app with the requested renderer options in a single platform
    /// initialization pass.
    pub fn new_with_renderer_options(options: RendererOptions) -> Self {
        #[cfg(any(test, feature = "test-support"))]
        log::info!("GPUI was compiled in test mode");

        let options = options.resolve();
        record_renderer_backend(options.backend);
        Self(App::new_app(
            current_platform(false, options),
            Arc::new(()),
            Arc::new(NullHttpClient),
        ))
    }

    /// Builds an app with the requested renderer backend in a single platform
    /// initialization pass.
    pub fn new_with_renderer_backend(backend: RendererBackend) -> Self {
        Self::new_with_renderer_options(RendererOptions::with_backend(backend))
    }

    /// Build an app in headless mode. This prevents opening windows,
    /// but makes it possible to run an application in an context like
    /// SSH, where GUI applications are not allowed.
    pub fn headless() -> Self {
        record_renderer_backend(RendererBackend::Auto);
        Self(App::new_app(
            current_platform(true, RendererOptions::default()),
            Arc::new(()),
            Arc::new(NullHttpClient),
        ))
    }

    /// Sets the preferred renderer backend before the application is launched.
    ///
    /// `GPUI_RENDERER` takes precedence when it is set so local debugging and
    /// release diagnostics can override builder configuration without code changes.
    pub fn with_renderer_backend(self, backend: RendererBackend) -> Self {
        self.with_renderer_options(RendererOptions::with_backend(backend))
    }

    /// Sets renderer options before the application is launched.
    pub fn with_renderer_options(self, options: RendererOptions) -> Self {
        let options = options.resolve();
        record_renderer_backend(options.backend);
        self.0.borrow_mut().platform = current_platform(false, options);
        self
    }

    /// Sets the application-wide default font family and registers any provided font sources.
    pub fn try_with_default_font(self, config: DefaultFontConfig) -> Result<Self> {
        let family = config.family.clone();
        self.apply_default_font(config)?;
        self.0
            .borrow()
            .text_system
            .log_application_default_font(&family);
        Ok(self)
    }

    /// Sets the application-wide default font family, falling back to the platform font on error.
    pub fn with_default_font_or_platform_default(self, config: DefaultFontConfig) -> Self {
        let family = config.family.clone();
        if let Err(error) = self.apply_default_font(config) {
            self.0
                .borrow()
                .text_system
                .log_application_default_font_fallback(&family, &error);
        } else {
            self.0
                .borrow()
                .text_system
                .log_application_default_font(&family);
        }
        self
    }

    /// Registers the default native window icon for the application.
    ///
    /// Windows opened without an explicit [`crate::WindowOptions::window_icon`] will inherit it.
    pub fn with_default_window_icon(self, icon: crate::WindowIconSource) -> Self {
        self.0.borrow_mut().default_window_icon = Some(icon);
        self
    }

    fn apply_default_font(&self, config: DefaultFontConfig) -> Result<()> {
        let mut fonts = Vec::new();
        let mut font_paths = Vec::new();
        for source in config.sources {
            match source {
                FontSource::SystemFamily(_) => {}
                FontSource::Path(path) => font_paths.push(path),
                FontSource::Embedded(bytes) => fonts.push(bytes),
            }
        }

        let mut context_lock = self.0.borrow_mut();
        if !font_paths.is_empty() {
            context_lock.text_system.add_font_paths(font_paths)?;
        }
        if !fonts.is_empty() {
            context_lock.text_system.add_fonts(fonts)?;
        }
        context_lock
            .text_system
            .preload_font_family(config.family.clone())?;
        context_lock
            .text_system
            .set_system_font_family(config.family.clone());
        context_lock.default_text_style.font_family = config.family;
        drop(context_lock);
        Ok(())
    }

    /// Sets the application-wide default font family without registering font data.
    pub fn try_with_default_font_family(self, family: impl Into<SharedString>) -> Result<Self> {
        self.try_with_default_font(DefaultFontConfig::system_family(family))
    }

    /// Sets application-wide image pipeline limits and animated image behavior.
    pub fn with_image_pipeline_config(self, config: ImagePipelineConfig) -> Self {
        self.0.borrow_mut().image_pipeline_config = config;
        self
    }

    /// Assign
    pub fn with_assets(self, asset_source: impl AssetSource) -> Self {
        let mut context_lock = self.0.borrow_mut();
        let asset_source = Arc::new(asset_source);
        context_lock.asset_source = asset_source.clone();
        context_lock.svg_renderer = SvgRenderer::new(asset_source);
        drop(context_lock);
        self
    }

    /// Sets the HTTP client for the application.
    pub fn with_http_client(self, http_client: Arc<dyn HttpClient>) -> Self {
        let mut context_lock = self.0.borrow_mut();
        context_lock.http_client = http_client;
        drop(context_lock);
        self
    }

    /// Start the application. The provided callback will be called once the
    /// app is fully launched.
    pub fn run<F>(self, on_finish_launching: F)
    where
        F: 'static + FnOnce(&mut App),
    {
        let this = self.0.clone();
        let platform = {
            let app = self.0.borrow();
            app.text_system.log_platform_default_font_once();
            app.platform.clone()
        };
        platform.run(Box::new(move || {
            let cx = &mut *this.borrow_mut();
            on_finish_launching(cx);
        }));
    }

    /// Register a handler to be invoked when the platform instructs the application
    /// to open one or more URLs.
    pub fn on_open_urls<F>(&self, mut callback: F) -> &Self
    where
        F: 'static + FnMut(Vec<String>),
    {
        self.0.borrow().platform.on_open_urls(Box::new(callback));
        self
    }

    /// Invokes a handler when an already-running application is launched.
    /// On macOS, this can occur when the application icon is double-clicked or the app is launched via the dock.
    pub fn on_reopen<F>(&self, mut callback: F) -> &Self
    where
        F: 'static + FnMut(&mut App),
    {
        let this = Rc::downgrade(&self.0);
        self.0.borrow_mut().platform.on_reopen(Box::new(move || {
            if let Some(app) = this.upgrade() {
                callback(&mut app.borrow_mut());
            }
        }));
        self
    }

    /// Returns a handle to the [`BackgroundExecutor`] associated with this app, which can be used to spawn futures in the background.
    pub fn background_executor(&self) -> BackgroundExecutor {
        self.0.borrow().background_executor.clone()
    }

    /// Returns a handle to the [`ForegroundExecutor`] associated with this app, which can be used to spawn futures in the foreground.
    pub fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.borrow().foreground_executor.clone()
    }

    /// Returns a reference to the [`TextSystem`] associated with this app.
    pub fn text_system(&self) -> Arc<TextSystem> {
        self.0.borrow().text_system.clone()
    }

    /// Returns the file URL of the executable with the specified name in the application bundle
    pub fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.0.borrow().path_for_auxiliary_executable(name)
    }
}

struct NullHttpClient;

impl HttpClient for NullHttpClient {
    fn send(
        &self,
        _req: http_client::Request<http_client::AsyncBody>,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<http_client::Response<http_client::AsyncBody>>,
    > {
        async move {
            anyhow::bail!("No HttpClient available");
        }
        .boxed()
    }

    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }
}
