#![cfg(target_os = "linux")]

use gpui::{Global, SharedString};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LinuxOnboardingStep {
    #[default]
    Welcome,
    Environment,
    AcquireGame,
    Runtime,
}

#[derive(Clone, Debug, Default)]
pub struct LinuxOnboardingEnvironmentSummary {
    pub distribution_name: SharedString,
    pub runtime_ready: bool,
    pub runner_label: Option<SharedString>,
    pub missing_reason: Option<SharedString>,
    pub bmcbl_versions: u64,
}

pub struct LinuxOnboardingState {
    pub visible: bool,
    pub step: LinuxOnboardingStep,
    pub scanning: bool,
    pub environment: Option<LinuxOnboardingEnvironmentSummary>,
    pub error: Option<SharedString>,
    request_id: u64,
}

impl Global for LinuxOnboardingState {}

impl Default for LinuxOnboardingState {
    fn default() -> Self {
        Self {
            visible: !crate::config::onboarding::is_current_onboarding_completed(),
            step: LinuxOnboardingStep::Welcome,
            scanning: false,
            environment: None,
            error: None,
            request_id: 0,
        }
    }
}

impl LinuxOnboardingState {
    pub fn reopen(&mut self) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = true;
        self.step = LinuxOnboardingStep::Welcome;
        self.scanning = false;
        self.environment = None;
        self.error = None;
    }

    pub fn begin_scan(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = true;
        self.step = LinuxOnboardingStep::Environment;
        self.scanning = true;
        self.environment = None;
        self.error = None;
        self.request_id
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn request_id_for_error(&self) -> u64 {
        self.request_id
    }

    pub fn apply_environment(
        &mut self,
        request_id: u64,
        environment: LinuxOnboardingEnvironmentSummary,
    ) -> bool {
        if !self.visible || self.request_id != request_id {
            return false;
        }
        self.environment = Some(environment);
        self.scanning = false;
        self.error = None;
        true
    }

    pub fn set_error(&mut self, request_id: u64, error: impl Into<SharedString>) -> bool {
        if !self.visible || self.request_id != request_id {
            return false;
        }
        self.scanning = false;
        self.error = Some(error.into());
        true
    }

    pub fn next(&mut self) {
        if !self.visible {
            return;
        }
        self.step = match self.step {
            LinuxOnboardingStep::Welcome => LinuxOnboardingStep::Environment,
            LinuxOnboardingStep::Environment => LinuxOnboardingStep::AcquireGame,
            LinuxOnboardingStep::AcquireGame => LinuxOnboardingStep::Runtime,
            LinuxOnboardingStep::Runtime => LinuxOnboardingStep::Runtime,
        };
    }

    pub fn back(&mut self) {
        if !self.visible {
            return;
        }
        self.step = match self.step {
            LinuxOnboardingStep::Welcome => LinuxOnboardingStep::Welcome,
            LinuxOnboardingStep::Environment => LinuxOnboardingStep::Welcome,
            LinuxOnboardingStep::AcquireGame => LinuxOnboardingStep::Environment,
            LinuxOnboardingStep::Runtime => LinuxOnboardingStep::AcquireGame,
        };
    }

    pub fn finish(&mut self) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = false;
        self.scanning = false;
        self.error = None;
    }
}
