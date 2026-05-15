//! macOS WgpuOverlay — winit_view 위에 sibling subview로 깔리는 CAMetalLayer-backed NSView.
//!
//! Phase 3 step 2 재설계 (옵션 F, `docs/backdrop-redesign.md`):
//! - winit_view를 NSWindow.contentView 그대로 유지 (실패 1의 winit RefCell 재진입 회피)
//! - wgpu surface는 winit_view.layer 대신 우리가 만든 CAMetalLayer에 직접 생성
//!   (`SurfaceTargetUnsafe::CoreAnimationLayer`)
//! - winit_view.layer는 vanilla CALayer로 남아 NSVE(M-P3-2b에서 추가)가 그 위로 비치는 토대
//!
//! M-P3-2a 범위: WgpuOverlay만 부착, NSVE는 미부착. 시각적으로 step 1과 동일.
//!
//! Layer 합성 stack (bottom → top, M-P3-2a):
//! ```text
//! 1. NSWindow background (clearColor — winit with_transparent)
//! 2. winit_view.layer (vanilla CALayer — 빈 transparent)
//! 3. WgpuOverlay.layer (CAMetalLayer, isOpaque=NO) ← 우리 cell/text
//! ```
//!
//! M-P3-2b에서 winit_view에 NSVisualEffectView를 sibling subview(Below)로 추가하면
//! layer-stack 2 위·3 아래에 NSVE가 들어와 cell.bg.alpha<1 영역에서 vibrancy가 비친다.

use std::ffi::c_void;

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject};
use objc2_app_kit::NSView;
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

// NSAutoresizingMaskOptions: NSViewWidthSizable=2 | NSViewHeightSizable=16
const NS_VIEW_WIDTH_HEIGHT_SIZABLE: u64 = 2 | 16;

define_class!(
    /// winit_view의 sibling subview. backing layer가 우리가 관리하는 CAMetalLayer.
    /// hitTest=nil 반환으로 모든 마우스 이벤트를 sibling/superview(winit_view)에 pass-through.
    #[unsafe(super(NSView))]
    #[name = "Pj001WgpuOverlay"]
    #[ivars = ()]
    struct WgpuOverlay;

    impl WgpuOverlay {
        /// 키 포커스는 winit_view에 맡김.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> Bool {
            Bool::NO
        }

        /// Codex 리뷰: hitTest 회귀 방지. `pointInside:withEvent:`로 항상 NO 반환하면
        /// AppKit이 이 view를 hit detection에서 통과시켜 모든 마우스 이벤트가 winit_view에
        /// 도달한다. CursorMoved/drag/xterm mouse mode/cursor icon 회귀 차단.
        /// Bool 반환은 ABI 안전 (retain 시멘틱 없음).
        #[unsafe(method(pointInside:withEvent:))]
        fn point_inside(&self, _point: NSPoint, _event: *mut AnyObject) -> Bool {
            Bool::NO
        }
    }
);

/// Overlay attach 결과. CAMetalLayer를 wgpu surface 생성에 raw pointer로 넘기기 위해
/// 호출자에게 양도. layer는 WgpuOverlay가 retain하고 있으므로 raw pointer는 overlay
/// 살아있는 동안 유효. overlay/metal_layer 필드는 lifetime 유지용 (drop 시 release).
pub struct OverlayAttach {
    /// CAMetalLayer raw pointer — `wgpu::SurfaceTargetUnsafe::CoreAnimationLayer`에 전달.
    pub metal_layer_ptr: *mut c_void,
    /// WgpuOverlay 강한 참조 — App이 보관 (drop 시 자동 release).
    #[allow(dead_code)]
    overlay: Retained<WgpuOverlay>,
    /// CAMetalLayer 강한 참조 — overlay와 함께 보관 (실제 retain은 WgpuOverlay가 하지만
    /// 추가 안전망).
    #[allow(dead_code)]
    metal_layer: Retained<CAMetalLayer>,
}

