use std::collections::HashMap;

use gpui::{Bounds, Global, Pixels};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnboardingScene {
    #[default]
    Welcome,
    GameDownload,
    ResourcePackDownload,
    ModDownload,
    ImportPackage,
    ManageOverview,
    PlatformSetup,
    Finish,
}

impl OnboardingScene {
    pub const COUNT: usize = 8;

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Welcome => 1,
            Self::GameDownload => 2,
            Self::ResourcePackDownload => 3,
            Self::ModDownload => 4,
            Self::ImportPackage => 5,
            Self::ManageOverview => 6,
            Self::PlatformSetup => 7,
            Self::Finish => 8,
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Welcome => Self::GameDownload,
            Self::GameDownload => Self::ResourcePackDownload,
            Self::ResourcePackDownload => Self::ModDownload,
            Self::ModDownload => Self::ImportPackage,
            Self::ImportPackage => Self::ManageOverview,
            Self::ManageOverview => Self::PlatformSetup,
            Self::PlatformSetup => Self::Finish,
            Self::Finish => Self::Finish,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Welcome => Self::Welcome,
            Self::GameDownload => Self::Welcome,
            Self::ResourcePackDownload => Self::GameDownload,
            Self::ModDownload => Self::ResourcePackDownload,
            Self::ImportPackage => Self::ModDownload,
            Self::ManageOverview => Self::ImportPackage,
            Self::PlatformSetup => Self::ManageOverview,
            Self::Finish => Self::PlatformSetup,
        }
    }

    #[must_use]
    pub const fn is_download_scene(self) -> bool {
        matches!(
            self,
            Self::GameDownload
                | Self::ResourcePackDownload
                | Self::ModDownload
                | Self::ImportPackage
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OnboardingAnchor {
    DownloadToolbar,
    DownloadImport,
    VersionSidebar,
}

#[derive(Clone, Debug)]
pub struct OnboardingSummaryItem {
    pub label: String,
    pub value: String,
    pub warning: bool,
}

#[derive(Clone, Debug)]
pub struct OnboardingPlatformSummary {
    pub title: String,
    pub detail: String,
    pub items: Vec<OnboardingSummaryItem>,
}

pub struct OnboardingTourState {
    pub visible: bool,
    pub scene: OnboardingScene,
    pub reopened: bool,
    pub platform_scanning: bool,
    pub platform_summary: Option<OnboardingPlatformSummary>,
    pub error: Option<String>,
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

    pub fn set_error(&mut self, request_id: u64, error: impl Into<String>) -> bool {
        if !self.visible || self.request_id != request_id {
            return false;
        }
        self.platform_scanning = false;
        self.error = Some(error.into());
        true
    }

    pub fn set_persist_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn finish(&mut self) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = false;
        self.platform_scanning = false;
        self.error = None;
        self.anchors.clear();
    }
}