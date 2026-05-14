//! macOS NSTextInputContext wake-up helper.
//!
//! winit 0.30.13 macOS는 `set_ime_allowed(true)` 호출 시 flag만 set하고 IME activation은
//! 첫 `setMarkedText` 호출 시까지 지연된다(view.rs:880, 301). 결과적으로 입력 소스를
//! 영어→한국어로 전환한 직후 첫 키가 `KeyboardInput.text=Some("ㅎ")`로 escape되고,
//! `Ime::Enabled/Preedit`는 두 번째 키부터 발화된다.
//!
//! 본 모듈은 winit Window의 NSView를 raw_window_handle로 얻어 `NSView.inputContext` →
//! `activate()` + `invalidateCharacterCoordinates()` 직접 호출로 IME를 강제 wake-up.
//! Apple 문서는 `activate()` 직접 호출을 권장하지 않지만, first-key escape를 막는 유일한
//! 실용 fix (Codex 분석 thread `019e2491`).
//!
//! 출처:
//! - winit 0.30.13 macos view.rs first-key path:
//!   https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/macos/view.rs
//! - NSView.inputContext docs:
//!   https://developer.apple.com/documentation/appkit/nsview/inputcontext
//! - 동일 증상 Ghostty discussion:
//!   https://github.com/ghostty-org/ghostty/discussions/9213

use objc2::msg_send;
use objc2::runtime::AnyObject;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// winit Window에서 NSView를 얻어 NSTextInputContext를 강제 activate.
/// macOS 외에서는 no-op (target_os = "macos"로 conditional compile).
pub fn wake_input_context(window: &Window) {
    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(e) => {
            log::warn!("macos_ime: window_handle failed: {e:?}");
            return;
        }
    };
    let RawWindowHandle::AppKit(ns) = handle.as_raw() else {
        log::warn!("macos_ime: not an AppKit window handle");
        return;
    };
    // ns.ns_view: NonNull<c_void>. 안전성: winit이 NSView를 보장하는 동안만 유효.
    unsafe {
        let view: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if view.is_null() {
            log::warn!("macos_ime: ns_view null");
            return;
        }
        // NSView.inputContext returns NSTextInputContext or nil.
        let input_context: *mut AnyObject = msg_send![view, inputContext];
        if input_context.is_null() {
            log::debug!("macos_ime: NSView.inputContext is nil (NSTextInputClient not conformed?)");
            return;
        }
        let _: () = msg_send![input_context, invalidateCharacterCoordinates];
        let _: () = msg_send![input_context, activate];
        log::debug!("macos_ime: NSTextInputContext woken (activate + invalidateCoordinates)");
    }
}
