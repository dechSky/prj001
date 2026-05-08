use std::collections::HashMap;

use swash::scale::image::Content;

use super::font::GlyphRaster;

#[derive(Debug, Clone, Copy)]
pub struct AtlasEntry {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub placement_left: i32,
    pub placement_top: i32,
    pub width: u32,
    pub height: u32,
}

const ATLAS_SIZE: u32 = 4096;

pub struct GlyphAtlas {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    entries: HashMap<char, AtlasEntry>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            entries: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
        }
    }

    pub fn insert(&mut self, queue: &wgpu::Queue, ch: char, raster: &GlyphRaster) {
        if self.entries.contains_key(&ch) {
            return;
        }
        let w = raster.placement.width;
        let h = raster.placement.height;
        if w == 0 || h == 0 {
            self.entries.insert(
                ch,
                AtlasEntry {
                    uv_min: [0.0; 2],
                    uv_max: [0.0; 2],
                    placement_left: raster.placement.left,
                    placement_top: raster.placement.top,
                    width: 0,
                    height: 0,
                },
            );
            return;
        }
        // 다음 shelf로 줄바꿈
        if self.shelf_x + w > ATLAS_SIZE {
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > ATLAS_SIZE {
            // 정책 A: grow-only 4096², panic
            panic!(
                "glyph atlas full: cannot fit {}x{} for char {:?}",
                w, h, ch
            );
        }

        let pixels: Vec<u8> = match raster.content {
            Content::Mask => raster.data.clone(),
            Content::Color | Content::SubpixelMask => raster
                .data
                .chunks_exact(4)
                .map(|c| ((c[0] as u32 + c[1] as u32 + c[2] as u32) / 3) as u8)
                .collect(),
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: self.shelf_x,
                    y: self.shelf_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.entries.insert(
            ch,
            AtlasEntry {
                uv_min: [
                    self.shelf_x as f32 / ATLAS_SIZE as f32,
                    self.shelf_y as f32 / ATLAS_SIZE as f32,
                ],
                uv_max: [
                    (self.shelf_x + w) as f32 / ATLAS_SIZE as f32,
                    (self.shelf_y + h) as f32 / ATLAS_SIZE as f32,
                ],
                placement_left: raster.placement.left,
                placement_top: raster.placement.top,
                width: w,
                height: h,
            },
        );
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
    }

    pub fn get(&self, ch: char) -> Option<&AtlasEntry> {
        self.entries.get(&ch)
    }
}
