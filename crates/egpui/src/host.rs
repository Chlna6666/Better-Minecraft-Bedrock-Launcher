use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use gpui::{App, Application, Global, Subscription, Task};
use thiserror::Error;

use crate::{
    ApplicationLifecycle, ApplicationRuntime, LifecycleState, RuntimeConfig, RuntimeError,
    ServiceRegistry, TaskScope, UiBridgeConfig, UiHandle, ui,
};

/// Configuration for an [`ApplicationHost`].
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Whether to construct GPUI without opening native windows.
    pub headless: bool,
    /// Physical application runtime sizing.
    pub runtime: RuntimeConfig,
    /// Maximum time allotted to application shutdown.
    pub shutdown_timeout: Duration,
    /// Capacity of each background-to-GUI bounded queue.
    pub ui_queue_capacity: NonZeroUsize,
    /// Maximum queued actions applied before yielding to GPUI.
    pub ui_maximum_batch_size: NonZeroUsize,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            headless: false,
            runtime: RuntimeConfig::default(),
            shutdown_timeout: Duration::from_secs(5),
            ui_queue_capacity: NonZeroUsize::new(256).unwrap_or(NonZeroUsize::MIN),
            ui_maximum_batch_size: NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

impl HostConfig {
    /// Returns a configuration for a headless host.
    #[must_use]
    pub fn headless() -> Self {
        Self {
            headless: true,
            ..Self::default()
        }
    }
}

/// Errors returned while constructing or stopping an application host.
#[derive(Debug, Error)]
pub enum HostError {
    /// The application runtime could not be initialized or stopped.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

/// Application-scoped state passed to the launch callback.
#[derive(Clone)]
pub struct ApplicationContext {
    runtime: ApplicationRuntime,
    scope: TaskScope,
    services: Arc<ServiceRegistry>,
    lifecycle: ApplicationLifecycle,
    ui: UiHandle,
}

impl ApplicationContext {
    /// Returns the application background runtime.
    #[must_use]
    pub fn runtime(&self) -> &ApplicationRuntime {
        &self.runtime
    }

    /// Returns the application task scope.
    #[must_use]
    pub fn scope(&self) -> &TaskScope {
        &self.scope
    }

    /// Returns the application service registry.
    #[must_use]
    pub fn services(&self) -> &ServiceRegistry {
        &self.services
    }

    /// Returns the lifecycle observer.
    #[must_use]
    pub fn lifecycle(&self) -> &ApplicationLifecycle {
        &self.lifecycle
    }

    /// Returns the bounded background-to-GUI dispatcher.
    #[must_use]
    pub fn ui(&self) -> &UiHandle {
        &self.ui
    }
}

struct HostGlobal {
    _quit_subscription: Subscription,
    _ui_task: Task<()>,
    _context: ApplicationContext,
}

impl Global for HostGlobal {}

/// Owns one GPUI application and one application runtime.
pub struct ApplicationHost {
    application: Application,
    runtime: ApplicationRuntime,
    services: Arc<ServiceRegistry>,
    lifecycle: ApplicationLifecycle,
    config: HostConfig,
}

impl ApplicationHost {
    /// Constructs a host with the configured GPUI mode and default provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the application runtime cannot be initialized.
    pub fn new(config: HostConfig) -> Result<Self, HostError> {
        let runtime = ApplicationRuntime::new(config.runtime)?;
        let application = if config.headless {
            Application::headless()
        } else {
            Application::new()
        };

        Ok(Self {
            application,
            runtime,
            services: Arc::new(ServiceRegistry::new()),
            lifecycle: ApplicationLifecycle::new(),
            config,
        })
    }

    /// Constructs a headless application host.
    ///
    /// # Errors
    ///
    /// Returns an error when the application runtime cannot be initialized.
    pub fn headless() -> Result<Self, HostError> {
        Self::new(HostConfig::headless())
    }

    /// Constructs a host around an already configured GPUI application.
    ///
    /// This is the escape hatch for application-owned renderer, assets and
    /// window configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the application runtime cannot be initialized.
    pub fn with_application(
        application: Application,
        config: HostConfig,
    ) -> Result<Self, HostError> {
        let runtime = ApplicationRuntime::new(config.runtime)?;
        Ok(Self {
            application,
            runtime,
            services: Arc::new(ServiceRegistry::new()),
            lifecycle: ApplicationLifecycle::new(),
            config,
        })
    }

    /// Returns the service registry before the event loop starts.
    #[must_use]
    pub fn services(&self) -> &ServiceRegistry {
        &self.services
    }

    /// Returns the application runtime before the event loop starts.
    #[must_use]
    pub fn runtime(&self) -> &ApplicationRuntime {
        &self.runtime
    }

    /// Returns a lifecycle observer that can be shared with services.
    #[must_use]
    pub fn lifecycle(&self) -> ApplicationLifecycle {
        self.lifecycle.clone()
    }

    /// Runs the native event loop and converges application execution domains.
    ///
    /// The launch callback receives an application context followed by GPUI's
    /// mutable [`App`]. The callback is the only place where an application
    /// should create windows and entities.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime shutdown coordination fails.
    pub fn run(
        self,
        on_launch: impl FnOnce(ApplicationContext, &mut App) + 'static,
    ) -> Result<crate::ShutdownReport, HostError> {
        let ApplicationHost {
            application,
            runtime,
            services,
            lifecycle,
            config,
        } = self;
        lifecycle.transition_to(LifecycleState::Starting);

        let bridge_config = UiBridgeConfig {
            capacity: config.ui_queue_capacity,
            maximum_batch_size: config.ui_maximum_batch_size,
        };
        let (ui, receiver) = UiHandle::channel(bridge_config);
        let context = ApplicationContext {
            runtime: runtime.clone(),
            scope: runtime.application_scope(),
            services,
            lifecycle: lifecycle.clone(),
            ui,
        };
        let quit_runtime = runtime.clone();
        let quit_lifecycle = lifecycle.clone();
        let launch_context = context.clone();
        let launch_lifecycle = lifecycle.clone();

        application.run(move |cx| {
            launch_lifecycle.transition_to(LifecycleState::Running);
            let quit_subscription = cx.on_app_quit(move |_| {
                quit_lifecycle.transition_to(LifecycleState::ShutdownRequested);
                quit_runtime.request_shutdown();
                async {}
            });
            let ui_task = ui::bind_ui_handle(receiver, bridge_config.maximum_batch_size, cx);
            cx.set_global(HostGlobal {
                _quit_subscription: quit_subscription,
                _ui_task: ui_task,
                _context: context,
            });
            on_launch(launch_context, cx);
        });

        lifecycle.transition_to(LifecycleState::ShutdownRequested);
        let report = runtime.shutdown(config.shutdown_timeout)?;
        lifecycle.transition_to(LifecycleState::Stopped);
        Ok(report)
    }
}
