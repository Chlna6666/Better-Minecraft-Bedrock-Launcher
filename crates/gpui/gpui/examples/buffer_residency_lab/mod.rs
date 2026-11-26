use anyhow::{Result, bail, ensure};
use gfx_core::{BackendKind, MemoryLocation};

mod native;
mod report;
mod workload;

pub(super) use native::run_backend;

#[derive(Debug)]
pub(super) struct Options {
    backend: BackendKind,
    memory: MemoryLocation,
    bytes: usize,
    samples: usize,
    warmup: usize,
    draws: usize,
    update_every: usize,
    adapter: Option<String>,
}

impl Options {
    pub(super) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut options = Self {
            backend: if cfg!(windows) {
                BackendKind::Dx12
            } else if cfg!(target_os = "macos") {
                BackendKind::Metal
            } else {
                BackendKind::Vulkan
            },
            memory: MemoryLocation::CpuToGpu,
            bytes: 8 * 1024 * 1024,
            samples: 80,
            warmup: 8,
            draws: 8,
            update_every: 0,
            adapter: None,
        };
        for argument in arguments {
            let (key, value) = argument.split_once('=').ok_or_else(|| anyhow::anyhow!(
                "use --backend=nova-dx11|nova-dx12|nova-vulkan|nova-opengl|nova-metal --memory=cpu-visible|gpu-only --bytes=8388608 --samples=80 --warmup=8 --draws=8 --update-every=0 --adapter=NAME"))?;
            match key {
                "--backend" => {
                    options.backend = match value {
                        "nova-dx11" => BackendKind::Dx11,
                        "nova-dx12" => BackendKind::Dx12,
                        "nova-vulkan" => BackendKind::Vulkan,
                        "nova-opengl" => BackendKind::OpenGl,
                        "nova-metal" => BackendKind::Metal,
                        _ => bail!("unknown backend: {value}"),
                    }
                }
                "--memory" => {
                    options.memory = match value {
                        "cpu-visible" => MemoryLocation::CpuToGpu,
                        "gpu-only" => MemoryLocation::GpuOnly,
                        _ => bail!("unknown memory policy: {value}"),
                    }
                }
                "--bytes" => options.bytes = value.parse()?,
                "--samples" => options.samples = value.parse()?,
                "--warmup" => options.warmup = value.parse()?,
                "--draws" => options.draws = value.parse()?,
                "--update-every" => options.update_every = value.parse()?,
                "--adapter" => options.adapter = Some(value.to_owned()),
                _ => bail!("unknown argument: {key}"),
            }
        }
        ensure!(
            options.bytes.is_power_of_two() && (65536..=67108864).contains(&options.bytes),
            "bytes must be a power of two from 64 KiB through 64 MiB"
        );
        ensure!(
            (1..=10000).contains(&options.samples),
            "samples must be 1..=10000"
        );
        ensure!(options.warmup <= 10000, "warmup must be <= 10000");
        ensure!((1..=64).contains(&options.draws), "draws must be 1..=64");
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_or_unbounded_workloads_are_rejected() {
        for argument in [
            "--bytes=0",
            "--bytes=65537",
            "--bytes=134217728",
            "--samples=0",
            "--warmup=10001",
            "--draws=0",
            "--draws=65",
            "--memory=automatic",
            "--unknown=1",
        ] {
            assert!(Options::parse([argument.to_owned()]).is_err(), "{argument}");
        }
        assert!(
            Options::parse(["--memory=gpu-only".to_owned(), "--bytes=65536".to_owned()]).is_ok()
        );
    }
}
