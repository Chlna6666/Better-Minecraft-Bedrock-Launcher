use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_millis(500);

/// 插件依赖拓扑图解析器
#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    /// 插件 ID -> 依赖的其他插件 ID 集合
    dependencies: BTreeMap<String, BTreeSet<String>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册插件及其声明的依赖
    pub fn add_plugin(&mut self, plugin_id: String, dependencies: BTreeSet<String>) {
        self.dependencies.insert(plugin_id, dependencies);
    }

    /// 计算拓扑加载顺序（被依赖的插件先加载）
    /// 若存在循环依赖或缺失依赖，返回相应错误
    pub fn compute_load_order(&self) -> Result<Vec<String>> {
        let mut in_degrees: BTreeMap<String, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for plugin_id in self.dependencies.keys() {
            in_degrees.insert(plugin_id.clone(), 0);
        }

        for (plugin, deps) in &self.dependencies {
            for dep in deps {
                if !self.dependencies.contains_key(dep) {
                    bail!("plugin '{plugin}' depends on missing plugin '{dep}'");
                }
                dependents.entry(dep.clone()).or_default().push(plugin.clone());
                *in_degrees.entry(plugin.clone()).or_default() += 1;
            }
        }

        let mut queue: VecDeque<String> = in_degrees
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut load_order = Vec::with_capacity(self.dependencies.len());

        while let Some(current) = queue.pop_front() {
            load_order.push(current.clone());
            if let Some(downstream) = dependents.get(&current) {
                for next in downstream {
                    if let Some(deg) = in_degrees.get_mut(next) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(next.clone());
                        }
                    }
                }
            }
        }

        if load_order.len() != self.dependencies.len() {
            bail!("detected circular dependency among plugins");
        }

        Ok(load_order)
    }

    /// 计算安全卸载顺序（逆序：依赖者先卸载，被依赖者后卸载）
    pub fn compute_unload_order(&self) -> Result<Vec<String>> {
        let mut order = self.compute_load_order()?;
        order.reverse();
        Ok(order)
    }
}

/// 跨插件 Service 描述符
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceDescriptor {
    pub provider_plugin_id: String,
    pub service_name: String,
}

/// 跨插件服务调用请求
#[derive(Clone, Debug)]
pub struct ServiceRpcRequest {
    pub caller_id: String,
    pub service_name: String,
    pub method: String,
    pub payload: Vec<u8>,
}

/// 跨插件服务调用响应
#[derive(Clone, Debug)]
pub struct ServiceRpcResponse {
    pub success: bool,
    pub payload: Vec<u8>,
    pub error: Option<String>,
}

pub const MAX_RPC_DEPTH: usize = 8;

/// 防重入与循环调用 RPC 栈
#[derive(Clone, Debug, Default)]
pub struct PluginRpcCallStack {
    active_calls: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl PluginRpcCallStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// 尝试进入一个插件的 RPC 调用上下文，若超出深度或检测到环路则报错。
    pub fn enter(&self, target_plugin_id: &str) -> Result<CallStackGuard> {
        let mut calls = self.active_calls.borrow_mut();
        if calls.len() >= MAX_RPC_DEPTH {
            bail!("maximum RPC recursion depth ({MAX_RPC_DEPTH}) exceeded");
        }
        if calls.iter().any(|id| id == target_plugin_id) {
            bail!(
                "detected circular RPC call dependency: {:?} -> {}",
                *calls,
                target_plugin_id
            );
        }
        calls.push(target_plugin_id.to_string());
        Ok(CallStackGuard {
            stack: self.active_calls.clone(),
        })
    }

    pub fn current_depth(&self) -> usize {
        self.active_calls.borrow().len()
    }
}

