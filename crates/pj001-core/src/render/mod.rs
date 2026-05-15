mod atlas;
mod font;
mod geometry;
mod theme;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::grid::{Attrs, Term};

pub use font::CellMetrics;
pub use geometry::{BlockOverlay, CursorRender, SelectionRange};
pub use theme::{MarkerKind, ThemePalette};

use atlas::GlyphAtlas;
use font::FontStack;
use geometry::CellInstance;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    cell: [f32; 2],
    fg: [f32; 4],
    /// Phase 4b-2c-4b: SDF block card corner의 외부 영역(palette.bg = clear color) 처리.
    palette_bg: [f32; 4],
    /// Phase 4d: MarkerKind enum 값 (0=RoundedSquare/1=Hex/2=Dollar/3=RunChip/4=Bubble).
    marker_kind: u32,
    /// Phase 3 step 3: 윈도우 bg 불투명도. 일반 cell의 bg alpha multiplier.
    /// glyph 영역은 1.0 강제 (텍스트 가독성).
    bg_opacity: f32,
    /// Visual Bell flash intensity (0.0=normal, 1.0=fully inverted). 250ms fade.
    /// AppState가 BEL 발생 시 1.0 → 0.0 점진 감쇠.
    bell_flash: f32,
    _pad: u32,
}

