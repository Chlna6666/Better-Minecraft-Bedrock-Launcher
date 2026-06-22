use std::cmp::Ordering;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginPageRegistration {
    pub plugin_id: String,
    pub page_id: String,
    pub title: String,
    pub navigation: Option<PluginNavigationEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginNavigationEntry {
    pub label: String,
    pub icon: Option<String>,
    pub order: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum InjectionSlot {
    MainRootOverlay,
    PageHeader,
    PageBody,
    HomeSidebar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginInjectionRegistration {
    pub plugin_id: String,
    pub slot: InjectionSlot,
    pub page: Option<String>,
    pub priority: i32,
    pub layout: Option<InjectionLayout>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactBehavior {
    None,
    Scroll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InjectionLayout {
    pub preferred_width: Option<u16>,
    pub min_width: Option<u16>,
    pub max_width: Option<u16>,
    pub max_height: Option<u16>,
    pub priority: i32,
    pub compact_behavior: CompactBehavior,
}

impl Default for InjectionLayout {
    fn default() -> Self {
        Self {
            preferred_width: None,
            min_width: None,
            max_width: None,
            max_height: None,
            priority: 0,
            compact_behavior: CompactBehavior::None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostEventKind {
    RouteChanged {
        path: String,
    },
    Action {
        action_id: String,
        value: Option<String>,
    },
    Global {
        name: String,
        payload: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostEvent {
    pub plugin_id: Option<String>,
    pub page_id: Option<String>,
    pub kind: HostEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InjectionRenderRequest {
    pub slot: InjectionSlot,
    pub page: Option<String>,
}

pub fn sort_injections(registrations: &mut [PluginInjectionRegistration]) {
    registrations.sort_by(|left, right| {
        left.slot
            .cmp(&right.slot)
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.plugin_id.cmp(&right.plugin_id))
            .then_with(|| option_string_cmp(&left.page, &right.page))
    });
}

fn option_string_cmp(left: &Option<String>, right: &Option<String>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injections_sort_by_slot_priority_and_plugin_id() {
        let mut registrations = vec![
            PluginInjectionRegistration {
                plugin_id: "zeta".to_string(),
                slot: InjectionSlot::PageBody,
                page: None,
                priority: 10,
                layout: None,
            },
            PluginInjectionRegistration {
                plugin_id: "alpha".to_string(),
                slot: InjectionSlot::PageBody,
                page: None,
                priority: 10,
                layout: None,
            },
            PluginInjectionRegistration {
                plugin_id: "beta".to_string(),
                slot: InjectionSlot::PageHeader,
                page: None,
                priority: 50,
                layout: None,
            },
            PluginInjectionRegistration {
                plugin_id: "gamma".to_string(),
                slot: InjectionSlot::PageBody,
                page: None,
                priority: 0,
                layout: None,
            },
        ];

        sort_injections(&mut registrations);

        let ids = registrations
            .into_iter()
            .map(|registration| registration.plugin_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["beta", "gamma", "alpha", "zeta"]);
    }
}
