//! macOS visual bell — NSApp.requestUserAttention(NSCriticalRequest).
//! BEL(0x07) 수신 시 dock bounce + 윈도우가 background면 사용자 attention 요청.

use objc2::msg_send;
use objc2::runtime::AnyObject;

// NSRequestUserAttentionType: NSCriticalRequest=0, NSInformationalRequest=10.
// dock bounce는 둘 다 가능하지만 critical은 사용자가 dismiss할 때까지, informational은 1회.
// BEL은 1회로 충분.
const NS_INFORMATIONAL_REQUEST: i64 = 10;

pub fn request_user_attention() {
    unsafe {
        let cls = objc2::class!(NSApplication);
        let app: *mut AnyObject = msg_send![cls, sharedApplication];
        if app.is_null() {
            return;
        }
        let _: i64 = msg_send![app, requestUserAttention: NS_INFORMATIONAL_REQUEST];
    }
}

/// NSBeep — system audible bell. AppKit C function `NSBeep()`.
/// objc2-app-kit이 NSBeep typed binding 제공 안 함. extern "C" 직접 호출.
pub fn ns_beep() {
    unsafe extern "C" {
        fn NSBeep();
    }
    unsafe { NSBeep() };
}
