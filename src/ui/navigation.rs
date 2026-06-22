use crate::ui::state::navigation::NavState;
use gpui::BorrowAppContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppRoute {
    Home,
    Download,
    Manage,
    Tools,
    Tasks,
    Settings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteTarget {
    Builtin(AppRoute),
    Plugin { plugin_id: String, page_id: String },
}

impl Default for AppRoute {
    fn default() -> Self {
        Self::Home
    }
}

impl AppRoute {
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Home,
            1 => Self::Download,
            2 => Self::Manage,
            3 => Self::Tools,
            4 => Self::Tasks,
            _ => Self::Settings,
        }
    }

    #[must_use]
    pub fn pathname(self) -> &'static str {
        match self {
            Self::Home => "/",
            Self::Download => "/download",
            Self::Manage => "/list",
            Self::Tools => "/tools/online",
            Self::Tasks => "/tasks",
            Self::Settings => "/settings",
        }
    }

    #[must_use]
    pub fn from_pathname(pathname: &str) -> Self {
        match RouteTarget::from_pathname(pathname) {
            RouteTarget::Builtin(route) => route,
            RouteTarget::Plugin { .. } => Self::Home,
        }
    }

    #[must_use]
    pub fn from_builtin_pathname(pathname: &str) -> Self {
        match pathname {
            "/" => Self::Home,
            "/download" => Self::Download,
            "/versions" => Self::Manage,
            "/manage" => Self::Manage,
            "/tools" => Self::Tools,
            "/list" => Self::Manage,
            "/tasks" => Self::Tasks,
            "/settings" => Self::Settings,
            _ if pathname.starts_with("/manage/") => Self::Manage,
            _ if pathname.starts_with("/tools/") => Self::Tools,
            _ => Self::default(),
        }
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "启动",
            Self::Download => "下载",
            Self::Manage => "管理",
            Self::Tools => "工具",
            Self::Tasks => "任务",
            Self::Settings => "设置",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Home => 0,
            Self::Download => 1,
            Self::Manage => 2,
            Self::Tools => 3,
            Self::Tasks => 4,
            Self::Settings => 5,
        }
    }
}

impl RouteTarget {
    #[must_use]
    pub fn from_pathname(pathname: &str) -> Self {
        if let Some((plugin_id, page_id)) = parse_plugin_pathname(pathname) {
            return Self::Plugin { plugin_id, page_id };
        }

        Self::Builtin(AppRoute::from_builtin_pathname(pathname))
    }

    #[must_use]
    pub fn pathname(&self) -> String {
        match self {
            Self::Builtin(route) => route.pathname().to_string(),
            Self::Plugin { plugin_id, page_id } => {
                format!("/plugins/{plugin_id}/{page_id}")
            }
        }
    }

    #[must_use]
    pub fn visual_index(&self, cx: &gpui::App) -> usize {
        match self {
            Self::Builtin(route) => route.index(),
            Self::Plugin { plugin_id, page_id } => {
                let pages = cx
                    .global::<crate::plugins::runtime::PluginRegistry>()
                    .pages();
                let mut plugin_pages = pages
                    .into_iter()
                    .filter(|page| page.navigation.is_some())
                    .collect::<Vec<_>>();
                plugin_pages.sort_by(|left, right| {
                    let left_nav = left.navigation.as_ref();
                    let right_nav = right.navigation.as_ref();
                    left_nav
                        .map_or(0, |nav| nav.order)
                        .cmp(&right_nav.map_or(0, |nav| nav.order))
                        .then_with(|| left.plugin_id.cmp(&right.plugin_id))
                        .then_with(|| left.page_id.cmp(&right.page_id))
                });

                plugin_pages
                    .iter()
                    .position(|page| &page.plugin_id == plugin_id && &page.page_id == page_id)
                    .map_or(6, |index| 6 + index)
            }
        }
    }
}

#[must_use]
pub fn parse_plugin_pathname(pathname: &str) -> Option<(String, String)> {
    let suffix = pathname.strip_prefix("/plugins/")?;
    let mut parts = suffix.split('/');
    let plugin_id = parts.next()?.trim();
    let page_id = parts.next()?.trim();
    if plugin_id.is_empty() || page_id.is_empty() || parts.next().is_some() {
        return None;
    }

    if crate::plugins::manifest::validate_plugin_id(plugin_id).is_err()
        || crate::plugins::manifest::validate_plugin_id(page_id).is_err()
    {
        return None;
    }

    Some((plugin_id.to_string(), page_id.to_string()))
}

#[must_use]
pub fn current_route(cx: &gpui::App) -> AppRoute {
    match current_route_target(cx) {
        RouteTarget::Builtin(route) => route,
        RouteTarget::Plugin { .. } => AppRoute::Home,
    }
}

#[must_use]
pub fn current_route_target(cx: &gpui::App) -> RouteTarget {
    let location = gpui_router::use_location(cx);
    RouteTarget::from_pathname(&location.pathname)
}

pub fn set_route(cx: &mut gpui::App, route: AppRoute) {
    let now = std::time::Instant::now();
    cx.update_global(|nav: &mut NavState, _cx| {
        nav.start_pill_animation(route.index(), now);
    });
    navigate_to(cx, route);
}

pub fn navigate_to(cx: &mut gpui::App, route: AppRoute) {
    navigate_target(cx, RouteTarget::Builtin(route));
}

pub fn navigate_plugin(cx: &mut gpui::App, plugin_id: String, page_id: String) {
    navigate_target(cx, RouteTarget::Plugin { plugin_id, page_id });
}

pub fn navigate_target(cx: &mut gpui::App, target: RouteTarget) {
    let path = target.pathname();
    let visual_index = target.visual_index(cx);
    let now = std::time::Instant::now();
    cx.update_global(|nav: &mut NavState, _cx| {
        nav.start_pill_animation(visual_index, now);
    });
    {
        let mut navigate = gpui_router::use_navigate(cx);
        navigate(path.clone().into());
    }
    crate::plugins::runtime::dispatch_route_changed(cx, path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_route() {
        assert_eq!(
            RouteTarget::from_pathname("/plugins/hello-wasm/main"),
            RouteTarget::Plugin {
                plugin_id: "hello-wasm".to_string(),
                page_id: "main".to_string(),
            }
        );
    }

    #[test]
    fn rejects_malformed_plugin_route() {
        assert_eq!(
            RouteTarget::from_pathname("/plugins/Hello/main"),
            RouteTarget::Builtin(AppRoute::Home)
        );
        assert_eq!(
            RouteTarget::from_pathname("/plugins/hello/main/extra"),
            RouteTarget::Builtin(AppRoute::Home)
        );
    }

    #[test]
    fn keeps_builtin_route_parsing() {
        assert_eq!(
            RouteTarget::from_pathname("/download"),
            RouteTarget::Builtin(AppRoute::Download)
        );
        assert_eq!(
            RouteTarget::from_pathname("/tools/online"),
            RouteTarget::Builtin(AppRoute::Tools)
        );
    }
}
