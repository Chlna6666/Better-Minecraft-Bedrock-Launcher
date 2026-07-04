use gfx_core::{BackendCapabilities, BackendKind, DeviceDesc};

fn main() {
    let descriptor = DeviceDesc::default();
    let capabilities = BackendCapabilities {
        surface: true,
        cpu_visible_memory: true,
        gpu_only_memory: true,
    };
    let backends = [
        BackendKind::Vulkan,
        BackendKind::Dx12,
        BackendKind::Metal,
        BackendKind::OpenGl,
        BackendKind::WebGl,
    ];

    assert_eq!(descriptor.application_name, "nova-gfx");
    assert!(capabilities.surface);
    assert_eq!(backends.len(), 5);
}
