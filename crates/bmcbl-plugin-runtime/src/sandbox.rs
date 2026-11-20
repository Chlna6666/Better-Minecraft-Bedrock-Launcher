//! 沙箱资源限制与存储键校验。
//!
//! 内存硬熔断（32 MiB）/软预算（8 MiB）与表元素上限沿用旧 budget 语义。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tracing::warn;

use crate::PluginError;

pub const DEFAULT_SOFT_BUDGET_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_HARD_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const MAX_TABLE_ELEMENTS: usize = 10_000;
/// 存储 key 的字节上限（与旧 ABI 一致）。
pub const STORAGE_KEY_MAX_BYTES: usize = 128;

/// wasmtime 资源限制器（硬上限防护熔断器）。
#[derive(Clone, Debug)]
pub struct PluginResourceLimiter {
    pub max_memory_bytes: usize,
    observed_bytes: Arc<AtomicUsize>,
    pending_growth: usize,
}

impl PluginResourceLimiter {
    #[must_use]
    pub fn new(max_memory_bytes: usize) -> Self {
        Self {
            max_memory_bytes,
            observed_bytes: Arc::new(AtomicUsize::new(0)),
            pending_growth: 0,
        }
    }

    #[must_use]
    pub fn observed_bytes(&self) -> usize {
        self.observed_bytes.load(Ordering::Relaxed)
    }
}

impl Default for PluginResourceLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_HARD_LIMIT_BYTES)
    }
}

impl wasmtime::ResourceLimiter for PluginResourceLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        let growth = desired.saturating_sub(current);
        let total = self.observed_bytes().saturating_add(growth);
        if total > self.max_memory_bytes {
            self.pending_growth = 0;
            warn!(
                current_bytes = current,
                desired_bytes = desired,
                max_bytes = self.max_memory_bytes,
                "plugin requested memory grow exceeds hard limit; rejecting allocation"
            );
            return Ok(false);
        }
        self.pending_growth = growth;
        self.observed_bytes.store(total, Ordering::Relaxed);
        Ok(true)
    }

    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> Result<(), wasmtime::Error> {
        self.observed_bytes
            .fetch_sub(std::mem::take(&mut self.pending_growth), Ordering::Relaxed);
        Ok(())
    }

    fn memories(&self) -> usize {
        8
    }

    fn tables(&self) -> usize {
        16
    }

    fn instances(&self) -> usize {
        64
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        if desired > MAX_TABLE_ELEMENTS {
            warn!(
                desired_elements = desired,
                max_elements = MAX_TABLE_ELEMENTS,
                "plugin table growth exceeds hard limit; rejecting growth"
            );
            return Ok(false);
        }
        Ok(true)
    }
}

/// 宿主内存健康状态评估。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetEvaluation {
    /// 位于软预算内，状态健康
    Normal,
    /// 超过软预算但在允许工况内，若系统紧张建议回收冷缓存
    TrimCaches,
    /// 系统紧张且空闲，建议休眠释放 Store 线性内存
    Hibernate,
}

/// 评估插件当前归属内存。
#[must_use]
pub fn evaluate_plugin_budget(
    soft_limit_bytes: usize,
    attributed_bytes: usize,
    is_idle: bool,
    system_memory_pressure: bool,
) -> BudgetEvaluation {
    if attributed_bytes <= soft_limit_bytes {
        BudgetEvaluation::Normal
    } else if system_memory_pressure && is_idle {
        BudgetEvaluation::Hibernate
    } else {
        BudgetEvaluation::TrimCaches
    }
}

fn is_storage_key_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_' | b':')
}

/// 校验 KV key（非空、≤128 字节、受限字符集）。
pub fn validate_storage_key(key: &str) -> Result<(), PluginError> {
    if key.is_empty() || key.len() > STORAGE_KEY_MAX_BYTES {
        return Err(PluginError::invalid_input(format!(
            "storage key must be 1..={STORAGE_KEY_MAX_BYTES} bytes"
        )));
    }
    if key.bytes().any(|byte| !is_storage_key_byte(byte)) {
        return Err(PluginError::invalid_input(
            "storage key contains unsupported characters",
        ));
    }
    Ok(())
}

/// 校验 KV key 前缀（用于 list）。
pub fn validate_storage_key_prefix(prefix: &str) -> Result<(), PluginError> {
    if prefix.len() > STORAGE_KEY_MAX_BYTES {
        return Err(PluginError::invalid_input(format!(
            "storage key prefix must be <= {STORAGE_KEY_MAX_BYTES} bytes"
        )));
    }
    if prefix.bytes().any(|byte| !is_storage_key_byte(byte)) {
        return Err(PluginError::invalid_input(
            "storage key prefix contains unsupported characters",
        ));
    }
    Ok(())
}

/// 计算写入 `key=value` 之后快照的下一个总字节数（替换语义）。
#[must_use]
pub fn storage_snapshot_next_used_bytes(
    values: &BTreeMap<String, String>,
    key: &str,
    value: &str,
) -> u64 {
    let used = values
        .values()
        .map(|value| value.len() as u64)
        .fold(0_u64, u64::saturating_add);
    used.saturating_sub(
        values
            .get(key)
            .map(|current| current.len() as u64)
            .unwrap_or(0),
    )
    .saturating_add(value.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_rejects_memory_above_hard_limit() {
        use wasmtime::ResourceLimiter;
        let mut limiter = PluginResourceLimiter::new(16 * 1024 * 1024);
        assert_eq!(
            limiter.memory_growing(0, 8 * 1024 * 1024, None).ok(),
            Some(true)
        );
        assert_eq!(
            limiter
                .memory_growing(8 * 1024 * 1024, 20 * 1024 * 1024, None)
                .ok(),
            Some(false)
        );
    }

    #[test]
    fn budget_evaluation_matches_old_semantics() {
        assert_eq!(
            evaluate_plugin_budget(8 * 1024 * 1024, 4 * 1024 * 1024, false, false),
            BudgetEvaluation::Normal
        );
        assert_eq!(
            evaluate_plugin_budget(8 * 1024 * 1024, 10 * 1024 * 1024, false, false),
            BudgetEvaluation::TrimCaches
        );
        assert_eq!(
            evaluate_plugin_budget(8 * 1024 * 1024, 10 * 1024 * 1024, true, true),
            BudgetEvaluation::Hibernate
        );
    }

    #[test]
    fn storage_key_rules_match_old_abi() {
        assert!(validate_storage_key("tasks.sync").is_ok());
        assert!(validate_storage_key("").is_err());
        assert!(validate_storage_key("UPPER").is_err());
        assert!(validate_storage_key(&"a".repeat(129)).is_err());
        assert!(validate_storage_key_prefix("tasks.").is_ok());
    }

    #[test]
    fn snapshot_next_used_bytes_replaces_existing_key() {
        let mut values = BTreeMap::new();
        values.insert("a".to_string(), "x".repeat(100));
        values.insert("b".to_string(), "y".repeat(50));
        let next = storage_snapshot_next_used_bytes(&values, "a", "z");
        assert_eq!(next, 51);
    }
}