#[derive(Debug)]
pub struct CallStackGuard {
    stack: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl Drop for CallStackGuard {
    fn drop(&mut self) {
        self.stack.borrow_mut().pop();
    }
}

/// 插件服务注册表与总线
#[derive(Clone, Debug, Default)]
pub struct PluginServiceRegistry {
    services: BTreeMap<String, ServiceDescriptor>,
}

impl PluginServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册由某插件提供的命名服务
    pub fn register_service(&mut self, provider_plugin_id: String, service_name: String) -> Result<()> {
        if let Some(existing) = self.services.get(&service_name) {
            bail!(
                "service '{service_name}' already registered by plugin '{}'",
                existing.provider_plugin_id
            );
        }
        self.services.insert(
            service_name.clone(),
            ServiceDescriptor {
                provider_plugin_id,
                service_name,
            },
        );
        Ok(())
    }

    /// 查找服务提供方
    pub fn find_service(&self, service_name: &str) -> Option<&ServiceDescriptor> {
        self.services.get(service_name)
    }

    /// 注销插件提供的全部服务
    pub fn unregister_by_plugin(&mut self, plugin_id: &str) {
        self.services.retain(|_, desc| desc.provider_plugin_id != plugin_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_graph_load_order() {
        let mut graph = DependencyGraph::new();
        // A -> B -> C
        graph.add_plugin("C".into(), BTreeSet::new());
        graph.add_plugin("B".into(), BTreeSet::from(["C".into()]));
        graph.add_plugin("A".into(), BTreeSet::from(["B".into()]));

        let order = graph.compute_load_order().expect("valid DAG");
        assert_eq!(order, vec!["C", "B", "A"]);

        let unload = graph.compute_unload_order().expect("valid DAG");
        assert_eq!(unload, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_circular_dependency_detected() {
        let mut graph = DependencyGraph::new();
        // A -> B -> A
        graph.add_plugin("A".into(), BTreeSet::from(["B".into()]));
        graph.add_plugin("B".into(), BTreeSet::from(["A".into()]));

        let err = graph.compute_load_order().unwrap_err();
        assert!(err.to_string().contains("circular dependency"));
    }

    #[test]
    fn test_missing_dependency_detected() {
        let mut graph = DependencyGraph::new();
        graph.add_plugin("A".into(), BTreeSet::from(["NonExistent".into()]));

        let err = graph.compute_load_order().unwrap_err();
        assert!(err.to_string().contains("missing plugin"));
    }

    #[test]
    fn test_service_registry() {
        let mut registry = PluginServiceRegistry::new();
        registry.register_service("audio_plugin".into(), "audio_service".into()).unwrap();
        assert!(registry.register_service("other".into(), "audio_service".into()).is_err());
        assert_eq!(
            registry.find_service("audio_service").unwrap().provider_plugin_id,
            "audio_plugin"
        );
        registry.unregister_by_plugin("audio_plugin");
        assert!(registry.find_service("audio_service").is_none());
    }

    #[test]
    fn test_rpc_call_stack_guard_and_cycle_prevention() {
        let stack = PluginRpcCallStack::new();
        assert_eq!(stack.current_depth(), 0);

        {
            let _g1 = stack.enter("plugin_a").expect("enter A");
            assert_eq!(stack.current_depth(), 1);

            {
                let _g2 = stack.enter("plugin_b").expect("enter B");
                assert_eq!(stack.current_depth(), 2);

                // Circular call: B -> A
                let err = stack.enter("plugin_a").unwrap_err();
                assert!(err.to_string().contains("circular RPC"));

                // Self call: B -> B
                let err = stack.enter("plugin_b").unwrap_err();
                assert!(err.to_string().contains("circular RPC"));
            }
            assert_eq!(stack.current_depth(), 1);
        }
        assert_eq!(stack.current_depth(), 0);
    }

    #[test]
    fn test_rpc_call_stack_max_depth() {
        let stack = PluginRpcCallStack::new();
        let mut guards = Vec::new();

        for i in 0..MAX_RPC_DEPTH {
            guards.push(stack.enter(&format!("plugin_{i}")).expect("within depth"));
        }
        assert_eq!(stack.current_depth(), MAX_RPC_DEPTH);

        // Exceeds max depth
        let err = stack.enter("overflow_plugin").unwrap_err();
        assert!(err.to_string().contains("maximum RPC recursion depth"));
    }
}
