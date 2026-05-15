//! macOS NSVisualEffectView vibrancy attach — M-P3-2b 직접 구현.
//!
//! M-P3-2a에서 도입한 WgpuOverlay sibling 패턴 위에 NSVisualEffectView를 또 다른
//! sibling subview로 추가한다. subview 순서:
//! - subview[0] (가장 뒤) = NSVisualEffectView
//! - subview[1] (가장 앞) = WgpuOverlay (CAMetalLayer)
//!
//! window-vibrancy crate는 winit_view layer를 wgpu가 점유한 가정에서 작동 안 함
//! (M-P3-2 진단). 직접 objc2 typed binding으로 attach.
//!
//! 테마별 NSVisualEffectMaterial 선택은 `material_for_theme` helper로 분기.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2_app_kit::{
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{MainThreadMarker, NSRect};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::render::ThemePalette;

// NSAutoresizingMaskOptions: NSViewWidthSizable=2 | NSViewHeightSizable=16
const NS_VIEW_WIDTH_HEIGHT_SIZABLE: u64 = 2 | 16;

/// 테마별 NSVisualEffectMaterial 매핑. themes-handoff.md §2 시각 의도 따라:
/// - aurora (라이트, 소프트 파스텔) → underWindowBackground (밝은 vibrancy)
/// - obsidian (다크 보이드) → hudWindow (반투명 다크)
/// - vellum (페이퍼) → 거의 opaque이라 windowBackground (vibrancy 최소)
/// - holo (홀로 prism) → hudWindow + state Active
/// - bento (warm light) → underWindowBackground
/// - crystal (radial deep) → hudWindow
fn material_for_theme(palette: &ThemePalette) -> NSVisualEffectMaterial {
    match palette.name {
        "aurora" => NSVisualEffectMaterial::UnderWindowBackground,
        "obsidian" => NSVisualEffectMaterial::HUDWindow,
        "vellum" => NSVisualEffectMaterial::WindowBackground,
        "holo" => NSVisualEffectMaterial::HUDWindow,
        "bento" => NSVisualEffectMaterial::UnderWindowBackground,
        "crystal" => NSVisualEffectMaterial::HUDWindow,
        _ => NSVisualEffectMaterial::HUDWindow,
    }
}

/// NSVisualEffectView attach 결과. App이 Retained를 보관 (drop 시 release).
pub struct BackdropAttach {
    #[allow(dead_code)]
    nsve: Retained<NSVisualEffectView>,
}

/// winit_view에 NSVisualEffectView를 sibling subview (가장 뒤)로 추가.
/// WgpuOverlay (M-P3-2a)가 이미 attach된 후 호출되어야 한다 — 그래야 NSVE가 subview[0],
/// WgpuOverlay가 subview[1]로 자연 z-order 정렬.
pub fn attach_visual_effect(window: &Window, palette: &ThemePalette) -> Option<BackdropAttach> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(ns) = handle.as_raw() else {
        log::warn!("macos_backdrop: not an AppKit handle");
        return None;
    };
    let mtm = MainThreadMarker::new()?;

    unsafe {
        let winit_view_ptr: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if winit_view_ptr.is_null() {
            log::warn!("macos_backdrop: ns_view null");
            return None;
        }

        // winit_view bounds 받기.
        let bounds: NSRect = msg_send![winit_view_ptr, bounds];

        // NSVisualEffectView 생성 + 속성. typed binding 사용 (raw msg_send retained 함정 회피).
        let nsve = NSVisualEffectView::initWithFrame(mtm.alloc::<NSVisualEffectView>(), bounds);

        let material = material_for_theme(palette);
        nsve.setMaterial(material);
        nsve.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        nsve.setState(NSVisualEffectState::Active);
        nsve.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // winit_view에 subview로 추가 — positioned:Below relativeTo:nil로 가장 뒤(z-order
        // bottom)에. WgpuOverlay가 그 위에 위치 (M-P3-2a에서 이미 첫 번째 추가됨).
        let _: () = msg_send![
            winit_view_ptr,
            addSubview: &*nsve,
            positioned: NS_WINDOW_BELOW,
            relativeTo: std::ptr::null::<AnyObject>(),
        ];

        // NSWindow를 opaque=NO로 명시 (winit with_transparent도 처리하나 idempotent safety).
        let ns_window: *mut AnyObject = msg_send![winit_view_ptr, window];
        if !ns_window.is_null() {
            let _: () = msg_send![ns_window, setOpaque: Bool::NO];
        }

        log::info!(
            "macos_backdrop: NSVisualEffectView attached material={:?} blending=BehindWindow state=Active theme={}",
            material,
            palette.name,
        );
        Some(BackdropAttach { nsve })
    }
}

// NSWindowOrderingMode::Below = -1
const NS_WINDOW_BELOW: i64 = -1;

// suppress autoresize 미사용 const 경고.
#[allow(dead_code)]
const _: u64 = NS_VIEW_WIDTH_HEIGHT_SIZABLE;
