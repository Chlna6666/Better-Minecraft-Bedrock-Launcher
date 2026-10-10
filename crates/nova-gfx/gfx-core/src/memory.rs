//! Memory accounting describes ownership separately from driver residency.

/// Physical CPU/GPU memory relationship reported by a native device query.
///
/// This is independent of resource placement: unified devices can still offer
/// different upload and device-local resource policies. Unknown must not be
/// inferred from adapter names, dedicated-memory sizes or local budgets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MemoryArchitecture {
    /// The backend has no authoritative architecture query.
    #[default]
    Unknown,
    /// CPU and GPU use a unified physical memory architecture.
    Unified,
    /// CPU and GPU use separate physical memory pools.
    Discrete,
}

/// Origin of byte counts in [`crate::ResourceStats`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MemoryAccounting {
    /// The backend does not provide memory byte accounting.
    #[default]
    Unavailable,
    /// Native resource sizes, excluding driver overhead and untracked resources.
    ResourceSizes,
    /// Allocator live allocations and reserved backing blocks.
    Allocator,
}

/// A driver-reported process memory usage and current budget for one segment.
///
/// Budgets can change while the application runs. They are observations, not
/// hard allocation limits. A local segment can be shared system RAM on UMA.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryBudget {
    /// Current driver-reported usage in bytes.
    pub usage_bytes: u64,
    /// Current driver-reported budget in bytes; zero is preserved as reported.
    pub budget_bytes: u64,
}

impl MemoryBudget {
    /// Remaining budget, clamped to zero when already exceeded.
    #[must_use]
    pub const fn headroom_bytes(self) -> u64 {
        self.budget_bytes.saturating_sub(self.usage_bytes)
    }

    /// Usage above the current budget.
    #[must_use]
    pub const fn over_budget_bytes(self) -> u64 {
        self.usage_bytes.saturating_sub(self.budget_bytes)
    }

    /// Whole percentage of budget used, including values above 100 percent.
    /// Returns `None` for a reported zero budget.
    #[must_use]
    pub fn utilization(self) -> Option<u64> {
        (self.budget_bytes != 0).then(|| {
            u64::try_from(u128::from(self.usage_bytes) * 100 / u128::from(self.budget_bytes))
                .unwrap_or(u64::MAX)
        })
    }
}

/// Available driver memory segments. Missing segments are unsupported, not zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeviceMemoryBudget {
    /// Device-local segment; this does not necessarily mean dedicated VRAM.
    pub local: Option<MemoryBudget>,
    /// Non-local segment, when separately reported by the driver.
    pub non_local: Option<MemoryBudget>,
}

/// Result of explicit owner-thread heap maintenance; zero moved resources means no relocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryCompactReport {
    /// Native buffers/images whose contents were preserved while replacing their allocation.
    pub moved_resources: usize,
    /// Live allocation bytes copied, excluding transient upload pages discarded at maintenance.
    pub moved_bytes: u64,
    /// Allocator backing bytes before maintenance (not process residency).
    pub reserved_before: u64,
    /// Maximum allocator backing observed while reserving and copying this pass.
    /// Driver-managed backends that skip maintenance leave this at zero.
    pub reserved_peak: u64,
    /// Allocator backing bytes after maintenance (not process residency).
    pub reserved_after: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_preserves_zero_and_over_budget_usage() {
        let budget = MemoryBudget {
            usage_bytes: 150,
            budget_bytes: 100,
        };
        assert_eq!(budget.headroom_bytes(), 0);
        assert_eq!(budget.over_budget_bytes(), 50);
        assert_eq!(budget.utilization(), Some(150));
        assert_eq!(MemoryBudget::default().utilization(), None);
        assert_eq!(DeviceMemoryBudget::default().local, None);
        assert_eq!(
            MemoryBudget {
                usage_bytes: u64::MAX,
                budget_bytes: 1
            }
            .utilization(),
            Some(u64::MAX)
        );
    }
}
