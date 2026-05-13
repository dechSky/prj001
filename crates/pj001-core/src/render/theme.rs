//! 테마 팔레트 — Claude Design handoff 6 테마(themes-handoff.md §2)에서
//! 추출한 fg/bg/selection 색을 보관한다. 슬라이스 1.1: fg/bg/selection만 도입.
//! ANSI 16색·accent/ok/red 같은 role 색은 후속 슬라이스에서 확장.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemePalette {
    pub name: &'static str,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub selection_bg: [f32; 4],
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
    pub const fn aurora() -> Self {
        Self {
            name: "aurora",
            fg: rgb(0x2b, 0x27, 0x34),
            bg: rgb(0xf6, 0xf2, 0xeb),
            // accent #7a5af8 ↔ bg를 0.5 mix한 톤
            selection_bg: rgb(0xb9, 0xa6, 0xc2),
        }
    }

    /// 02 · Obsidian Glass — 다크 보이드 + 시안 네온.
    pub const fn obsidian() -> Self {
        Self {
            name: "obsidian",
            fg: rgb(0xe8, 0xe7, 0xef),
            bg: rgb(0x07, 0x09, 0x1a),
            // accent #3edaff ↔ bg를 0.5 mix
            selection_bg: rgb(0x23, 0x72, 0x8e),
        }
    }

    /// 03 · Vellum — 따뜻한 종이결.
    pub const fn vellum() -> Self {
        Self {
            name: "vellum",
            fg: rgb(0x2a, 0x24, 0x19),
            bg: rgb(0xef, 0xe7, 0xd2),
            // accent #b8451f ↔ bg를 0.5 mix
            selection_bg: rgb(0xd4, 0x96, 0x78),
        }
    }

    /// 04 · Holo Prism — 이리데센트 다이크로익.
    pub const fn holo() -> Self {
        Self {
            name: "holo",
            fg: rgb(0xeb, 0xe8, 0xf6),
            bg: rgb(0x0a, 0x08, 0x1e),
            // accent #c3a5ff ↔ bg를 0.5 mix
            selection_bg: rgb(0x66, 0x57, 0x8f),
        }
    }

    /// 05 · Bento Pop — 청키 3D, 따뜻한 베이지.
    pub const fn bento() -> Self {
        Self {
            name: "bento",
            fg: rgb(0x1f, 0x1a, 0x14),
            bg: rgb(0xe9, 0xdf, 0xc1),
            // accent #2563eb ↔ bg를 0.5 mix
            selection_bg: rgb(0x87, 0xa1, 0xd6),
        }
    }

    /// 06 · Crystal — 두꺼운 굴절 글래스.
    pub const fn crystal() -> Self {
        Self {
            name: "crystal",
            fg: rgb(0xe6, 0xe8, 0xf4),
            bg: rgb(0x0a, 0x0f, 0x24),
            // accent #5ed2c5 ↔ bg를 0.5 mix (감색이 강해 톤 다운)
            selection_bg: rgb(0x34, 0x70, 0x74),
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
    [
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        1.0,
    ]
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
}
