use crate::ui::state::navigation::NavState;
use gpui::BorrowAppContext;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppRoute {
    Home,
    Download,
    Manage,
    Tasks,
    Settings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteTarget {
    Builtin(AppRoute),
    Plugin { plugin_id: String, page_id: String },
    Tools,
    Tool { plugin_id: String, page_id: String },
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
            3 => Self::Tasks,
            _ => Self::Settings,
        }
    }

    #[must_use]
    pub fn pathname(self) -> &'static str {
        match self {
            Self::Home => "/",
            Self::Download => "/download",
            Self::Manage => "/list",
            Self::Tasks => "/tasks",
            Self::Settings => "/settings",
        }
    }

    #[must_use]
    pub fn from_pathname(pathname: &str) -> Self {
        match RouteTarget::from_pathname(pathname) {
            RouteTarget::Builtin(route) => route,
            _ => Self::Home,
        }
    }

    #[must_use]
    pub fn from_builtin_pathname(pathname: &str) -> Self {
        match pathname {
            "/" => Self::Home,
            "/download" => Self::Download,
            "/versions" => Self::Manage,
            "/manage" => Self::Manage,
            "/list" => Self::Manage,
            "/tasks" => Self::Tasks,
            "/settings" => Self::Settings,
            _ if pathname.starts_with("/manage/") => Self::Manage,
            _ => Self::default(),
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Home => 0,
            Self::Download => 1,
            Self::Manage => 2,
            Self::Tasks => 3,
            Self::Settings => 4,
        }
    }
}

impl RouteTarget {
    #[must_use]
    pub fn from_pathname(pathname: &str) -> Self {
        if matches!(pathname, "/tools" | "/online" | "/tools/online") {
            return Self::Tools;
        }
        if let Some((plugin_id, page_id)) = pathname.strip_prefix("/tools/").and_then(parse_plugin_page) {
            return Self::Tool { plugin_id, page_id };
        }
        if let Some((plugin_id, page_id)) = parse_plugin_pathname(pathname) {
            return Self::Plugin { plugin_id, page_id };
        }

        Self::Builtin(AppRoute::from_builtin_pathname(pathname))
    }

    #[must_use]
    pub fn pathname(&self) -> Cow<'static, str> {
        match self {
            Self::Builtin(route) => Cow::Borrowed(route.pathname()),
            Self::Plugin { plugin_id, page_id } => {
                Cow::Owned(format!("/plugins/{plugin_id}/{page_id}"))
            }
            Self::Tools => Cow::Borrowed("/tools"),
            Self::Tool { plugin_id, page_id } => Cow::Owned(format!("/tools/{plugin_id}/{page_id}")),
        }
    }

    #[must_use]
    pub fn visual_index(&self, cx: &gpui::App) -> usize {
        let tools_visible = tools_visible(cx);
        match self {
            Self::Builtin(route) => route.index() + usize::from(tools_visible && route.index() >= AppRoute::Tasks.index()),
            Self::Tool { .. } | Self::Tools => AppRoute::Tasks.index(),
            Self::Plugin { .. } => AppRoute::Home.index(),
        }
    }
}

pub(crate) fn tools_visible(cx: &gpui::App) -> bool {
    cx.try_global::<crate::plugins::runtime::PluginRegistry>()
        .is_some_and(|registry| registry.has_tools())
}

pub(crate) fn resolve_tool_route(target: RouteTarget, cx: &gpui::App) -> RouteTarget {
    let Some(registry) = cx.try_global::<crate::plugins::runtime::PluginRegistry>() else {
        return target;
    };
    match target {
        RouteTarget::Tool { ref plugin_id, ref page_id }
            if registry.is_tool(plugin_id, page_id) => target,
        RouteTarget::Tool { .. } | RouteTarget::Tools => registry.tool_pages()
            .into_iter().next()
            .map_or(RouteTarget::Builtin(AppRoute::Home), |page| RouteTarget::Tool {
                plugin_id: page.plugin_id,
                page_id: page.page_id,
            }),
        RouteTarget::Plugin { plugin_id, page_id } if registry.is_tool(&plugin_id, &page_id) => {
            RouteTarget::Tool { plugin_id, page_id }
        }
        _ => target,
    }
}

#[must_use]
pub fn parse_plugin_pathname(pathname: &str) -> Option<(String, String)> {
    parse_plugin_page(pathname.strip_prefix("/plugins/")?)
}

fn parse_plugin_page(suffix: &str) -> Option<(String, String)> {
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
        _ => AppRoute::Home,
    }
}

#[must_use]
pub fn current_route_target(cx: &gpui::App) -> RouteTarget {
    let location = gpui_router::use_location(cx);
    resolve_tool_route(RouteTarget::from_pathname(&location.pathname), cx)
}

pub fn set_route(cx: &mut gpui::App, route: AppRoute) {
    navigate_to(cx, route);
}

pub fn navigate_to(cx: &mut gpui::App, route: AppRoute) {
    navigate_target(cx, RouteTarget::Builtin(route));
}

pub fn navigate_plugin(cx: &mut gpui::App, plugin_id: String, page_id: String) {
    navigate_target(cx, RouteTarget::Plugin { plugin_id, page_id });
}

pub fn navigate_target(cx: &mut gpui::App, target: RouteTarget) {
    let target = resolve_tool_route(target, cx);
    let path = target.pathname().into_owned();
    let visual_index = target.visual_index(cx);
    cx.update_global(|nav: &mut NavState, _cx| {
        nav.start_pill_animation(visual_index);
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
            RouteTarget::Tools
        );
    }

    #[test]
    fn tool_paths_keep_both_namespaces() {
        let tool = RouteTarget::Tool { plugin_id: "first-plugin".into(), page_id: "convert".into() };
        assert_eq!(RouteTarget::from_pathname(&tool.pathname()), tool);
        assert_eq!(RouteTarget::from_pathname("/tools"), RouteTarget::Tools);
    }
}
