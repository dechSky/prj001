//! macOS first-key IME workaround — 자체 NSView subclass + NSTextInputClient PoC.
//!
//! winit 0.30.13 macOS는 `set_ime_allowed(true)` 호출 후에도 입력 소스 영어→한국어
//! 전환 직후 첫 자모를 IME path가 아닌 `KeyboardInput.text=Some("ㅎ")`로 직송한다
//! (Codex thread `019e2491`, `019e24d9` 분석). winit `WinitView`가 `NSTextInputClient`를
//! 구현해도 first responder가 처음부터 우리 view가 아니라 IME state가 lazy 활성화된다.
//!
//! WezTerm은 자체 NSView + NSTextInputClient를 first responder로 두어 첫 키부터 IME path
//! 정상 처리.
//! 본 모듈은 그 패턴을 PoC로 시도 — `PjImeView`(NSView subclass)를 winit view subview로
//! 추가하고 `makeFirstResponder`로 빼앗아 NSTextInputClient 이벤트를 받는다.
//!
//! PoC v0.1 단계: setMarkedText / insertText / hasMarkedText 등 최소 메서드만. 호출 시
//! `EventLoopProxy<UserEvent>`로 UserEvent::MacImeCommit/MacImePreedit 송신. AppState
//! 연결은 검증 후.
//!
//! 출처: WezTerm `window/src/os/macos/window.rs`, Apple `NSTextInputClient` docs.

#![allow(unused_imports)] // v0.2에서 NSView subclass 정의 시 사용.

use std::cell::RefCell;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::app::event::UserEvent;

/// PoC v0.1 — install_ime_view는 winit window NSView를 얻어 superview로 사용하고
/// 우리 IME view를 subview로 추가한다. PoC 단계는 stub — 실제 NSView subclass 정의 +
/// makeFirstResponder는 다음 단계.
///
/// 현재 phase: 기존 wake_input_context와 호환. NSView subclass 정의가 objc2 0.6 macro
/// 학습 필요해 단계적 진행.
pub fn install_ime_view(window: &Window, _proxy: EventLoopProxy<UserEvent>) {
    // 임시: 기존 wake_input_context와 동일하게 NSTextInputContext.activate 호출.
    // 다음 단계에서 PjImeView subclass로 대체.
    wake_input_context(window);
    log::debug!("macos_ime: install_ime_view PoC v0.1 stub — using wake_input_context fallback");
}

/// 기존 wake_input_context (활성화 시도). PoC 단계 fallback.
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
    unsafe {
        let view: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if view.is_null() {
            log::warn!("macos_ime: ns_view null");
            return;
        }
        let input_context: *mut AnyObject = msg_send![view, inputContext];
        if input_context.is_null() {
            log::debug!("macos_ime: NSView.inputContext is nil");
            return;
        }
        let _: () = msg_send![input_context, invalidateCharacterCoordinates];
        let _: () = msg_send![input_context, activate];
        log::debug!("macos_ime: NSTextInputContext woken (activate + invalidateCoordinates)");
    }
}

// =================================================================================
// PoC v0.2 — PjImeView NSView subclass (objc2 0.6 define_class!)
// 다음 commit에서 활성화. 현재는 컴파일 검증 + 구조 placeholder.
// =================================================================================

#[allow(dead_code)]
struct PjImeIvars {
    proxy: RefCell<Option<EventLoopProxy<UserEvent>>>,
    marked_text: RefCell<String>,
}

// NOTE: objc2 0.6 define_class!를 사용한 NSView subclass + NSTextInputClient protocol
// 구현은 작업량이 크고 한 번에 컴파일 검증이 어려워 다음 commit으로 분리. 본 commit은
// install_ime_view shell + UserEvent::MacImeCommit/MacImePreedit variants 추가 + 기존
// wake_input_context fallback 유지로 PoC 인프라만 완성.
//
// 다음 단계 단순화 design:
// - `objc2::define_class!`로 `PjImeView: NSView` 생성
// - Ivars { proxy, marked_text: String, marked_range_loc, marked_range_len }
// - selectors:
//   - hasMarkedText -> bool
//   - markedRange -> NSRange
//   - selectedRange -> NSRange
//   - setMarkedText:selectedRange:replacementRange: (NSString or NSAttributedString)
//   - unmarkText
//   - insertText:replacementRange: (NSString)
//   - attributedSubstringForProposedRange:actualRange: -> NSAttributedString (nil OK)
//   - validAttributesForMarkedText -> NSArray (empty OK)
//   - firstRectForCharacterRange:actualRange: -> NSRect
//   - characterIndexForPoint: -> NSUInteger (NSNotFound OK)
//   - doCommandBySelector: (no-op or forward)
//   - keyDown: (super + inputContext.handleEvent)
// - install: alloc PjImeView, set proxy ivar, addSubview to winit NSView,
//   window.makeFirstResponder(view), 0-sized frame or hitTest:nil for wgpu surface 보호
//
// 검증 기준 (PoC v0.2):
// 1. 영어→한국어 전환 후 첫 ㅎ 입력 시 setMarkedText 또는 insertText 호출 → log
// 2. 기존 KeyboardInput.text=Some("ㅎ") 더 이상 발화 안 함
// 3. wgpu surface 정상 (winit view 위에 잘 덮임)
// 4. 영문 입력 / Cmd 단축키 forward 정상
