use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tinywasm::Trap;
use tracing::warn;

pub const DEFAULT_SOFT_BUDGET_BYTES: usize = 8 * 1024 * 1024; // 8 MiB 软预算
pub const DEFAULT_HARD_LIMIT_BYTES: usize = 32 * 1024 * 1024; // 32 MiB 硬熔断上限

/// 插件内存预算配置
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginMemoryBudget {
    pub soft_limit_bytes: usize,
    pub hard_limit_bytes: usize,
}

impl Default for PluginMemoryBudget {
    fn default() -> Self {
        Self {
            soft_limit_bytes: DEFAULT_SOFT_BUDGET_BYTES,
            hard_limit_bytes: DEFAULT_HARD_LIMIT_BYTES,
        }
    }
}

/// TinyWasm 资源限制器（硬上限防护熔断器）
#[derive(Clone, Debug)]
pub struct PluginResourceLimiter {
    pub max_memory_bytes: usize,
    pub max_table_elements: usize,
    pub observed_pages: Arc<AtomicUsize>,
}

impl PluginResourceLimiter {
    pub fn new(max_memory_bytes: usize) -> Self {
        Self {
            max_memory_bytes,
            max_table_elements: 10_000,
            observed_pages: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Default for PluginResourceLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_HARD_LIMIT_BYTES)
    }
}

impl tinywasm::ResourceLimiter for PluginResourceLimiter {
    fn memory_growing(
        &self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, Trap> {
        if desired > self.max_memory_bytes {
            warn!(
                current_bytes = current,
                desired_bytes = desired,
                max_bytes = self.max_memory_bytes,
                "plugin requested memory grow exceeds hard limit; rejecting allocation"
            );
            return Ok(false);
        }
        let pages = desired.div_ceil(64 * 1024);
        self.observed_pages.store(pages, Ordering::Relaxed);
        Ok(true)
    }

    fn table_growing(
        &self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, Trap> {
        if desired > self.max_table_elements {
            warn!(
                desired_elements = desired,
                max_elements = self.max_table_elements,
                "plugin table growth exceeds hard limit; rejecting growth"
            );
            return Ok(false);
        }
        Ok(true)
    }
}

/// 宿主内存健康状态评估
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetEvaluation {
    /// 位于 8 MiB 软预算内，状态健康
    Normal,
    /// 超过 8 MiB 但在允许工况内，若系统紧张建议回收冷缓存
    TrimCaches,
    /// 长期超额或空闲，建议进行安全休眠（Hibernate）释放 Store 线性内存
    Hibernate,
}

/// 评估插件当前归属内存
pub fn evaluate_plugin_budget(
    budget: &PluginMemoryBudget,
    attributed_bytes: usize,
    is_idle: bool,
    system_memory_pressure: bool,
) -> BudgetEvaluation {
    if attributed_bytes <= budget.soft_limit_bytes {
        BudgetEvaluation::Normal
    } else if system_memory_pressure && is_idle {
        BudgetEvaluation::Hibernate
    } else {
        BudgetEvaluation::TrimCaches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_evaluation_under_soft_limit() {
        let budget = PluginMemoryBudget::default();
        let eval = evaluate_plugin_budget(&budget, 4 * 1024 * 1024, false, false);
        assert_eq!(eval, BudgetEvaluation::Normal);
    }

    #[test]
    fn test_budget_evaluation_over_soft_limit_active() {
        let budget = PluginMemoryBudget::default();
        let eval = evaluate_plugin_budget(&budget, 10 * 1024 * 1024, false, false);
        assert_eq!(eval, BudgetEvaluation::TrimCaches);
    }

    #[test]
    fn test_budget_evaluation_over_soft_limit_idle_under_pressure() {
        let budget = PluginMemoryBudget::default();
        let eval = evaluate_plugin_budget(&budget, 10 * 1024 * 1024, true, true);
        assert_eq!(eval, BudgetEvaluation::Hibernate);
    }

    #[test]
    fn test_resource_limiter_rejects_above_hard_limit() {
        use tinywasm::ResourceLimiter;
        let limiter = PluginResourceLimiter::new(16 * 1024 * 1024);
        assert_eq!(limiter.memory_growing(0, 8 * 1024 * 1024, None).ok(), Some(true));
        assert_eq!(limiter.memory_growing(8 * 1024 * 1024, 20 * 1024 * 1024, None).ok(), Some(false));
    }
}
