use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 宿主广播的路由变化事件订阅名。
pub const ROUTE_CHANGED_EVENT: &str = "route-changed";

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

impl HostEvent {
    /// 事件级联预算里用于标识一次投递的名称。
    ///
    /// 全局事件使用订阅名，路由变化使用 [`ROUTE_CHANGED_EVENT`]，动作事件使用 action id，
    /// 这样同一插件重复收到同一事件可以被稳定识别。
    pub fn cascade_name(&self) -> &str {
        match &self.kind {
            HostEventKind::Global { name, .. } => name,
            HostEventKind::RouteChanged { .. } => ROUTE_CHANGED_EVENT,
            HostEventKind::Action { action_id, .. } => action_id,
        }
    }
}

/// 单次顶层宿主事件派发允许的级联预算。
///
/// 插件在事件处理期间可以再次 `emit_event`，宿主必须限制这种级联：两个插件互相触发事件
/// 而没有预算时，派发会在主线程无限展开。预算按目标插件加事件的投递次数计算，因此
/// 同一插件对不同事件、不同插件对同一事件仍然可以正常收到投递。
#[derive(Debug)]
pub struct EventCascade {
    deliveries: usize,
    visits: Vec<u64>,
}

impl Default for EventCascade {
    fn default() -> Self {
        Self::new()
    }
}

impl EventCascade {
    /// 单次顶层派发允许的插件事件投递总数。
    pub const MAX_DELIVERIES: usize = 128;
    /// 同一插件与事件的组合在一次级联中允许的重复投递次数。
    pub const MAX_TARGET_REPEATS: usize = 4;

    pub fn new() -> Self {
        Self {
            deliveries: 0,
            visits: Vec::new(),
        }
    }

    /// 记录一次向 `plugin_id` 投递 `event_name` 事件。
    ///
    /// 返回 false 表示这次投递会超出级联预算，调用方必须跳过该目标，避免两个插件通过
    /// `emit_event` 互相触发造成无界派发。
    pub fn try_deliver(&mut self, plugin_id: &str, event_name: &str) -> bool {
        if self.deliveries >= Self::MAX_DELIVERIES {
            return false;
        }
        let visit = visit_key(plugin_id, event_name);
        let repeats = self.visits.iter().filter(|entry| **entry == visit).count();
        if repeats >= Self::MAX_TARGET_REPEATS {
            return false;
        }
        self.visits.push(visit);
        self.deliveries += 1;
        true
    }

    /// 本次级联已经完成的投递数。
    pub fn deliveries(&self) -> usize {
        self.deliveries
    }

    /// 投递总数是否已经用尽；用尽后不应再把新事件放进派发队列。
    pub fn is_exhausted(&self) -> bool {
        self.deliveries >= Self::MAX_DELIVERIES
    }
}

/// 用 64 位哈希标识插件与事件的组合，避免只读的预算检查分配字符串。
///
/// 级联内的投递数受 [`EventCascade::MAX_DELIVERIES`] 限制，哈希碰撞带来误判的概率可以忽略。
fn visit_key(plugin_id: &str, event_name: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    plugin_id.hash(&mut hasher);
    event_name.hash(&mut hasher);
    hasher.finish()
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

    #[test]
    fn cascade_stops_plugin_ping_pong() {
        let mut cascade = EventCascade::new();
        let mut deliveries = 0;
        for _ in 0..256 {
            // 两个插件互相 emit_event 时，同一 (插件, 事件) 组合必须被限流。
            if !cascade.try_deliver("alpha", "ping") {
                break;
            }
            deliveries += 1;
            if !cascade.try_deliver("beta", "pong") {
                break;
            }
            deliveries += 1;
        }
        assert_eq!(deliveries, EventCascade::MAX_TARGET_REPEATS * 2);
    }

    #[test]
    fn cascade_limits_totals_across_distinct_targets() {
        let mut cascade = EventCascade::new();
        let mut delivered = 0;
        'outer: for plugin in 0..64 {
            for event in 0..64 {
                let plugin_id = format!("plugin-{plugin}");
                let event_name = format!("event-{event}");
                if !cascade.try_deliver(&plugin_id, &event_name) {
                    break 'outer;
                }
                delivered += 1;
            }
        }
        assert_eq!(delivered, EventCascade::MAX_DELIVERIES);
        assert!(cascade.is_exhausted());
    }

    #[test]
    fn cascade_allows_distinct_events_for_one_plugin() {
        let mut cascade = EventCascade::new();
        for event in 0..EventCascade::MAX_TARGET_REPEATS {
            assert!(cascade.try_deliver("alpha", &format!("event-{event}")));
        }
        assert_eq!(cascade.deliveries(), EventCascade::MAX_TARGET_REPEATS);
    }

    #[test]
    fn cascade_name_covers_every_event_kind() {
        let action = HostEvent {
            plugin_id: Some("alpha".to_string()),
            page_id: None,
            kind: HostEventKind::Action {
                action_id: "refresh".to_string(),
                value: None,
            },
        };
        assert_eq!(action.cascade_name(), "refresh");

        let route = HostEvent {
            plugin_id: None,
            page_id: None,
            kind: HostEventKind::RouteChanged {
                path: "/settings".to_string(),
            },
        };
        assert_eq!(route.cascade_name(), ROUTE_CHANGED_EVENT);

        let global = HostEvent {
            plugin_id: None,
            page_id: None,
            kind: HostEventKind::Global {
                name: "download-finished".to_string(),
                payload: String::new(),
            },
        };
        assert_eq!(global.cascade_name(), "download-finished");
    }
}
