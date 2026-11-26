use gfx_core::{BackendKind, DeviceMemoryBudget, DiagnosticsDevice, MemoryLocation};
use serde_json::{Value, json};

pub(super) fn memory<D: DiagnosticsDevice>(device: &D) -> Value {
    let stats = device.resource_stats();
    let budget = match device.memory_budget() {
        Ok(Some(DeviceMemoryBudget { local, non_local })) => json!({
            "local": local.map(|value| json!({"usage_bytes": value.usage_bytes, "budget_bytes": value.budget_bytes})),
            "non_local": non_local.map(|value| json!({"usage_bytes": value.usage_bytes, "budget_bytes": value.budget_bytes})) }),
        Ok(None) => Value::Null,
        Err(error) => json!({"error": error.to_string()}),
    };
    json!({"accounting": format!("{:?}", stats.memory_accounting),
        "allocated_bytes": stats.allocated_bytes, "reserved_bytes": stats.reserved_bytes, "driver_budget": budget})
}

pub(super) fn path(backend: BackendKind, memory: MemoryLocation) -> Value {
    let (distinct_native_requests, allocation, upload) = match (backend, memory) {
        (BackendKind::Dx11, _) => (
            false,
            "D3D11_USAGE_DEFAULT (both requests equivalent)",
            "UpdateSubresource",
        ),
        (BackendKind::OpenGl, MemoryLocation::GpuOnly) => {
            (false, "driver managed, STATIC_DRAW hint", "glBufferSubData")
        }
        (BackendKind::OpenGl, _) => (
            false,
            "driver managed, DYNAMIC_DRAW hint",
            "glBufferSubData",
        ),
        (BackendKind::Dx12, MemoryLocation::GpuOnly) => (
            true,
            "DEFAULT heap",
            "temporary upload buffer + CopyBufferRegion + fence wait",
        ),
        (BackendKind::Dx12, _) => (true, "UPLOAD heap", "mapped destination"),
        (BackendKind::Vulkan, MemoryLocation::GpuOnly) => (
            true,
            "allocator GpuOnly request (actual memory type can overlap on UMA)",
            "upload page + copy + queue_wait_idle",
        ),
        (BackendKind::Vulkan, _) => (
            true,
            "allocator CpuToGpu request (actual memory type can overlap on UMA)",
            "mapped destination",
        ),
        _ => (false, "unsupported", "unsupported"),
    };
    json!({"distinct_native_requests": distinct_native_requests, "physical_pool_separation_verified": false,
        "allocation": allocation, "upload": upload,
        "asynchronous_gpu_only_upload_validated": false})
}

pub(super) fn summaries(samples: &[Value]) -> Value {
    let mut output = serde_json::Map::new();
    for key in [
        "upload_us",
        "offscreen_call_us",
        "completion_signal_us",
        "completion_wait_us",
        "completion_us",
        "update_and_completion_us",
    ] {
        let all: Vec<_> = samples
            .iter()
            .filter_map(|sample| sample[key].as_f64())
            .collect();
        output.insert(key.into(), distribution(all));
    }
    output.insert(
        "dirty_upload_us".into(),
        distribution(
            samples
                .iter()
                .filter(|sample| sample["dirty"] == true)
                .filter_map(|sample| sample["upload_us"].as_f64())
                .collect(),
        ),
    );
    Value::Object(output)
}

fn distribution(mut values: Vec<f64>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(f64::total_cmp);
    let percentile =
        |percent: usize| values[(values.len() * percent).div_ceil(100).saturating_sub(1)];
    json!({"count": values.len(), "p50": percentile(50), "p95": percentile(95), "p99": percentile(99),
        "max": values.last(), "mean": values.iter().sum::<f64>() / values.len() as f64})
}
