//! 테마 팔레트 — 6 테마(themes-handoff.md §2)의 fg/bg/selection + ANSI 16색.

use std::fmt;

/// Phase 4d: theme별 marker 모양. design §10 themes-handoff 매핑:
/// - aurora/obsidian: RoundedSquare
/// - vellum: Dollar (`$` glyph)
/// - holo: Hex (6각형)
/// - bento: RunChip (status chip 형태)
/// - crystal: Bubble (radial gradient circle)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum MarkerKind {
    RoundedSquare = 0,
    Hex = 1,
    Dollar = 2,
    RunChip = 3,
    Bubble = 4,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemePalette {
    pub name: &'static str,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub selection_bg: [f32; 4],
    /// ANSI 16색. [0-7] = 표준 (검정/빨/녹/노/파/마젠타/시안/흰),
    /// [8-15] = bright. 테마별로 톤 매칭 + 라이트 테마는 7/15가 bg와 안 묻히게.
    pub ansi: [[f32; 4]; 16],
    /// Phase 4b-2c: OSC 133 명령 블록 카드 bg. palette.bg와 다른 약간의 톤 변화.
    pub block_bg: [f32; 4],
    /// Phase 4b-2c: 명령 블록 카드 border. edge cell에만 적용.
    pub block_border: [f32; 4],
    /// Phase 4d: prompt marker 모양 (테마별 시각 차별화).
    pub block_marker_kind: MarkerKind,
    /// Phase 3 step 3: 윈도우 bg 불투명도 (0.0~1.0). 1.0 = 완전 불투명(vibrancy 없음),
    /// 낮을수록 NSVisualEffectView 뒤 데스크톱이 더 비침. glyph(텍스트)는 항상 1.0으로
    /// shader에서 별도 처리. themes-handoff.md §2 시각 의도:
    /// - aurora: 0.35 (라이트 글래스)
    /// - obsidian: 0.85 (다크 vibrancy)
    /// - vellum: 1.0 (paper, no blur)
    /// - holo: 0.85
    /// - bento: 1.0 (no blur 권장)
    /// - crystal: 0.55 (radial deep — original 0.085은 가독성 떨어져 보강)
    pub bg_opacity: f32,
}

impl ThemePalette {
    pub fn by_name(name: &str) -> Option<Self> {
        Some(match name {
            "aurora" => Self::aurora(),
            "obsidian" => Self::obsidian(),
            "vellum" => Self::vellum(),
            "holo" => Self::holo(),
            "bento" => Self::bento(),
            "crystal" => Self::crystal(),
            _ => return None,
        })
    }

    /// 01 · Aurora Glass — 라이트, 소프트 파스텔. bg는 multi-stop radial의 기저(linear base) 사용.
    /// ANSI: 라이트 bg + 어두운 텍스트 정렬. 7/15은 라이트 회색이 아닌 dim한 fg-tone(bg와 안 묻힘).
    pub const fn aurora() -> Self {
        Self {
            name: "aurora",
            fg: rgb(0x2b, 0x27, 0x34),
            bg: rgb(0xf6, 0xf2, 0xeb),
            selection_bg: rgb(0xb9, 0xa6, 0xc2),
            ansi: [
                rgb(0x2b, 0x27, 0x34),
                rgb(0xc4, 0x48, 0x5a),
                rgb(0x4a, 0x8a, 0x6a),
                rgb(0xb8, 0x9c, 0x4a),
                rgb(0x4a, 0x6a, 0xc4),
                rgb(0x8a, 0x4a, 0xc4),
                rgb(0x4a, 0x96, 0xb8),
                rgb(0x5a, 0x53, 0x6a),
                rgb(0x5a, 0x53, 0x6a),
                rgb(0xe0, 0x42, 0x7a),
                rgb(0x1f, 0x9d, 0x6b),
                rgb(0xe0, 0x7a, 0x3a),
                rgb(0x3b, 0x7b, 0xe0),
                rgb(0x7a, 0x5a, 0xf8),
                rgb(0x3a, 0xa6, 0xc8),
                rgb(0x2b, 0x27, 0x34),
            ],
            // 4b-2c-1: aurora는 design §12 "obsidian 시각 + 5 테마 raw fallback" 따라
            // bg와 동일값 (visual 효과 0). 4d에서 정식 매핑.
            block_bg: rgb(0xf6, 0xf2, 0xeb),
            block_border: rgb(0xf6, 0xf2, 0xeb),
            block_marker_kind: MarkerKind::RoundedSquare,
            bg_opacity: 0.35,
        }
    }

