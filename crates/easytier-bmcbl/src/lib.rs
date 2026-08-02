#![allow(dead_code)]

pub use easytier_core::{
    Instant, VERSION, common, connector, embedded, instance, launcher, peers, proto, rpc_service,
    tunnel, utils, web_client,
};

pub mod instance_manager {
    use std::ops::Deref;
    use std::path::PathBuf;

    use easytier_core::common::config::{
        ConfigFileControl, ConfigLoader as _, TomlConfigLoader,
    };
    use easytier_core::proto::common::CompressionAlgoPb;

    const MIN_WORKER_THREADS: usize = 4;
    const MAX_WORKER_THREADS: usize = 6;

    fn worker_thread_count_for(logical_cpus: usize) -> usize {
        (logical_cpus / 2).clamp(MIN_WORKER_THREADS, MAX_WORKER_THREADS)
    }

    fn recommended_worker_threads() -> usize {
        let logical_cpus = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(MIN_WORKER_THREADS);
        worker_thread_count_for(logical_cpus)
    }

    /// BMCBL-specific adapter around EasyTier's instance manager.
    ///
    /// EasyTier still owns its dedicated OS thread and Tokio runtime. This
    /// adapter only constrains that runtime before an instance is started so
    /// it cannot consume all launcher CPU resources.
    pub struct NetworkInstanceManager {
        inner: easytier_core::instance_manager::NetworkInstanceManager,
    }

    impl NetworkInstanceManager {
        pub fn new() -> Self {
            Self {
                inner: easytier_core::instance_manager::NetworkInstanceManager::new(),
            }
        }

        pub fn with_config_path(self, config_dir: Option<PathBuf>) -> Self {
            Self {
                inner: self.inner.with_config_path(config_dir),
            }
        }

        pub fn run_network_instance(
            &self,
            cfg: TomlConfigLoader,
            watch_event: bool,
            config_file_control: ConfigFileControl,
        ) -> Result<uuid::Uuid, anyhow::Error> {
            let mut flags = cfg.get_flags();
            flags.multi_thread = true;
            flags.multi_thread_count = recommended_worker_threads() as u32;
            flags.data_compress_algo = CompressionAlgoPb::None.into();
            cfg.set_flags(flags);

            self.inner
                .run_network_instance(cfg, watch_event, config_file_control)
        }
    }

    impl Default for NetworkInstanceManager {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Deref for NetworkInstanceManager {
        type Target = easytier_core::instance_manager::NetworkInstanceManager;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    #[cfg(test)]
    mod tests {
        use super::worker_thread_count_for;

        #[test]
        fn worker_count_uses_half_cpu_with_bounds() {
            assert_eq!(worker_thread_count_for(1), 4);
            assert_eq!(worker_thread_count_for(4), 4);
            assert_eq!(worker_thread_count_for(8), 4);
            assert_eq!(worker_thread_count_for(10), 5);
            assert_eq!(worker_thread_count_for(12), 6);
            assert_eq!(worker_thread_count_for(64), 6);
        }
    }
}
