use pixels::PixelsContext;
use wgpu::util::DeviceExt;

pub struct GameScaler {
    vertex_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    render_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Locals {
    // Matches WGSL layout (2x vec2<f32>) = 16 bytes.
    scale: [f32; 2],
    offset: [f32; 2],
}

impl GameScaler {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = wgpu::include_wgsl!("shaders/game_scale.wgsl");
        let module = device.create_shader_module(shader);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vibeemu_game_scaler_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Two triangles covering a quad in NDC.
        // position (x,y), tex_coord (u,v)
        let verts: [[f32; 4]; 6] = [
            [-1.0, -1.0, 0.0, 1.0],
            [1.0, -1.0, 1.0, 1.0],
            [-1.0, 1.0, 0.0, 0.0],
            [-1.0, 1.0, 0.0, 0.0],
            [1.0, -1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 0.0],
        ];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vibeemu_game_scaler_vertex_buffer"),
            contents: unsafe {
                std::slice::from_raw_parts(
                    verts.as_ptr().cast::<u8>(),
                    std::mem::size_of_val(&verts),
                )
            },
            usage: wgpu::BufferUsages::VERTEX,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vibeemu_game_scaler_uniform_buffer"),
            size: std::mem::size_of::<Locals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vibeemu_game_scaler_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<Locals>() as u64
                        ),
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vibeemu_game_scaler_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: (4 * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: (2 * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
                    shader_location: 1,
                },
            ],
        };

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vibeemu_game_scaler_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: "vs_main",
                buffers: &[vertex_buffer_layout],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });

        Self {
            vertex_buffer,
            uniform_buffer,
            bind_group_layout,
            render_pipeline,
            sampler,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        render_target: &wgpu::TextureView,
        context: &PixelsContext,
        surface_w: u32,
        surface_h: u32,
        buffer_w: u32,
        buffer_h: u32,
        top_padding_px: u32,
    ) -> (u32, u32, u32, u32) {
        let avail_h = surface_h.saturating_sub(top_padding_px).max(1);
        let scale_x = (surface_w / buffer_w).max(1);
        let scale_y = (avail_h / buffer_h).max(1);
        let scale = scale_x.min(scale_y).max(1);

        let scaled_w = buffer_w.saturating_mul(scale).min(surface_w);
        let scaled_h = buffer_h.saturating_mul(scale).min(avail_h);

        let x0 = (surface_w - scaled_w) / 2;
        let y0 = top_padding_px + (avail_h - scaled_h) / 2;

        let ndc_scale_x = scaled_w as f32 / surface_w as f32;
        let ndc_scale_y = scaled_h as f32 / surface_h as f32;

        let center_x_ndc = -1.0 + ((x0 as f32 + (scaled_w as f32) / 2.0) * 2.0 / surface_w as f32);
        let center_y_ndc = 1.0 - ((y0 as f32 + (scaled_h as f32) / 2.0) * 2.0 / surface_h as f32);

        let locals = Locals {
            scale: [ndc_scale_x, ndc_scale_y],
            offset: [center_x_ndc, center_y_ndc],
        };

        context.queue.write_buffer(&self.uniform_buffer, 0, unsafe {
            std::slice::from_raw_parts(
                (&locals as *const Locals).cast::<u8>(),
                std::mem::size_of::<Locals>(),
            )
        });

        let texture_view = context
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vibeemu_game_scaler_bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                ],
            });

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vibeemu_game_scaler_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: render_target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: true,
                },
            })],
            depth_stencil_attachment: None,
        });

        rpass.set_pipeline(&self.render_pipeline);
        rpass.set_bind_group(0, &bind_group, &[]);
        rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        rpass.set_scissor_rect(x0, y0, scaled_w, scaled_h);
        rpass.draw(0..6, 0..1);

        (x0, y0, scaled_w, scaled_h)
    }
}
