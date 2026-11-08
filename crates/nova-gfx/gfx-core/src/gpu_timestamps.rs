//! Device-neutral native GPU workload timestamps.
//!
//! These are **completed hardware query** measurements, never host API timings.
//! A backend may return no samples on unsupported or disjoint clocks. Sampling
//! must reuse an existing GPU submission, and readback must not wait for fences.

use std::time::Duration;

/// Native GPU workload category. Rendering the indirect command list is a
/// distinct stage from CPU command encoding or compute dispatch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GpuTimestampStage {
    /// Ordered meshing compute dispatches.
    GeneratedMesh,
    /// Buffer copies into GPU-resident tables and meshing inputs.
    BufferUpload,
    /// Native indirect draws, excluding CPU command construction.
    DrawIndirect,
}

/// Correlation key for one Section publication, independent of backend API.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GpuTimestampTag {
    /// Globally unique application-side publication identity.
    pub batch_id: u64,
    /// Native GPU stage within that publication.
    pub stage: GpuTimestampStage,
}

/// Result of a completed native GPU timestamp query pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuTimestampSample {
    /// Identity attached when the native command list was recorded.
    pub tag: GpuTimestampTag,
    /// Elapsed native clock ticks, after handling backend-specific clock wrap.
    pub duration_ticks: u64,
    /// Native timer frequency. Zero means no calibrated clock is available.
    pub frequency_hz: u64,
}

impl GpuTimestampSample {
    /// Converts the hardware ticks to a duration without overflow.
    /// An unavailable clock is never represented as a zero-duration success.
    #[must_use]
    pub fn duration(self) -> Option<Duration> {
        if self.frequency_hz == 0 {
            return None;
        }
        let nanos = u128::from(self.duration_ticks)
            .saturating_mul(1_000_000_000)
            / u128::from(self.frequency_hz);
        Some(Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_native_ticks_without_rounding_to_host_api_time() {
        let sample = GpuTimestampSample {
            tag: GpuTimestampTag { batch_id: 7, stage: GpuTimestampStage::GeneratedMesh },
            duration_ticks: 125,
            frequency_hz: 1_000,
        };
        assert_eq!(sample.duration(), Some(Duration::from_millis(125)));
    }

    #[test]
    fn zero_frequency_is_unavailable() {
        let sample = GpuTimestampSample {
            tag: GpuTimestampTag { batch_id: 9, stage: GpuTimestampStage::BufferUpload },
            duration_ticks: 42,
            frequency_hz: 0,
        };
        assert_eq!(sample.duration(), None);
    }
}
