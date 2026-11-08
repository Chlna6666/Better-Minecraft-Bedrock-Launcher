use super::*;

#[test]
fn producer_fence_survives_without_a_main_submission() {
    let producer = SubmissionId::from_parts(1, 1);
    let main = SubmissionId::from_parts(2, 1);
    let entry = |submission| PendingSubmission {
        submission,
        predecessors: SmallVec::new(),
        frame_resource_index: 0,
        publication: None,
    };
    let mut pending = vec![entry(producer)];
    attach_extension_submission(&mut pending, Some(producer));
    assert_eq!(pending.len(), 1);
    pending.push(entry(main));
    attach_extension_submission(&mut pending, Some(producer));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].submission, main);
    assert_eq!(pending[0].predecessors.as_slice(), &[producer]);
}

#[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
mod native {
    use super::*;
    use crate::{
        ExtensionDevice, RendererExtension, RendererExtensionContext, RendererExtensionPass,
        RendererExtensionRenderer,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Clone)]
    struct ClearInput {
        pass: RendererExtensionPass,
        accepted: Arc<AtomicBool>,
    }
    impl RendererExtension for ClearInput {
        fn create_renderer(
            &self,
            _: &mut dyn ExtensionDevice,
            _: RendererExtensionContext,
        ) -> Result<Box<dyn RendererExtensionRenderer>> {
            Ok(Box::new(ClearRenderer {
                input: self.clone(),
                pending: Vec::new(),
            }))
        }
    }
    struct ClearRenderer {
        input: ClearInput,
        pending: Vec<RendererExtensionPass>,
    }
    impl RendererExtensionRenderer for ClearRenderer {
        fn render(
            &mut self,
            _: &dyn RendererExtension,
            _: &mut dyn ExtensionDevice,
            _: RendererExtensionContext,
            _: &mut Vec<RenderCommand>,
        ) -> Result<()> {
            self.pending.push(self.input.pass.clone());
            Ok(())
        }
        fn drain_render_passes(&mut self, passes: &mut Vec<RendererExtensionPass>) {
            passes.append(&mut self.pending);
        }
        fn render_passes_submitted(&mut self) {
            self.input.accepted.store(true, Ordering::SeqCst);
        }
        fn destroy(&mut self, _: &mut dyn ExtensionDevice) -> Result<()> {
            Ok(())
        }
    }

    fn target(
        device: &mut impl ResourceDevice,
        extent: Extent2d,
        format: Format,
        usage: TextureUsage,
    ) -> (TextureId, TextureViewId) {
        let texture = device
            .create_texture(&TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                format,
                usage,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
                sample_count: 1,
            })
            .unwrap();
        let view = device
            .create_texture_view(&TextureViewDescriptor {
                label: None,
                texture,
                base_mip_level: 0,
                mip_level_count: 1,
                format,
            })
            .unwrap();
        (texture, view)
    }

    #[test]
    #[ignore = "requires D3D12; ordered extension producer, Root consumer and failure fence retention"]
    fn extension_precedes_root_capture_dx12() {
        use gfx_core::{BackendKind, TextureTransferDevice};
        let mut device = gfx_dx12::Dx12Device::new(&DeviceDescriptor::default()).unwrap();
        let extent = Extent2d::new(4, 4).unwrap();
        let (color, view) = target(
            &mut device,
            extent,
            Format::Rgba8Unorm,
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
        );
        let (depth, depth_view) = target(
            &mut device,
            extent,
            Format::Depth32Float,
            TextureUsage::DEPTH_ATTACHMENT,
        );
        let pass = PipelineDevice::create_render_pass(
            &mut device,
            &RenderPassDescriptor {
                label: None,
                color_attachments: vec![ColorAttachmentDescriptor {
                    format: Format::Rgba8Unorm,
                }],
                depth_attachment: Some(DepthAttachmentDescriptor {
                    format: Format::Depth32Float,
                    stencil_format: None,
                }),

                sample_count: 1,
                resolve_target: None,
            },
        )
        .unwrap();
        let depth_attachment = DepthAttachment {
            target: depth_view,
            depth_load_op: LoadOp::Clear(1.),
            stencil_load_op: None,
        };
        let context = RendererExtensionContext::for_target(
            BackendKind::Dx12,
            pass,
            Format::Rgba8Unorm,
            extent,
        );
        let input = ClearInput {
            accepted: Arc::new(AtomicBool::new(false)),
            pass: RendererExtensionPass {
                target: view,
                render_pass: pass,
                commands: Vec::new().into(),
                color_load_op: LoadOp::Clear(ClearColor {
                    red: 1.,
                    ..ClearColor::default()
                }),
                depth_attachment: Some(depth_attachment),
                resolve_target: None,
            },
        };
        let mut registry = super::super::super::extensions::RendererRegistry::default();
        let mut pending = Vec::new();
        let layers = [PreparedColorLayer {
            index: 0,
            source_texture_view: view,
            source_groups: vec![PreparedBackdropBlurGroup {
                source_steps: Vec::new(),
                source_clear_region: None,
                filter_passes: Vec::new(),
                preserve_filtered_pixels: false,
            }],
            filter_passes: Vec::new(),
            preserve_filtered_pixels: false,
            preserve_retained_source: false,
        }];
        let graph = FrameGraphPlan::compile(
            false,
            0,
            0,
            super::super::super::frame_graph::RootColorAccess::Refresh,
            false,
        );
        let viewport = gfx_core::Viewport {
            origin: gfx_core::SignedOrigin2d::ZERO,
            extent,
            reference_extent: extent,
            mapping: gfx_core::ViewportMapping::Scaled,
        };
        for fail in [false, true] {
            registry.begin_frame();
            input.accepted.store(false, Ordering::SeqCst);
            registry
                .prepare(&input, &mut device, context.clone(), &mut Vec::new())
                .unwrap();
            let result = execute_frame_graph_offscreen(
                &mut device,
                &graph,
                if fail {
                    RenderPassId::from_parts(999_999, 1)
                } else {
                    pass
                },
                pass,
                depth_attachment,
                view,
                &[],
                &mut Duration::ZERO,
                &[],
                &layers,
                None,
                &[],
                viewport,
                ExtensionSubmission {
                    registry: &mut registry,
                    pending: &mut pending,
                    frame_resource_index: 0,
                },
            );
            assert_eq!(result.is_err(), fail);
            assert!(input.accepted.load(Ordering::SeqCst));
            assert_eq!(
                pending.len(),
                1,
                "a later consumer failure must not lose its producer fence"
            );
            let pixels = device.read_texture(color).unwrap();
            assert_eq!(
                pixels.bytes[0],
                if fail { 255 } else { 0 },
                "Root capture must execute after the red extension producer"
            );
            for entry in pending.drain(..) {
                SubmissionDevice::wait_submission(&mut device, entry.submission).unwrap();
            }
        }
        registry.destroy(&mut device);
        ResourceDevice::destroy_texture_view(&mut device, view).unwrap();
        ResourceDevice::destroy_texture_view(&mut device, depth_view).unwrap();
        ResourceDevice::destroy_texture(&mut device, color).unwrap();
        ResourceDevice::destroy_texture(&mut device, depth).unwrap();
        PipelineDevice::destroy_render_pass(&mut device, pass).unwrap();
    }
}