    /// 02 · Obsidian Glass — 다크 보이드 + 시안 네온.
    pub const fn obsidian() -> Self {
        Self {
            name: "obsidian",
            fg: rgb(0xe8, 0xe7, 0xef),
            bg: rgb(0x07, 0x09, 0x1a),
            selection_bg: rgb(0x23, 0x72, 0x8e),
            ansi: [
                rgb(0x1a, 0x1d, 0x2e),
                rgb(0xff, 0x6b, 0x88),
                rgb(0x5d, 0xff, 0xaa),
                rgb(0xff, 0xd0, 0x7a),
                rgb(0x3e, 0xda, 0xff),
                rgb(0xc0, 0x84, 0xfc),
                rgb(0x80, 0xf0, 0xff),
                rgb(0xa0, 0xa0, 0xb8),
                rgb(0x3a, 0x3d, 0x4e),
                rgb(0xff, 0x8a, 0xa6),
                rgb(0x80, 0xff, 0xc0),
                rgb(0xff, 0xe0, 0x90),
                rgb(0x80, 0xea, 0xff),
                rgb(0xd0, 0xa0, 0xff),
                rgb(0xa0, 0xff, 0xff),
                rgb(0xe8, 0xe7, 0xef),
            ],
            // 4b-2c-1: obsidian 카드 — bg는 mix(bg, fg, ~6%) ≈ #14 16 26, border는 ~15% ≈ #28 2a 3a.
            // design §12 obsidian이 4b 시각 발동 대상.
            block_bg: rgb(0x14, 0x16, 0x26),
            block_border: rgb(0x28, 0x2a, 0x3a),
            block_marker_kind: MarkerKind::RoundedSquare,
            bg_opacity: 0.85,
        }
    }

    /// 03 · Vellum — 따뜻한 종이결.
    pub const fn vellum() -> Self {
        Self {
            name: "vellum",
            fg: rgb(0x2a, 0x24, 0x19),
            bg: rgb(0xef, 0xe7, 0xd2),
            selection_bg: rgb(0xd4, 0x96, 0x78),
            ansi: [
                rgb(0x2a, 0x24, 0x19),
                rgb(0xb8, 0x45, 0x1f),
                rgb(0x1f, 0x7a, 0x4a),
                rgb(0x7a, 0x5f, 0x1a),
                rgb(0x3a, 0x4a, 0x7a),
                rgb(0x6a, 0x3a, 0x5f),
                rgb(0x3a, 0x6a, 0x7a),
                rgb(0x5a, 0x4a, 0x35),
                rgb(0x5a, 0x4a, 0x35),
                rgb(0xd4, 0x55, 0x2a),
                rgb(0x2a, 0x9a, 0x5a),
                rgb(0x9a, 0x7f, 0x2a),
                rgb(0x4a, 0x5a, 0x9a),
                rgb(0x8a, 0x4a, 0x7a),
                rgb(0x4a, 0x8a, 0x9a),
                rgb(0x2a, 0x24, 0x19),
            ],
            block_bg: rgb(0xef, 0xe7, 0xd2),
            block_border: rgb(0xef, 0xe7, 0xd2),
            block_marker_kind: MarkerKind::Dollar,
            bg_opacity: 1.0,
        }
    }

    /// 04 · Holo Prism — 이리데센트 다이크로익.
    pub const fn holo() -> Self {
        Self {
            name: "holo",
            fg: rgb(0xeb, 0xe8, 0xf6),
            bg: rgb(0x0a, 0x08, 0x1e),
            selection_bg: rgb(0x66, 0x57, 0x8f),
            ansi: [
                rgb(0x1f, 0x1c, 0x3a),
                rgb(0xff, 0x8a, 0xa6),
                rgb(0x7d, 0xff, 0xb0),
                rgb(0xff, 0xcf, 0xa3),
                rgb(0xa0, 0xf0, 0xff),
                rgb(0xc3, 0xa5, 0xff),
                rgb(0xa0, 0xff, 0xe0),
                rgb(0xb0, 0xa8, 0xc8),
                rgb(0x3a, 0x36, 0x5a),
                rgb(0xff, 0xa0, 0xc0),
                rgb(0xa0, 0xff, 0xc8),
                rgb(0xff, 0xe0, 0xb8),
                rgb(0xc0, 0xff, 0xff),
                rgb(0xd8, 0xc0, 0xff),
                rgb(0xc0, 0xff, 0xf0),
                rgb(0xeb, 0xe8, 0xf6),
            ],
            block_bg: rgb(0x0a, 0x08, 0x1e),
            block_border: rgb(0x0a, 0x08, 0x1e),
            block_marker_kind: MarkerKind::Hex,
            bg_opacity: 0.85,
        }
    }

    /// 05 · Bento Pop — 청키 3D, 따뜻한 베이지. 굵은 채도 + 검정 텍스트.
    pub const fn bento() -> Self {
        Self {
            name: "bento",
            fg: rgb(0x1f, 0x1a, 0x14),
            bg: rgb(0xe9, 0xdf, 0xc1),
            selection_bg: rgb(0x87, 0xa1, 0xd6),
            ansi: [
                rgb(0x1f, 0x1a, 0x14),
                rgb(0xd8, 0x3a, 0x3a),
                rgb(0x10, 0x89, 0x4b),
                rgb(0xb0, 0x7a, 0x1a),
                rgb(0x25, 0x63, 0xeb),
                rgb(0x80, 0x44, 0xd6),
                rgb(0x2a, 0x8a, 0x96),
                rgb(0x5a, 0x4f, 0x3a),
                rgb(0x5a, 0x4f, 0x3a),
                rgb(0xf0, 0x4a, 0x4a),
                rgb(0x20, 0xa9, 0x5b),
                rgb(0xd0, 0x9a, 0x2a),
                rgb(0x40, 0x7f, 0xff),
                rgb(0xa0, 0x5f, 0xf0),
                rgb(0x3f, 0xaa, 0xb6),
                rgb(0x1f, 0x1a, 0x14),
            ],
            block_bg: rgb(0xe9, 0xdf, 0xc1),
            block_border: rgb(0xe9, 0xdf, 0xc1),
            block_marker_kind: MarkerKind::RunChip,
            bg_opacity: 1.0,
        }
    }

