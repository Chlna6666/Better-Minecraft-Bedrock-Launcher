use std::collections::HashMap;

use crate::i18n::{I18nKey, LocalizedText};
use gpui::{Bounds, Global, Pixels};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnboardingScene {
    #[default]
    Welcome,
    DownloadNavigation,
    GameDownload,
    ResourcePackDownload,
    ModDownload,
    ImportPackage,
    TasksOverview,
    ManageOverview,
    ManageContent,
    SettingsOverview,
    ToolsOverview,
    PlatformSetup,
    Finish,
}

impl OnboardingScene {
    #[cfg(target_os = "linux")]
    pub const COUNT: usize = 13;
    #[cfg(not(target_os = "linux"))]
    pub const COUNT: usize = 12;

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Welcome => 1,
            Self::DownloadNavigation => 2,
            Self::GameDownload => 3,
            Self::ResourcePackDownload => 4,
            Self::ModDownload => 5,
            Self::ImportPackage => 6,
            Self::TasksOverview => 7,
            Self::ManageOverview => 8,
            Self::ManageContent => 9,
            Self::SettingsOverview => 10,
            Self::ToolsOverview => 11,
            Self::PlatformSetup => 12,
            Self::Finish => {
                #[cfg(target_os = "linux")]
                {
                    13
                }
                #[cfg(not(target_os = "linux"))]
                {
                    12
                }
            }
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Welcome => Self::DownloadNavigation,
            Self::DownloadNavigation => Self::GameDownload,
            Self::GameDownload => Self::ResourcePackDownload,
            Self::ResourcePackDownload => Self::ModDownload,
            Self::ModDownload => Self::ImportPackage,
            Self::ImportPackage => Self::TasksOverview,
            Self::TasksOverview => Self::ManageOverview,
            Self::ManageOverview => Self::ManageContent,
            Self::ManageContent => Self::SettingsOverview,
            Self::SettingsOverview => Self::ToolsOverview,
            Self::ToolsOverview => {
                #[cfg(target_os = "linux")]
                {
                    Self::PlatformSetup
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Self::Finish
                }
            }
            Self::PlatformSetup => Self::Finish,
            Self::Finish => Self::Finish,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Welcome => Self::Welcome,
            Self::DownloadNavigation => Self::Welcome,
            Self::GameDownload => Self::DownloadNavigation,
            Self::ResourcePackDownload => Self::GameDownload,
            Self::ModDownload => Self::ResourcePackDownload,
            Self::ImportPackage => Self::ModDownload,
            Self::TasksOverview => Self::ImportPackage,
            Self::ManageOverview => Self::TasksOverview,
            Self::ManageContent => Self::ManageOverview,
            Self::SettingsOverview => Self::ManageContent,
            Self::ToolsOverview => Self::SettingsOverview,
            Self::PlatformSetup => Self::ToolsOverview,
            Self::Finish => {
                #[cfg(target_os = "linux")]
                {
                    Self::PlatformSetup
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Self::ToolsOverview
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OnboardingAnchor {
    DownloadToolbar,
    DownloadTabs,
    DownloadImport,
    TasksPage,
    SettingsTabs,
    ToolsSidebar,
}

#[derive(Clone, Debug)]
pub struct OnboardingSummaryItem {
    pub label: I18nKey,
    pub warning: bool,
}

#[derive(Clone, Debug)]
pub struct OnboardingPlatformSummary {
    pub ready: bool,
    pub missing_reason: Option<String>,
    pub distribution_name: String,
    pub runner: Option<String>,
    pub local_versions: usize,
    pub items: Vec<OnboardingSummaryItem>,
}

pub struct OnboardingTourState {
    pub visible: bool,
    pub scene: OnboardingScene,
    pub reopened: bool,
    pub platform_scanning: bool,
    pub platform_summary: Option<OnboardingPlatformSummary>,
    pub error: Option<LocalizedText>,
    anchors: HashMap<OnboardingAnchor, Bounds<Pixels>>,
    request_id: u64,
}

impl Global for OnboardingTourState {}

impl Default for OnboardingTourState {
    fn default() -> Self {
        Self {
            visible: !crate::config::onboarding::is_current_onboarding_completed(),
            scene: OnboardingScene::Welcome,
            reopened: false,
            platform_scanning: false,
            platform_summary: None,
            error: None,
            anchors: HashMap::new(),
            request_id: 0,
        }
    }
}

impl OnboardingTourState {
    pub fn reopen(&mut self) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = true;
        self.scene = OnboardingScene::Welcome;
        self.reopened = true;
        self.platform_scanning = false;
        self.platform_summary = None;
        self.error = None;
        self.anchors.clear();
    }

    pub fn set_scene(&mut self, scene: OnboardingScene) {
        self.scene = scene;
        self.error = None;
        self.anchors.clear();
        if scene != OnboardingScene::PlatformSetup {
            self.platform_scanning = false;
        }
    }

    #[must_use]
    pub fn anchor(&self, anchor: OnboardingAnchor) -> Option<Bounds<Pixels>> {
        self.anchors.get(&anchor).copied()
    }

    pub fn set_anchor(&mut self, anchor: OnboardingAnchor, bounds: Bounds<Pixels>) {
        self.anchors.insert(anchor, bounds);
    }

    pub fn begin_platform_scan(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.platform_scanning = true;
        self.platform_summary = None;
        self.error = None;
        self.request_id
    }

    pub fn apply_platform_summary(
        &mut self,
        request_id: u64,
        summary: OnboardingPlatformSummary,
    ) -> bool {
        if !self.visible
            || self.scene != OnboardingScene::PlatformSetup
            || self.request_id != request_id
        {
            return false;
        }
        self.platform_scanning = false;
        self.platform_summary = Some(summary);
        self.error = None;
        true
    }

    pub fn set_error(&mut self, request_id: u64, error: LocalizedText) -> bool {
        if !self.visible || self.request_id != request_id {
            return false;
        }
        self.platform_scanning = false;
        self.error = Some(error);
        true
    }

    pub fn set_persist_error(&mut self, error: LocalizedText) {
        self.error = Some(error);
    }

    pub fn finish(&mut self) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = false;
        self.platform_scanning = false;
        self.error = None;
        self.anchors.clear();
    }
}
