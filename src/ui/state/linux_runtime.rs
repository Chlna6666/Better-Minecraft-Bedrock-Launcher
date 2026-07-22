use crate::core::linux_runtime::{LinuxInstallPlan, LinuxRuntimeCheck};
use gpui::{Global, SharedString};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LinuxRuntimeStatus {
    #[default]
    Idle,
    Checking,
    Missing,
    Installing,
    Ready,
    Error,
}

#[derive(Default)]
pub struct LinuxRuntimeState {
    pub visible: bool,
    pub check_started: bool,
    pub request_id: u64,
    pub status: LinuxRuntimeStatus,
    pub error_message: Option<SharedString>,
    pub(crate) check: Option<LinuxRuntimeCheck>,
}

impl Global for LinuxRuntimeState {}

impl LinuxRuntimeState {
    pub fn begin_check(&mut self, show_while_checking: bool) -> Option<u64> {
        if self.status == LinuxRuntimeStatus::Installing {
            return None;
        }
        self.check_started = true;
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.status = LinuxRuntimeStatus::Checking;
        self.error_message = None;
        if show_while_checking {
            self.visible = true;
        }
        Some(self.request_id)
    }

    pub fn apply_check(&mut self, request_id: u64, check: LinuxRuntimeCheck) -> bool {
        if self.request_id != request_id {
            return false;
        }
        self.visible = !check.is_ready();
        self.status = if check.is_ready() {
            LinuxRuntimeStatus::Ready
        } else {
            LinuxRuntimeStatus::Missing
        };
        self.error_message = None;
        self.check = Some(check);
        true
    }

    pub fn set_check_error(&mut self, request_id: u64, error: impl Into<SharedString>) -> bool {
        if self.request_id != request_id {
            return false;
        }
        self.visible = true;
        self.status = LinuxRuntimeStatus::Error;
        self.error_message = Some(error.into());
        true
    }

    pub fn begin_install(&mut self) -> Option<(u64, LinuxInstallPlan)> {
        if self.status == LinuxRuntimeStatus::Installing {
            return None;
        }
        let plan = self.check.as_ref()?.install_plan.clone()?;
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = true;
        self.status = LinuxRuntimeStatus::Installing;
        self.error_message = None;
        Some((self.request_id, plan))
    }

    pub fn set_install_error(&mut self, request_id: u64, error: impl Into<SharedString>) -> bool {
        if self.request_id != request_id {
            return false;
        }
        self.visible = true;
        self.status = LinuxRuntimeStatus::Error;
        self.error_message = Some(error.into());
        true
    }

    pub fn dismiss(&mut self) {
        if self.status != LinuxRuntimeStatus::Installing {
            self.visible = false;
        }
    }
}