// Codex 9차 권: Uniforms layout 검증 — vec2(8) + vec2(8) + vec4(16) + vec4(16) +
// u32(4) + f32(4) + f32(4) + u32(4) = 64 bytes. WGSL std140 / Rust repr(C) 정합.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 64);

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    instance_count: u32,
    atlas: GlyphAtlas,
    font_stack: FontStack,
    font_size: f32,
    cell: CellMetrics,
    baseline: f32,
    viewport: [f32; 2],
    pending_instances: Vec<CellInstance>,
    palette: ThemePalette,
    bell_flash: f32,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        viewport: [f32; 2],
        font_size: f32,
        palette: ThemePalette,
    ) -> Self {
        let mut font_stack = FontStack::new(font_size);
        let cell = font_stack.cell;
        let baseline = cell.baseline;
        let mut atlas = GlyphAtlas::new(device);
        for (ch, raster) in font_stack.raster_ascii() {
            atlas.insert(queue, ch, &raster);
        }

        let uniforms = Uniforms {
            viewport,
            cell: [cell.width as f32, cell.height as f32],
            fg: palette.fg,
            palette_bg: palette.bg,
            marker_kind: palette.block_marker_kind as u32,
            bg_opacity: palette.bg_opacity,
            bell_flash: 0.0,
            _pad: 0,
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let stride = std::mem::size_of::<CellInstance>() as wgpu::BufferAddress;
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: stride,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 40,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 56,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 72,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 76,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Uint32,
                },
                // Phase 4b-2c-4a: block_border_color
                wgpu::VertexAttribute {
                    offset: 80,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let initial_capacity = 4096usize;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance-buffer"),
            size: (initial_capacity * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            sampler,
            instance_buffer,
            instance_capacity: initial_capacity,
            instance_count: 0,
            atlas,
            font_stack,
            font_size,
            cell,
            baseline,
            viewport,
            pending_instances: Vec::new(),
            palette,
            bell_flash: 0.0,
        }
    }

    pub fn palette(&self) -> ThemePalette {
        self.palette
    }

    pub fn set_palette(&mut self, palette: ThemePalette) {
        self.palette = palette;
    }

    pub fn cell_metrics(&self) -> CellMetrics {
        self.cell
    }

    pub fn set_font_size(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_size: f32,
    ) -> bool {
        if (self.font_size - font_size).abs() < f32::EPSILON {
            return false;
        }
        let mut font_stack = FontStack::new(font_size);
        let cell = font_stack.cell;
        let baseline = cell.baseline;
        let mut atlas = GlyphAtlas::new(device);
        for (ch, raster) in font_stack.raster_ascii() {
            atlas.insert(queue, ch, &raster);
        }

        self.font_stack = font_stack;
        self.font_size = font_size;
        self.cell = cell;
        self.baseline = baseline;
        self.atlas = atlas;
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.resize(queue, self.viewport);
        true
    }

    pub fn resize(&mut self, queue: &wgpu::Queue, viewport: [f32; 2]) {
        self.viewport = viewport;
        self.write_uniforms(queue);
    }

    /// Visual bell flash intensity 갱신 (0.0=normal, 1.0=fully inverted).
    /// AppState가 매 frame elapsed로 fade out 호출.
    pub fn set_bell_flash(&mut self, queue: &wgpu::Queue, intensity: f32) {
        self.bell_flash = intensity.clamp(0.0, 1.0);
        self.write_uniforms(queue);
    }

    fn write_uniforms(&self, queue: &wgpu::Queue) {
        let uniforms = Uniforms {
            viewport: self.viewport,
            cell: [self.cell.width as f32, self.cell.height as f32],
            fg: [0.86, 0.86, 0.86, 1.0],
            palette_bg: self.palette.bg,
            marker_kind: self.palette.block_marker_kind as u32,
            bg_opacity: self.palette.bg_opacity,
            bell_flash: self.bell_flash,
            _pad: 0,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    pub fn begin_terms(&mut self) {
        self.pending_instances.clear();
        self.instance_count = 0;
    }

    pub fn append_term(
        &mut self,
        queue: &wgpu::Queue,
        term: &Term,
        preedit: Option<(&str, usize, usize)>,
        cursor: Option<geometry::CursorRender>,
        selection: Option<geometry::SelectionRange>,
        col_offset: usize,
        row_offset: usize,
        block_overlays: &[geometry::BlockOverlay],
        gutter_cells: usize,
    ) {
        // atlas miss 글리프를 동적으로 raster + insert
        for r in 0..term.rows() {
            for c in 0..term.cols() {
                let cell = term.cell(r, c);
                if cell.ch == ' ' || (cell.ch as u32) < 0x20 {
                    continue;
                }
                if cell.attrs.contains(Attrs::WIDE_CONT) {
                    continue;
                }
                if self.atlas.get(cell.ch).is_none() {
                    if let Some(raster) = self.font_stack.raster_one(cell.ch) {
                        self.atlas.insert(queue, cell.ch, &raster);
                    }
                }
            }
        }
        // preedit 글리프도 atlas에 raster (한글 자모 등 ascii 외 글자 대응)
        if let Some((preedit_str, _, _)) = preedit {
            for ch in preedit_str.chars() {
                if ch == ' ' || (ch as u32) < 0x20 {
                    continue;
                }
                if self.atlas.get(ch).is_none() {
                    if let Some(raster) = self.font_stack.raster_one(ch) {
                        self.atlas.insert(queue, ch, &raster);
                    }
                }
            }
        }
        let mut instances = geometry::build_instances_at(
            term,
            &self.atlas,
            self.baseline,
            cursor,
            selection,
            col_offset,
            row_offset,
            &self.palette,
            block_overlays,
            gutter_cells,
        );
        if let Some((preedit_str, col, row)) = preedit {
            let mut preedit_inst = geometry::build_preedit_instances_at(
                preedit_str,
                col,
                row,
                term.cols(),
                &self.atlas,
                self.baseline,
                col_offset,
                row_offset,
                &self.palette,
            );
            instances.append(&mut preedit_inst);
        }
        self.pending_instances.append(&mut instances);
    }

    pub fn append_text_line(
        &mut self,
        queue: &wgpu::Queue,
        text: &str,
        col: usize,
        row: usize,
        width: usize,
        fg: [f32; 4],
        bg: [f32; 4],
    ) {
        if width == 0 {
            return;
        }
        let chars: Vec<char> = text.chars().take(width).collect();
        for ch in &chars {
            if *ch == ' ' || (*ch as u32) < 0x20 {
                continue;
            }
            if self.atlas.get(*ch).is_none() {
                if let Some(raster) = self.font_stack.raster_one(*ch) {
                    self.atlas.insert(queue, *ch, &raster);
                }
            }
        }

        for idx in 0..width {
            let ch = chars.get(idx).copied().unwrap_or(' ');
            let entry = if ch == ' ' || (ch as u32) < 0x20 {
                None
            } else {
                self.atlas.get(ch).filter(|e| e.width > 0 && e.height > 0)
            };
            let (uv_min, uv_max, glyph_offset, glyph_size) = if let Some(e) = entry {
                (
                    e.uv_min,
                    e.uv_max,
                    [
                        e.placement_left as f32,
                        self.baseline - e.placement_top as f32,
                    ],
                    [e.width as f32, e.height as f32],
                )
            } else {
                ([0.0; 2], [0.0; 2], [0.0; 2], [0.0; 2])
            };
            self.pending_instances.push(CellInstance {
                cell_xy: [(col + idx) as f32, row as f32],
                uv_min,
                uv_max,
                glyph_offset,
                glyph_size,
                fg,
                bg,
                cell_span: 1.0,
                flags: 0,
                block_border_color: [0.0; 4],
                _pad: [0.0; 2],
            });
        }
    }

    pub fn append_fill_column(&mut self, col: usize, row: usize, height: usize, bg: [f32; 4]) {
        for idx in 0..height {
            self.pending_instances.push(CellInstance {
                cell_xy: [col as f32, (row + idx) as f32],
                uv_min: [0.0; 2],
                uv_max: [0.0; 2],
                glyph_offset: [0.0; 2],
                glyph_size: [0.0; 2],
                fg: [0.0; 4],
                bg,
                cell_span: 1.0,
                flags: 0,
                block_border_color: [0.0; 4],
                _pad: [0.0; 2],
            });
        }
    }

    /// Phase 5: sub-cell scrollbar thumb — cell 1개 폭에서 우측 가장자리 약 3px만 그림.
    /// shader가 FLAG_SCROLLBAR_THUMB bit 검사해 우측 thin band만 thumb 색.
    pub fn append_scrollbar_thumb(
        &mut self,
        col: usize,
        row: usize,
        height: usize,
        thumb_color: [f32; 4],
    ) {
        for idx in 0..height {
            self.pending_instances.push(CellInstance {
                cell_xy: [col as f32, (row + idx) as f32],
                uv_min: [0.0; 2],
                uv_max: [0.0; 2],
                glyph_offset: [0.0; 2],
                glyph_size: [0.0; 2],
                fg: [0.0; 4],
                bg: thumb_color,
                cell_span: 1.0,
                flags: geometry::FLAG_SCROLLBAR_THUMB,
                block_border_color: [0.0; 4],
                _pad: [0.0; 2],
            });
        }
    }

    pub fn append_fill_row(&mut self, col: usize, row: usize, width: usize, bg: [f32; 4]) {
        for idx in 0..width {
            self.pending_instances.push(CellInstance {
                cell_xy: [(col + idx) as f32, row as f32],
                uv_min: [0.0; 2],
                uv_max: [0.0; 2],
                glyph_offset: [0.0; 2],
                glyph_size: [0.0; 2],
                fg: [0.0; 4],
                bg,
                cell_span: 1.0,
                flags: 0,
                block_border_color: [0.0; 4],
                _pad: [0.0; 2],
            });
        }
    }

    pub fn finish_terms(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.instance_count = self.pending_instances.len() as u32;
        if self.pending_instances.is_empty() {
            return;
        }
        if self.pending_instances.len() > self.instance_capacity {
            let new_cap = self.pending_instances.len().next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instance-buffer"),
                size: (new_cap * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_cap;
            // bind_group은 instance_buffer를 참조하지 않으므로 재생성 불필요
            // 하지만 향후 storage buffer로 전환 시엔 재바인드 필요 — 지금은 sampler/atlas 참조만 유지
            self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bg"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.atlas.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        }
        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&self.pending_instances),
        );
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        // Visual Bell full-window flash (Codex 9차 사각지대 1): clear color에도 bell_flash
        // 적용. cell instance 안 그려진 빈 영역(padding/status row 등)까지 inverted.
        // bell_flash > 0이면 palette.bg ↔ inverted를 mix + alpha 1.0 강제 (vibrancy 차단).
        let bg = self.palette.bg;
        let (clear_r, clear_g, clear_b, clear_a) = if self.bell_flash > 0.0 {
            let t = self.bell_flash;
            let inv_r = 1.0 - bg[0];
            let inv_g = 1.0 - bg[1];
            let inv_b = 1.0 - bg[2];
            let normal_a = bg[3] * self.palette.bg_opacity;
            (
                bg[0] * (1.0 - t) + inv_r * t,
                bg[1] * (1.0 - t) + inv_g * t,
                bg[2] * (1.0 - t) + inv_b * t,
                normal_a * (1.0 - t) + 1.0 * t,
            )
        } else {
            (bg[0], bg[1], bg[2], bg[3] * self.palette.bg_opacity)
        };
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("text-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear_r as f64,
                        g: clear_g as f64,
                        b: clear_b as f64,
                        a: clear_a as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if self.instance_count > 0 {
            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            rpass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            rpass.draw(0..4, 0..self.instance_count);
        }
    }
}
