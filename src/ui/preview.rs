use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu;
use wgpu::util::DeviceExt as _;

use crate::codec::ir::Yuv420;

/// GPU resources that live for the lifetime of the application.
pub struct YuvResources {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    /// Updated each frame by the prepare callback.
    pub bind_group: Option<wgpu::BindGroup>,
}

impl YuvResources {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shader.wgsl").into(),
            ),
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("yuv_bgl"),
                entries: &[
                    // Y plane
                    tex_entry(0),
                    // U plane
                    tex_entry(1),
                    // V plane
                    tex_entry(2),
                    // sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(
                            wgpu::SamplerBindingType::Filtering,
                        ),
                        count: None,
                    },
                ],
            });

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("yuv_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("yuv_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("yuv_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self { pipeline, bind_group_layout, sampler, bind_group: None }
    }
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Per-frame callback that uploads YUV planes and issues the draw call.
pub struct YuvPreviewCallback {
    pub yuv: Arc<Yuv420>,
}

impl egui_wgpu::CallbackTrait for YuvPreviewCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let res: &mut YuvResources = resources.get_mut().unwrap();

        let y_view = upload_r8(device, queue, &self.yuv.y, self.yuv.width, self.yuv.height, "y");
        let u_view = upload_r8(device, queue, &self.yuv.u, self.yuv.chroma_width(), self.yuv.chroma_height(), "u");
        let v_view = upload_r8(device, queue, &self.yuv.v, self.yuv.chroma_width(), self.yuv.chroma_height(), "v");

        res.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_bg"),
            layout: &res.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&y_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&u_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&v_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&res.sampler) },
            ],
        }));

        vec![]
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let res: &YuvResources = resources.get().unwrap();
        render_pass.set_pipeline(&res.pipeline);
        if let Some(bg) = &res.bind_group {
            render_pass.set_bind_group(0, bg, &[]);
            render_pass.draw(0..3, 0..1);
        }
    }
}

/// Upload a single-channel u8 plane as an R8Unorm texture and return its view.
fn upload_r8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &[u8],
    width: u32,
    height: u32,
    label: &str,
) -> wgpu::TextureView {
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        data,
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}
