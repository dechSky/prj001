use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};
use swash::scale::image::Content;
use swash::zeno::Placement;

#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    pub baseline: f32,
}

pub struct GlyphRaster {
    pub placement: Placement,
    pub data: Vec<u8>,
    pub content: Content,
}

pub struct FontStack {
    font_system: FontSystem,
    swash_cache: SwashCache,
    metrics: Metrics,
    pub cell: CellMetrics,
}

impl FontStack {
    pub fn new(font_size: f32) -> Self {
        let mut font_system = FontSystem::new();
        let line_height = (font_size * 1.3).ceil();
        let metrics = Metrics::new(font_size, line_height);
        let cell = measure_cell(&mut font_system, metrics);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            metrics,
            cell,
        }
    }

    pub fn raster_ascii(&mut self) -> Vec<(char, GlyphRaster)> {
        let attrs = Attrs::new().family(Family::Monospace);
        let chars: Vec<char> = (0x20u8..=0x7Eu8).map(|b| b as char).collect();
        let mut out = Vec::with_capacity(chars.len());
        for ch in chars {
            let phys = {
                let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
                let s = ch.to_string();
                buffer.set_text(&s, &attrs, Shaping::Basic, None);
                buffer.shape_until_scroll(&mut self.font_system, true);
                buffer
                    .layout_runs()
                    .next()
                    .and_then(|r| r.glyphs.first().cloned())
                    .map(|g| g.physical((0.0, 0.0), 1.0))
            };
            let Some(phys) = phys else { continue };
            if let Some(img) = self
                .swash_cache
                .get_image_uncached(&mut self.font_system, phys.cache_key)
            {
                out.push((
                    ch,
                    GlyphRaster {
                        placement: img.placement,
                        data: img.data,
                        content: img.content,
                    },
                ));
            }
        }
        out
    }
}

fn measure_cell(font_system: &mut FontSystem, metrics: Metrics) -> CellMetrics {
    let attrs = Attrs::new().family(Family::Monospace);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text("M", &attrs, Shaping::Basic, None);
    buffer.shape_until_scroll(font_system, true);
    let advance = buffer
        .layout_runs()
        .next()
        .and_then(|r| r.glyphs.first().map(|g| g.w))
        .unwrap_or(metrics.font_size * 0.6);
    let height = metrics.line_height;
    CellMetrics {
        width: advance.ceil().max(1.0) as u32,
        height: height.ceil().max(1.0) as u32,
        baseline: (height * 0.78).ceil(),
    }
}