    /// 06 · Crystal — 두꺼운 굴절 글래스, 차가운 청록/보라.
    pub const fn crystal() -> Self {
        Self {
            name: "crystal",
            fg: rgb(0xe6, 0xe8, 0xf4),
            bg: rgb(0x0a, 0x0f, 0x24),
            selection_bg: rgb(0x34, 0x70, 0x74),
            ansi: [
                rgb(0x1a, 0x20, 0x40),
                rgb(0xff, 0x7e, 0xa0),
                rgb(0x7e, 0xe8, 0xb0),
                rgb(0xff, 0xd2, 0x9a),
                rgb(0x5e, 0xd2, 0xc5),
                rgb(0xcf, 0xa9, 0xff),
                rgb(0x80, 0xe8, 0xe0),
                rgb(0xa0, 0xa8, 0xc0),
                rgb(0x2a, 0x30, 0x55),
                rgb(0xff, 0x9a, 0xb8),
                rgb(0xa0, 0xff, 0xc8),
                rgb(0xff, 0xe0, 0xb8),
                rgb(0x80, 0xe8, 0xe0),
                rgb(0xe0, 0xc0, 0xff),
                rgb(0xa0, 0xff, 0xf0),
                rgb(0xe6, 0xe8, 0xf4),
            ],
            block_bg: rgb(0x0a, 0x0f, 0x24),
            block_border: rgb(0x0a, 0x0f, 0x24),
            block_marker_kind: MarkerKind::Bubble,
            bg_opacity: 0.55,
        }
    }

    /// 기본 테마 — 현재 다크 색 상수(#0d0d12 → 0x07091a 근사)와 매칭되는 Obsidian.
    pub const fn default_theme() -> Self {
        Self::obsidian()
    }
}

impl fmt::Display for ThemePalette {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_themes_constructible() {
        let names = ["aurora", "obsidian", "vellum", "holo", "bento", "crystal"];
        for n in names {
            let p = ThemePalette::by_name(n).unwrap_or_else(|| panic!("{n} not found"));
            assert_eq!(p.name, n);
            assert_eq!(p.fg[3], 1.0);
            assert_eq!(p.bg[3], 1.0);
            assert_eq!(p.selection_bg[3], 1.0);
        }
    }

    #[test]
    fn by_name_unknown_returns_none() {
        assert!(ThemePalette::by_name("solarized").is_none());
    }

    #[test]
    fn default_is_obsidian() {
        assert_eq!(ThemePalette::default_theme().name, "obsidian");
    }

    #[test]
    fn light_themes_have_bright_bg() {
        // Aurora/Vellum/Bento bg luminance가 fg보다 높아야 light.
        for n in ["aurora", "vellum", "bento"] {
            let p = ThemePalette::by_name(n).unwrap();
            let bg_lum = p.bg[0] + p.bg[1] + p.bg[2];
            let fg_lum = p.fg[0] + p.fg[1] + p.fg[2];
            assert!(
                bg_lum > fg_lum,
                "{n} expected light theme bg>{:?} fg>{:?}",
                p.bg,
                p.fg
            );
        }
    }

    #[test]
    fn dark_themes_have_dark_bg() {
        for n in ["obsidian", "holo", "crystal"] {
            let p = ThemePalette::by_name(n).unwrap();
            let bg_lum = p.bg[0] + p.bg[1] + p.bg[2];
            let fg_lum = p.fg[0] + p.fg[1] + p.fg[2];
            assert!(bg_lum < fg_lum, "{n} expected dark theme");
        }
    }

    #[test]
    fn block_tokens_obsidian_distinct_from_bg() {
        // design §12: obsidian만 4b 시각 발동, 다른 5 테마는 raw fallback (bg와 동일).
        let obs = ThemePalette::obsidian();
        assert_ne!(obs.block_bg, obs.bg);
        assert_ne!(obs.block_border, obs.bg);
        assert_ne!(obs.block_bg, obs.block_border);
    }

    #[test]
    fn block_tokens_other_themes_fallback_to_bg() {
        for n in ["aurora", "vellum", "holo", "bento", "crystal"] {
            let p = ThemePalette::by_name(n).unwrap();
            assert_eq!(p.block_bg, p.bg, "{n} block_bg should match bg (fallback)");
            assert_eq!(
                p.block_border, p.bg,
                "{n} block_border should match bg (fallback)"
            );
        }
    }
}
