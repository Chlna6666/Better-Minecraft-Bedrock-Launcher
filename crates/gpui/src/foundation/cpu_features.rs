use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CpuVectorLevel {
    Scalar,
    Sse2,
    Avx2,
    Neon,
}

pub(crate) fn cpu_vector_level() -> CpuVectorLevel {
    static LEVEL: OnceLock<CpuVectorLevel> = OnceLock::new();
    *LEVEL.get_or_init(detect_cpu_vector_level)
}

fn detect_cpu_vector_level() -> CpuVectorLevel {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            CpuVectorLevel::Avx2
        } else {
            // SSE2 is part of the x86-64 baseline ISA.
            CpuVectorLevel::Sse2
        }
    }

    #[cfg(target_arch = "x86")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            CpuVectorLevel::Avx2
        } else if std::arch::is_x86_feature_detected!("sse2") {
            CpuVectorLevel::Sse2
        } else {
            CpuVectorLevel::Scalar
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // Advanced SIMD (NEON) is part of the AArch64 baseline ISA.
        CpuVectorLevel::Neon
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64")))]
    {
        CpuVectorLevel::Scalar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_level_is_stable_after_detection() {
        assert_eq!(cpu_vector_level(), cpu_vector_level());
    }
}
