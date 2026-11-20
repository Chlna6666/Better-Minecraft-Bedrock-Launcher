//! Shared Wasmtime engine, component cache and execution deadlines.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Linker};
use wasmtime::{Cache, CacheConfig, Config, Engine, Strategy};

use crate::bindings::{PluginWorld, PluginWorldPre};
use crate::execution::PluginState;

/// epoch 递增周期；入口超时的粒度上限。
pub const EPOCH_TICK: Duration = Duration::from_millis(50);

/// 入口调用时间预算（与旧 ABI 语义一致；epoch 粒度下最晚追加一个 tick）。
pub const INIT_TIMEOUT: Duration = Duration::from_secs(1);
pub const RENDER_TIMEOUT: Duration = Duration::from_millis(100);
pub const RENDER_WARN_THRESHOLD: Duration = Duration::from_millis(16);
pub const EVENT_TIMEOUT: Duration = Duration::from_millis(50);
pub const RPC_TIMEOUT: Duration = Duration::from_millis(500);

/// 入口调用类型 → 时间预算。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryTimeout {
    Init,
    Render,
    Event,
    Rpc,
}

impl EntryTimeout {
    #[must_use]
    pub const fn duration(self) -> Duration {
        match self {
            Self::Init => INIT_TIMEOUT,
            Self::Render => RENDER_TIMEOUT,
            Self::Event => EVENT_TIMEOUT,
            Self::Rpc => RPC_TIMEOUT,
        }
    }
}

/// 编译好的组件及其 wasm 摘要。
#[derive(Clone)]
pub struct ComponentArtifact {
    pub(crate) pre: PluginWorldPre<PluginState>,
    pub sha256: String,
}

/// 引擎句柄：析构时停止 epoch 线程。
pub struct PluginEngine {
    engine: Engine,
    linker: Linker<PluginState>,
    compiled: Mutex<BTreeMap<String, ComponentArtifact>>,
    epoch_stop: Arc<AtomicBool>,
}

impl PluginEngine {
    pub fn new() -> anyhow::Result<Self> {
        Self::with_cache(None)
    }

    /// Creates a Cranelift engine with an optional application-owned compile cache.
    pub fn with_cache(cache_dir: Option<&Path>) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config
            .strategy(Strategy::Cranelift)
            .epoch_interruption(true);
        if let Some(directory) = cache_dir {
            let mut settings = CacheConfig::new();
            match std::path::absolute(directory).and_then(|path| {
                settings.with_directory(path);
                Cache::new(settings).map_err(std::io::Error::other)
            }) {
                Ok(cache) => {
                    config.cache(Some(cache));
                }
                Err(error) => tracing::warn!(%error, "plugin disk compilation cache unavailable"),
            }
        }
        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        PluginWorld::add_to_linker::<PluginState, wasmtime::component::HasSelf<PluginState>>(
            &mut linker,
            |state| state,
        )?;
        let stop = Arc::new(AtomicBool::new(false));

        let thread_engine = engine.clone();
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("bmcbl-plugin-epoch".to_string())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    thread_engine.increment_epoch();
                }
            })
            .context("spawn plugin epoch thread")?;

        Ok(Self {
            engine,
            linker,
            compiled: Mutex::new(BTreeMap::new()),
            epoch_stop: stop,
        })
    }

    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    #[must_use]
    pub fn deadline_ticks(timeout: EntryTimeout) -> u64 {
        u64::try_from(timeout.duration().as_millis())
            .unwrap_or(u64::MAX)
            .div_ceil(EPOCH_TICK.as_millis() as u64)
            + 1
    }

    /// Compiles and pre-links an untrusted component with the shared linker.
    pub fn load_component(&self, wasm: &[u8]) -> anyhow::Result<ComponentArtifact> {
        const MAX_COMPONENT_BYTES: usize = 32 * 1024 * 1024;
        const MAX_CACHED_COMPONENTS: usize = 32;
        anyhow::ensure!(
            wasm.len() <= MAX_COMPONENT_BYTES,
            "plugin component is too large"
        );
        let sha256 = wasm_sha256_hex(wasm);
        let mut compiled = self
            .compiled
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(artifact) = compiled.get(&sha256) {
            return Ok(artifact.clone());
        }

        let component = Component::from_binary(&self.engine, wasm)
            .map_err(|error| error.context(format!("compile plugin component ({sha256})")))?;
        let pre = self.linker.instantiate_pre(&component).and_then(PluginWorldPre::new)
            .map_err(|error| error.context("link BMCBL 1.0.0 plugin component; rebuild the plugin against this WIT version"))?;
        let artifact = ComponentArtifact {
            pre,
            sha256: sha256.clone(),
        };
        if compiled.len() == MAX_CACHED_COMPONENTS {
            compiled.pop_first();
        }
        compiled.insert(sha256, artifact.clone());
        Ok(artifact)
    }
}

impl Drop for PluginEngine {
    fn drop(&mut self) {
        self.epoch_stop.store(true, Ordering::Relaxed);
    }
}

#[must_use]
pub fn wasm_sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}
