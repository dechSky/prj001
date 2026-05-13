use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};
use swash::scale::image::Content;
use swash::zeno::Placement;

const LINE_HEIGHT_MULTIPLIER: f32 = 1.35;
const CELL_FIT_SAMPLE_CHARS: &[char] = &[
    'M', 'g', 'j', 'p', 'q', 'y', '_', '|', '[', ']', '{', '}', '(', ')', 'Á', 'É', '안', '한',
    '가', '힣', '┃', '─', '╋',
];

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
        let line_height = (font_size * LINE_HEIGHT_MULTIPLIER).ceil();
        let metrics = Metrics::new(font_size, line_height);
        let cell = measure_cell(&mut font_system, metrics);
        let mut stack = Self {
            font_system,
            swash_cache: SwashCache::new(),
            metrics,
            cell,
        };
        stack.fit_cell_to_sampled_glyphs();
        stack
    }

    pub fn raster_one(&mut self, ch: char) -> Option<GlyphRaster> {
        let attrs = Attrs::new().family(Family::Monospace);
        let phys = {
            let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
            let s = ch.to_string();
            buffer.set_text(&s, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, true);
            buffer
                .layout_runs()
                .next()
                .and_then(|r| r.glyphs.first().cloned())
                .map(|g| g.physical((0.0, 0.0), 1.0))
        }?;
        // 비-ASCII 글자가 어떤 폰트로 raster되는지 진단(debug 수준).
        // 한글 입력 많을 때 INFO는 noise라 DEBUG. RUST_LOG=debug로 활성화.
        if !ch.is_ascii() && log::log_enabled!(log::Level::Debug) {
            let font_name = self
                .font_system
                .db()
                .face(phys.cache_key.font_id)
                .map(|f| f.post_script_name.as_str())
                .unwrap_or("<unknown>");
            log::debug!("font-fallback ch={:?} font={}", ch, font_name);
        }
        let img = self
            .swash_cache
            .get_image_uncached(&mut self.font_system, phys.cache_key)?;
        Some(GlyphRaster {
            placement: img.placement,
            data: img.data,
            content: img.content,
        })
    }

    pub fn raster_ascii(&mut self) -> Vec<(char, GlyphRaster)> {
        // Phase 0 진단: Korean fallback 작동 여부 확인
        if let Some(test) = self.raster_one('안') {
            log::info!(
                "korean fallback test: '안' placement={}x{} data_len={}",
                test.placement.width,
                test.placement.height,
                test.data.len()
            );
        } else {
            log::error!("korean fallback test: '안' raster FAILED (no glyph)");
        }

        let chars: Vec<char> = (0x20u8..=0x7Eu8).map(|b| b as char).collect();
        let mut out = Vec::with_capacity(chars.len());
        for ch in chars {
            if let Some(r) = self.raster_one(ch) {
                out.push((ch, r));
            }
        }
        out
    }

    fn fit_cell_to_sampled_glyphs(&mut self) {
        let mut placements = Vec::new();
        for ch in CELL_FIT_SAMPLE_CHARS {
            if let Some(raster) = self.raster_one(*ch) {
                placements.push(raster.placement);
            }
        }
        if placements.is_empty() {
            return;
        }

        let baseline = placements
            .iter()
            .map(|placement| placement.top as f32)
            .fold(self.cell.baseline, f32::max)
            .ceil();
        let height = placements
            .iter()
            .map(|placement| baseline - placement.top as f32 + placement.height as f32)
            .fold(self.cell.height as f32, f32::max)
            .ceil()
            .max(1.0) as u32;
        if baseline != self.cell.baseline || height != self.cell.height {
            log::debug!(
                "fit_cell_to_sampled_glyphs: cell {}x{} baseline={} -> {}x{} baseline={}",
                self.cell.width,
                self.cell.height,
                self.cell.baseline,
                self.cell.width,
                height,
                baseline,
            );
            self.cell.height = height;
            self.cell.baseline = baseline;
        }
    }
}

fn measure_cell(font_system: &mut FontSystem, metrics: Metrics) -> CellMetrics {
    let attrs = Attrs::new().family(Family::Monospace);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text("M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, true);
    // cosmic-text의 LayoutRun이 baseline y(line_y)을 직접 알려줌. line_top은 라인의 위쪽,
    // line_y는 baseline. 픽셀 align 위해 ceil. line_run 없는 edge case에 0.78 휴리스틱
    // fallback (M6-1에서 우연히 정확함을 확인했으나 이론적 안전망).
    let line_run = buffer.layout_runs().next();
    let advance = line_run
        .as_ref()
        .and_then(|r| r.glyphs.first().map(|g| g.w))
        .unwrap_or(metrics.font_size * 0.6);
    let baseline_y = line_run
        .as_ref()
        .map(|r| r.line_y)
        .unwrap_or(metrics.line_height * 0.78);
    let height = metrics.line_height;
    log::debug!(
        "measure_cell: font_size={} line_height={} advance={} baseline_y={}",
        metrics.font_size,
        height,
        advance,
        baseline_y,
    );
    CellMetrics {
        width: advance.ceil().max(1.0) as u32,
        height: height.ceil().max(1.0) as u32,
        baseline: baseline_y.ceil(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CELL_FIT_SAMPLE_CHARS, FontStack};

    #[test]
    fn sampled_glyphs_fit_inside_cell_after_metric_adjustment() {
        for font_size in [14.0, 28.0, 56.0] {
            let mut font = FontStack::new(font_size);
            let cell = font.cell;
            for ch in CELL_FIT_SAMPLE_CHARS {
                let Some(raster) = font.raster_one(*ch) else {
                    continue;
                };
                let y = cell.baseline - raster.placement.top as f32;
                assert!(
                    y >= 0.0,
                    "glyph {ch:?} starts above cell: y={y}, cell={cell:?}"
                );
                assert!(
                    y + raster.placement.height as f32 <= cell.height as f32,
                    "glyph {ch:?} overflows cell: bottom={}, height={}, cell={cell:?}",
                    y + raster.placement.height as f32,
                    cell.height,
                );
            }
        }
    }
}
