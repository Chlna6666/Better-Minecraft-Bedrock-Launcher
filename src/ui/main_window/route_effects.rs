use super::*;
use crate::ui::state::navigation::NavState;

impl MainWindowView {
    fn sync_route_navigation(&mut self, route: &RouteTarget, cx: &mut Context<Self>) {
        let index = route.visual_index(cx);
        cx.update_global(|nav: &mut NavState, _cx| {
            nav.confirm_route(index);
        });
    }

    fn on_route_changed_cleanup(
        &mut self,
        previous_route: Option<&RouteTarget>,
        route: &RouteTarget,
        cx: &mut Context<Self>,
    ) {
        if previous_route == Some(&RouteTarget::Builtin(AppRoute::Download))
            && !matches!(route, RouteTarget::Builtin(AppRoute::Download))
        {
            cx.update_global(
                |state: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                    state.curseforge_mod_page_open = false;
                    state.curseforge_mod_page_loading = false;
                    state.curseforge_mod_page_error = None;
                    state.curseforge_mod_page_mod_id = None;
                    state.curseforge_mod_page_mod = None;
                    state.set_curseforge_mod_page_description(SharedString::from(""));
                    state.curseforge_install_open = false;
                },
            );
        }

        self.release_inactive_target_resources(route, cx);
    }

    fn on_route_changed_registry(&mut self, route: &RouteTarget, cx: &mut Context<Self>) {
        self.ensure_page_view_for_target(route, cx);
        self.sync_page_active_flags(route, cx);
    }

    fn on_route_changed_bootstrap(&mut self, route: &RouteTarget, cx: &mut Context<Self>) {
        let RouteTarget::Builtin(route) = route else {
            return;
        };

        if *route == AppRoute::Settings {
            self.ensure_settings_loaded(cx);
        }

        self.ensure_route_page_data_loaded(*route, cx);
    }

    pub(super) fn handle_route_change_without_window(
        &mut self,
        route: RouteTarget,
        cx: &mut Context<Self>,
    ) {
        let previous_route = self.last_route_for_side_effects.take();

        if previous_route.as_ref() != Some(&route) {
            self.sync_route_navigation(&route, cx);
            self.on_route_changed_cleanup(previous_route.as_ref(), &route, cx);
            self.on_route_changed_registry(&route, cx);
            self.on_route_changed_bootstrap(&route, cx);
            let now = Instant::now();
            let debug_enabled = cx.global::<DebugState>().enabled;
            let update_render_state = self.read_update_render_state(now, debug_enabled, cx);
            if self.sync_background_animation_policy_for_route(&route, &update_render_state, cx) {
                cx.notify();
            }
        } else {
            if let RouteTarget::Builtin(route) = route {
                self.ensure_route_page_data_loaded(route, cx);
            }
        }

        self.last_route_for_side_effects = Some(route);
    }
}
