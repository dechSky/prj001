//! macOS NSVisualEffectView backdrop attach via window-vibrancy crate.
//!
//! 직접 NSWindow.setContentView로 NSVisualEffectView를 wrap하면 winit 0.30의
//! NSView 내부 RefCell(cursor_state/IME)이 contentView 교체 후 AppKit 재진입으로
//! borrow_mut panic을 일으킨다 (winit view.rs:863, 901). 검증된 tauri-apps의
//! window-vibrancy crate가 이 상호작용을 이미 해결해놨다.
//!
//! - <https://github.com/tauri-apps/window-vibrancy>
//! - <https://docs.rs/window-vibrancy/0.7.1>

use winit::window::Window;

/// winit Window에 NSVisualEffectView vibrancy를 적용한다.
/// 호출 후 윈도우 뒤 데스크톱/다른 앱이 vibrancy blur로 비침.
pub fn attach_visual_effect(window: &Window) {
    // HudWindow material — advisor 권 (반투명 HUD 패널 스타일). step 4에서 테마별 분기 예정.
    // state=Active: 포커스와 무관하게 항상 vibrancy. radius=None: 기본 (corner radius 적용 X).
    match window_vibrancy::apply_vibrancy(
        window,
        window_vibrancy::NSVisualEffectMaterial::HudWindow,
        Some(window_vibrancy::NSVisualEffectState::Active),
        None,
    ) {
        Ok(()) => {
            log::info!(
                "macos_backdrop: NSVisualEffectView attached material=HudWindow state=Active (window-vibrancy)"
            );
        }
        Err(e) => {
            log::warn!("macos_backdrop: apply_vibrancy failed: {e:?}");
        }
    }
}