/// winit Window에서 NSView를 얻어 sibling subview로 WgpuOverlay를 부착.
/// CAMetalLayer 생성 + isOpaque=NO + contentsScale 동기화 + autoresize 설정.
pub fn attach_overlay(window: &Window) -> Option<OverlayAttach> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(ns) = handle.as_raw() else {
        log::warn!("macos_overlay: not an AppKit window handle");
        return None;
    };
    let scale = window.scale_factor();
    let mtm = MainThreadMarker::new()?;

    unsafe {
        let winit_view_ptr: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if winit_view_ptr.is_null() {
            log::warn!("macos_overlay: ns_view null");
            return None;
        }
        // raw 포인터를 NSView로 cast — winit이 살아있는 동안 유효.
        let winit_view: &NSView = &*(winit_view_ptr as *const NSView);

        // 진단: surface 생성 **전** winit_view.layer 상태 (presence만, class 이름은
        // 매크로 retained-detection 충돌로 별도 helper 필요 → 단순 boolean으로 축소).
        let before_layer: *mut AnyObject = msg_send![winit_view_ptr, layer];
        log::info!(
            "macos_overlay: BEFORE create_surface winit_view.layer={}",
            if before_layer.is_null() {
                "nil"
            } else {
                "present"
            }
        );

        // CAMetalLayer 생성 + 필수 속성. typed `new()` 사용 — objc2가 retained 자동 처리.
        let metal_layer: Retained<CAMetalLayer> = CAMetalLayer::new();
        // pixelFormat = BGRA8Unorm_sRGB (=80, MTLPixelFormat enum 값)
        let _: () = msg_send![&*metal_layer, setPixelFormat: 80usize];
        // framebufferOnly = YES (성능)
        let _: () = msg_send![&*metal_layer, setFramebufferOnly: Bool::YES];
        // contentsScale (retina). f64로 받음.
        let _: () = msg_send![&*metal_layer, setContentsScale: scale];
        // isOpaque = NO (critical — 미설정 시 M-P3-2b에서 vibrancy silent fail)
        let _: () = msg_send![&*metal_layer, setOpaque: Bool::NO];
        // presentsWithTransaction = NO (wgpu surface.present 호환)
        let _: () = msg_send![&*metal_layer, setPresentsWithTransaction: Bool::NO];

        // 진단 로그: isOpaque/presentsWithTransaction 값 확인.
        let is_opaque: Bool = msg_send![&*metal_layer, isOpaque];
        let pwt: Bool = msg_send![&*metal_layer, presentsWithTransaction];
        log::info!(
            "macos_overlay: metal layer isOpaque={} presentsWithTransaction={}",
            is_opaque.as_bool(),
            pwt.as_bool(),
        );

        // WgpuOverlay subclass alloc + init.
        let bounds: NSRect = winit_view.bounds();
        let overlay: Retained<WgpuOverlay> = {
            let alloc = mtm.alloc::<WgpuOverlay>().set_ivars(());
            msg_send![super(alloc), initWithFrame: bounds]
        };
        // wantsLayer + layer 설정. setWantsLayer 먼저 호출하지 않으면 setLayer가 무효 케이스 있음.
        let _: () = msg_send![&*overlay, setWantsLayer: Bool::YES];
        let _: () = msg_send![&*overlay, setLayer: &*metal_layer];
        let _: () = msg_send![&*overlay, setAutoresizingMask: NS_VIEW_WIDTH_HEIGHT_SIZABLE];

        // winit_view에 sub-view로 추가. addSubview만으로도 OK (위치는 마지막 = z-order top).
        // M-P3-2b에서 NSVE를 Below로 추가하면 NSVE는 subview[0], overlay는 subview[1].
        let _: () = msg_send![winit_view_ptr, addSubview: &*overlay];

        // 진단: overlay attach 후 winit_view.layer presence (class 이름 확인은 별도 helper).
        let after_attach_layer: *mut AnyObject = msg_send![winit_view_ptr, layer];
        log::info!(
            "macos_overlay: AFTER overlay attach winit_view.layer={}",
            if after_attach_layer.is_null() {
                "nil"
            } else {
                "present"
            }
        );

        let metal_layer_ptr = Retained::as_ptr(&metal_layer) as *mut c_void;
        Some(OverlayAttach {
            metal_layer_ptr,
            overlay,
            metal_layer,
        })
    }
}

/// wgpu surface 생성 **직후** winit_view.layer.class를 다시 확인.
/// 설계 가정: wgpu가 우리 별도 CAMetalLayer로 surface 생성 → winit_view.layer는 vanilla 그대로.
/// 만약 여기서 CAMetalLayer로 보이면 wgpu가 hidden fallback path로 winit_view.layer를 점유한 것
/// → 설계 가정 깨짐, 중단 신호 (advisor 권).
pub fn log_layer_class_after_surface(window: &Window) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(ns) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view_ptr: *mut AnyObject = ns.ns_view.as_ptr().cast();
        if view_ptr.is_null() {
            return;
        }
        let layer: *mut AnyObject = msg_send![view_ptr, layer];
        log::info!(
            "macos_overlay: AFTER create_surface_unsafe winit_view.layer={} (CAMetalLayer로 변하면 설계 가정 깨짐 — 시각 확인 필요)",
            if layer.is_null() { "nil" } else { "present" }
        );
    }
}

// `Send` 불가능: AppKit object는 main thread 전용. App이 main thread에서만 보관/drop하는
// 가정으로 unsafe impl Send를 피한다. OverlayAttach는 WindowState 안에서 직접 보관.
// Retained<NSObject>는 thread-safety가 NSObject impl에 따라 다르므로 추후 필요 시 검토.
#[allow(dead_code)]
fn _force_module_compiled() -> *const NSObject {
    std::ptr::null()
}
