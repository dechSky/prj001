//! macOS first-key IME workaround - NSView subclass + NSTextInputClient PoC v0.2.
//!
//! Codex thread `019e24d9-0686-7371-adea-6ba471f57990` design.

#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Sel};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{NSEvent, NSResponder, NSTextInputClient, NSTextInputContext, NSView};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSObjectProtocol, NSPoint,
    NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::app::event::UserEvent;

struct PjImeIvars {
    proxy: RefCell<Option<EventLoopProxy<UserEvent>>>,
    marked_text: RefCell<String>,
    marked_range: Cell<NSRange>,
    winit_view: Cell<*mut AnyObject>,
    ime_callback_seen: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "Pj001ImeView"]
    #[ivars = PjImeIvars]
    struct PjImeView;

    impl PjImeView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.ivars().ime_callback_seen.set(false);

            unsafe {
                let input_context: *mut NSTextInputContext = msg_send![self, inputContext];
                if !input_context.is_null() {
                    let handled: bool = msg_send![input_context, handleEvent: event];
                    if handled || self.ivars().ime_callback_seen.get() {
                        log::debug!(
                            "macos_ime: keyDown handled by NSTextInputContext handled={handled}"
                        );
                        return;
                    }
                }

                let winit_view = self.ivars().winit_view.get();
                if !winit_view.is_null() {
                    let _: () = msg_send![winit_view, keyDown: event];
                } else {
                    let _: () = msg_send![super(self), keyDown: event];
                }
            }
        }
    }

    unsafe impl NSObjectProtocol for PjImeView {}

    unsafe impl NSTextInputClient for PjImeView {
        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            !self.ivars().marked_text.borrow().is_empty()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            if self.ivars().marked_text.borrow().is_empty() {
                not_found_range()
            } else {
                self.ivars().marked_range.get()
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            not_found_range()
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn set_marked_text(
            &self,
            string: &AnyObject,
            selected_range: NSRange,
            _replacement_range: NSRange,
        ) {
            let text = unsafe { object_to_string(string) };
            let cursor_byte = cursor_byte_from_nsrange(&text, selected_range);

            *self.ivars().marked_text.borrow_mut() = text.clone();
            self.ivars().marked_range.set(if text.is_empty() {
                not_found_range()
            } else {
                NSRange::new(0, text.encode_utf16().count())
            });
            self.ivars().ime_callback_seen.set(true);

            log::debug!(
                "macos_ime: setMarkedText text={:?} selected={:?} cursor_byte={:?}",
                text, selected_range, cursor_byte
            );

            self.send(UserEvent::MacImePreedit { text, cursor_byte });
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            let text = self.ivars().marked_text.replace(String::new());
            self.ivars().marked_range.set(not_found_range());
            self.ivars().ime_callback_seen.set(true);

            log::debug!("macos_ime: unmarkText prior_marked={:?}", text);
            if !text.is_empty() {
                self.send(UserEvent::MacImeCommit(text));
            }
        }

        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert_text(&self, string: &AnyObject, _replacement_range: NSRange) {
            let text = unsafe { object_to_string(string) };

            self.ivars().marked_text.borrow_mut().clear();
            self.ivars().marked_range.set(not_found_range());
            self.ivars().ime_callback_seen.set(true);

            log::debug!("macos_ime: insertText {:?}", text);
            if !text.is_empty() {
                self.send(UserEvent::MacImeCommit(text));
            }
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        unsafe fn attributed_substring_for_proposed_range(
            &self,
            _range: NSRange,
            actual_range: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            if !actual_range.is_null() {
                unsafe {
                    *actual_range = not_found_range();
                }
            }
            None
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        unsafe fn first_rect_for_character_range(
            &self,
            _range: NSRange,
            actual_range: NSRangePointer,
        ) -> NSRect {
            if !actual_range.is_null() {
                unsafe {
                    *actual_range = NSRange::new(0, 0);
                }
            }
            NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(1.0, 18.0))
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            NSNotFound as NSUInteger
        }

        #[unsafe(method(doCommandBySelector:))]
        unsafe fn do_command_by_selector(&self, selector: Sel) {
            self.ivars().ime_callback_seen.set(true);
            log::debug!("macos_ime: doCommandBySelector {:?}", selector);
            // PoC: command 소비. winit으로 forward하면 중복 dispatch 위험.
        }
    }
);

impl PjImeView {
    fn new(
        frame: NSRect,
        proxy: EventLoopProxy<UserEvent>,
        winit_view: *mut AnyObject,
    ) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("PjImeView must be created on main thread");

        let this: Allocated<Self> = Self::alloc(mtm);
        let this = this.set_ivars(PjImeIvars {
            proxy: RefCell::new(Some(proxy)),
            marked_text: RefCell::new(String::new()),
            marked_range: Cell::new(not_found_range()),
            winit_view: Cell::new(winit_view),
            ime_callback_seen: Cell::new(false),
        });

        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn send(&self, event: UserEvent) {
        if let Some(proxy) = self.ivars().proxy.borrow().as_ref()
            && let Err(e) = proxy.send_event(event)
        {
            log::warn!("macos_ime: EventLoopProxy send_event failed: {e:?}");
        }
    }
}

pub fn install_ime_view(window: &Window, proxy: EventLoopProxy<UserEvent>) {
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
        let winit_view: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if winit_view.is_null() {
            log::warn!("macos_ime: ns_view null");
            return;
        }

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        let ime_view = PjImeView::new(frame, proxy, winit_view);

        let _: () = msg_send![winit_view, addSubview: &*ime_view];

        let ns_window: *mut AnyObject = msg_send![winit_view, window];
        if ns_window.is_null() {
            log::warn!("macos_ime: winit NSView has no window");
            return;
        }

        let ok: bool = msg_send![ns_window, makeFirstResponder: &*ime_view];
        log::info!("macos_ime: PjImeView installed, makeFirstResponder={ok}");

        // addSubview가 retain하므로 ime_view는 drop되어도 subview로 생존.
    }
}

#[allow(dead_code)]
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

        let input_context: *mut NSTextInputContext = msg_send![view, inputContext];
        if input_context.is_null() {
            log::debug!("macos_ime: NSView.inputContext is nil");
            return;
        }

        let _: () = msg_send![input_context, invalidateCharacterCoordinates];
        let _: () = msg_send![input_context, activate];
        log::debug!("macos_ime: NSTextInputContext woken");
    }
}

const fn not_found_range() -> NSRange {
    NSRange {
        location: NSNotFound as NSUInteger,
        length: 0,
    }
}

unsafe fn object_to_string(object: &AnyObject) -> String {
    unsafe {
        let is_attr: bool = msg_send![object, isKindOfClass: NSAttributedString::class()];
        if is_attr {
            let attr = object as *const AnyObject as *const NSAttributedString;
            (*attr).string().to_string()
        } else {
            let s = object as *const AnyObject as *const NSString;
            (*s).to_string()
        }
    }
}

fn cursor_byte_from_nsrange(text: &str, selected_range: NSRange) -> Option<usize> {
    if selected_range.location == NSNotFound as NSUInteger {
        return None;
    }
    utf16_location_to_byte(text, selected_range.location as usize)
}

fn utf16_location_to_byte(text: &str, utf16_location: usize) -> Option<usize> {
    let mut units = 0usize;
    for (byte, ch) in text.char_indices() {
        if units == utf16_location {
            return Some(byte);
        }
        units += ch.len_utf16();
        if units > utf16_location {
            return None;
        }
    }

    if units == utf16_location {
        Some(text.len())
    } else {
        None
    }
}
