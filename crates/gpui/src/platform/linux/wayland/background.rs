use std::{cell::RefCell, rc::Rc};

use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_protocols_plasma::blur::client::org_kde_kwin_blur_manager::OrgKdeKwinBlurManager;

use super::client::WaylandClientStatePtr;
use crate::WindowBackgroundCapabilities;

/// Shared registry/capability state: window-local Globals clones observe revocation together.
pub(super) struct BackgroundEffects {
    pub(super) manager: Option<ExtBackgroundEffectManagerV1>,
    pub(super) kde_manager: Option<OrgKdeKwinBlurManager>,
    pub(super) blur_supported: bool,
    manager_name: Option<u32>,
    kde_manager_name: Option<u32>,
}

impl BackgroundEffects {
    pub(super) fn bind(
        globals: &GlobalList,
        qh: &QueueHandle<WaylandClientStatePtr>,
    ) -> Rc<RefCell<Self>> {
        let names = globals.contents().clone_list();
        Rc::new(RefCell::new(Self {
            manager: globals.bind(qh, 1..=1, ()).ok(),
            kde_manager: globals.bind(qh, 1..=1, ()).ok(),
            blur_supported: false,
            manager_name: names
                .iter()
                .find(|global| global.interface == "ext_background_effect_manager_v1")
                .map(|global| global.name),
            kde_manager_name: names
                .iter()
                .find(|global| global.interface == "org_kde_kwin_blur_manager")
                .map(|global| global.name),
        }))
    }

    pub(super) fn capabilities(&self) -> WindowBackgroundCapabilities {
        WindowBackgroundCapabilities {
            blurred: (self.blur_supported && self.manager.as_ref().is_some_and(Proxy::is_alive))
                || self.kde_manager.as_ref().is_some_and(Proxy::is_alive),
            ..WindowBackgroundCapabilities::default()
        }
    }

    pub(super) fn global_added(
        &mut self,
        registry: &WlRegistry,
        name: u32,
        interface: &str,
        qh: &QueueHandle<WaylandClientStatePtr>,
    ) -> bool {
        match interface {
            "ext_background_effect_manager_v1" if self.manager.is_none() => {
                self.manager = Some(registry.bind(name, 1, qh, ()));
                self.manager_name = Some(name);
                true
            }
            "org_kde_kwin_blur_manager" if self.kde_manager.is_none() => {
                self.kde_manager = Some(registry.bind(name, 1, qh, ()));
                self.kde_manager_name = Some(name);
                true
            }
            _ => false,
        }
    }

    pub(super) fn global_removed(&mut self, name: u32) -> bool {
        if self.manager_name == Some(name) {
            if let Some(manager) = self.manager.take() {
                manager.destroy();
            }
            self.manager_name = None;
            self.blur_supported = false;
            true
        } else if self.kde_manager_name == Some(name) {
            if let Some(manager) = self.kde_manager.take() {
                manager.release();
            }
            self.kde_manager_name = None;
            true
        } else {
            false
        }
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for WaylandClientStatePtr {
    fn event(
        state: &mut Self,
        manager: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            let supported = matches!(flags, WEnum::Value(flags) if flags.contains(ext_background_effect_manager_v1::Capability::Blur));
            state.background_effect_capabilities_changed(manager, supported);
        }
    }
}

delegate_noop!(WaylandClientStatePtr: ignore ExtBackgroundEffectSurfaceV1);
