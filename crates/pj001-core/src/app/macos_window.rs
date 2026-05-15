//! macOS NSWindow policy glue.
//!
//! M-W-8: pj001 uses macOS NSWindow tabs as the user-visible tab system. The
//! in-app tab model remains as an internal single-tab container for pane/layout
//! state during the migration.

use std::sync::Once;

use objc2::msg_send;
use objc2::runtime::{AnyObject, Bool};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

static ENABLE_AUTOMATIC_TABBING_ONCE: Once = Once::new();

/// Enable native macOS tabbing for this app/window.
pub fn enable_native_tabbing(window: &Window) {
    ENABLE_AUTOMATIC_TABBING_ONCE.call_once(|| unsafe {
        let cls = objc2::class!(NSWindow);
        let _: () = msg_send![cls, setAllowsAutomaticWindowTabbing: Bool::YES];
        log::info!("macos_window: enabled NSWindow automatic tabbing");
    });

    let Some(ns_window) = ns_window(window) else {
        return;
    };
    unsafe {
        // NSWindowTabbingModePreferred = 1.
        let _: () = msg_send![ns_window, setTabbingMode: 1usize];
    }
}

/// Attach `new_window` as a native tab of `base_window`.
pub fn add_window_as_tab(base_window: &Window, new_window: &Window) {
    let (Some(base), Some(new)) = (ns_window(base_window), ns_window(new_window)) else {
        return;
    };
    unsafe {
        // NSWindowAbove = 1.
        let _: () = msg_send![base, addTabbedWindow: new, ordered: 1isize];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![new, makeKeyAndOrderFront: nil];
    }
}

pub fn select_next_tab(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    unsafe {
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![ns_window, selectNextTab: nil];
    }
}

pub fn select_previous_tab(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    unsafe {
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![ns_window, selectPreviousTab: nil];
    }
}

pub fn toggle_tab_overview(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    unsafe {
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![ns_window, toggleTabOverview: nil];
    }
}

pub fn select_tab_at_index(window: &Window, index: usize) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    unsafe {
        let tabbed_windows: *mut AnyObject = msg_send![ns_window, tabbedWindows];
        if tabbed_windows.is_null() {
            return;
        }
        let count: usize = msg_send![tabbed_windows, count];
        if index >= count {
            return;
        }
        let target: *mut AnyObject = msg_send![tabbed_windows, objectAtIndex: index];
        if target.is_null() {
            return;
        }
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![target, makeKeyAndOrderFront: nil];
    }
}

pub fn tab_ordinal(window: &Window) -> Option<usize> {
    let ns_window = ns_window(window)?;
    unsafe {
        let tabbed_windows: *mut AnyObject = msg_send![ns_window, tabbedWindows];
        if tabbed_windows.is_null() {
            return None;
        }
        let count: usize = msg_send![tabbed_windows, count];
        for index in 0..count {
            let candidate: *mut AnyObject = msg_send![tabbed_windows, objectAtIndex: index];
            if candidate == ns_window {
                return Some(index + 1);
            }
        }
    }
    None
}

fn ns_window(window: &Window) -> Option<*mut AnyObject> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(ns) = handle.as_raw() else {
        return None;
    };
    unsafe {
        let view: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if view.is_null() {
            return None;
        }
        let ns_window: *mut AnyObject = msg_send![view, window];
        (!ns_window.is_null()).then_some(ns_window)
    }
}
