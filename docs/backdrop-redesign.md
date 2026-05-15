# Phase 3 step 2 재설계 — NSVisualEffectView vibrancy attach

작성일: 2026-05-15
대상 commit: `7be74f7` (Phase 3 step 1 완료 + step 2 실패 2회 후)
관련 파일:
- `crates/pj001-core/src/app/macos_backdrop.rs` (현재 window-vibrancy wrap, default off)
- `crates/pj001-core/src/app/macos_ime.rs` (NSTextInputContext wake-up)
- `crates/pj001-core/src/app/mod.rs` (window 생성 `with_transparent(true)`, surface `alpha_mode=PostMultiplied`, attach 호출 PJ001_BACKDROP=1 opt-in)

## 1. 문제 정의

목표: macOS에서 NSVisualEffectView vibrancy backdrop이 wgpu가 그리는 cell 콘텐츠 **아래에** 보이도록 한다.
시각 결과: 텍스트는 그대로, 빈 cell(`cell.bg.alpha < 1`)에서 vibrancy가 비친다.

### 1.1 실패 1 — `NSWindow.setContentView` 교체

원래 패턴: `NSWindow.contentView`를 새 NSVisualEffectView로 교체, 기존 winit_view를 그 sub-view로 wrap.

증상:
- (a) `winit-0.30.13/.../view.rs:901` `inputContext().expect("input context")` panic — `makeFirstResponder` + retain/release로 해결됨
- (b) `view.rs:863` `cursor_state.borrow_mut()` panic — contentView 교체 후 AppKit이 winit_view에 cursorUpdate를 dispatch하면서 winit 내부 `RefCell<CursorState>` 재진입. winit 0.30.13 NSView ivars(`cursor_state: RefCell`, `input_context: RefCell`)는 unwind 불가 panic으로 abort.

근본 원인: winit의 NSView가 single-borrow를 가정하고 만든 RefCell들은 contentView 교체로 발생하는 AppKit 재진입을 견디지 못한다. retain/release로 회피 불가능한 구조적 결함.

### 1.2 실패 2 — window-vibrancy 0.7.1 `addSubview Below`

`window-vibrancy::apply_vibrancy(window, HudWindow, Active, None)` 호출:
```rust
view.addSubview_positioned_relativeTo(&blurred_view, NSWindowOrderingMode::Below, None);
// macos/vibrancy.rs:59
```
즉 NSVisualEffectView를 winit_view의 sub-view(Below)로 추가. panic은 없으나 텍스트가 가려졌다.

근본 원인:
- winit이 `view.setWantsLayer(true)` 호출 (`winit window_delegate.rs:652`) → winit_view는 layer-backed view.
- `Instance::create_surface(window)` 호출 시 wgpu가 winit_view의 backing layer를 CAMetalLayer로 교체 → **winit_view.layer == 우리 cell이 그려지는 CAMetalLayer**.
- AppKit layer-backed view에서 **sub-view의 backing layer는 항상 host view의 backing layer "위"에 합성**. NSVE를 sub-view(Below)로 넣어도 host(winit_view) layer "위"에 NSVE layer가 올라간다.
- 결과: NSVE의 vibrancy material이 wgpu가 그린 텍스트를 덮음.

핵심 사실: **wgpu가 winit_view의 backing layer를 점유하는 한** NSVE를 같은 view 안에 sibling/below로 어떻게 넣어도 cell 콘텐츠 위로 올라온다.

### 1.3 진단 분리

`PJ001_NO_BACKDROP=1` (실제는 `PJ001_BACKDROP` 미설정 = default off)로 attach skip → 텍스트 정상 + 윈도우 transparent로 데스크톱 비침. step 1 alpha 배관은 검증됨, step 2 NSVE 위치만 문제.

## 2. 옵션 비교

5가지 후보 + 본 설계에서 권장하는 hybrid path까지 6가지를 평가.

| 옵션 | 핵심 아이디어 | 위험 | 복잡도 | 평가 |
|---|---|---|---|---|
| A — `setContentView` 교체 | NSWindow.contentView = NSVE, winit_view를 sub-view | **panic 재발 확정** (winit RefCell 구조적) | 高 | 기각 |
| B — Custom NSView subclass | objc2 declare_class로 NSTextInputClient + CAMetalLayer backing view를 winit_view 대체 | IME 워크어라운드(`macos_ime.rs`) 전부 재구현 필요. 회귀 폭 막대 | 極高 | 기각 |
| C — raw CAMetalLayer로 wgpu surface | `SurfaceTargetUnsafe::CoreAnimationLayer` 사용해 wgpu가 winit_view layer를 안 건드리게 | 우리가 layer lifecycle, contentsScale, resize 직접 관리 | 中 | 채택 (개량) |
| D — 대안 crate (floem-window-vibrancy 등) | 다른 attach 패턴 기대 | 모두 `addSubview Below` 동일 패턴 (확인됨) | 低 | 기각 |
| E — shader-side blur | wgpu에서 직접 blur pass | **macOS sandbox는 타 앱 픽셀 read-back 차단** — system vibrancy 재현 불가 | 中 | 기각 |
| **F (권장)** — Hybrid: WgpuOverlay sibling | winit_view contentView 유지 + 자식으로 NSVE(below) + WgpuOverlay(above) sibling, wgpu surface는 overlay layer로 직접 생성 | layer lifecycle/scale/resize 자가 관리, hitTest pass-through 검증 필요 | 中 | **권장** |

### 2.1 옵션 A 기각 사유

winit 0.30.13의 NSView ivars는 `RefCell<CursorState>` + `RefCell<...>` 다수 사용. `contentView` 교체는 AppKit이 detach/attach 이벤트를 winit_view로 dispatch하면서 cursorUpdate 같은 event가 borrow_mut 중인 RefCell을 재진입한다. winit 측 fork 없이 우회 불가. 같은 path 재시도 = 동일 panic.

### 2.2 옵션 B 기각 사유

이미 winit 0.30 macOS의 IME 동작에 first-key escape 워크어라운드(`macos_ime.rs::wake_input_context` — NSTextInputContext.activate 강제 호출, Codex thread `019e2491` 분석 기반)를 적용 중. custom NSView로 winit_view를 대체하려면 NSTextInputClient 전체 구현 + IME 상태 머신 재현 + setMarkedText/insertText delegation을 직접 작성해야 한다. 한국어 IME 회귀 위험이 backdrop 가치 대비 압도적.

### 2.3 옵션 C(원안) → F(개량) 결정 이유

원안 옵션 C는 "winit_view의 CAMetalLayer를 빼서 NSWindow contentView 자리(NSVE 안)에 옮긴다" 같은 layer 이동을 시사. 그러나:
- winit_view가 layer를 잃으면 winit이 첫 redraw에서 자기 layer 가정으로 다시 만든다 (또는 충돌).
- 더 단순한 path: **wgpu가 winit_view layer를 처음부터 안 건드리게** 하면 됨. `Instance::create_surface_unsafe(SurfaceTargetUnsafe::CoreAnimationLayer(ptr))`는 우리가 만든 layer로 surface 생성. winit_view는 winit이 만든 vanilla CALayer 그대로 유지.

확인된 wgpu 29 API (`wgpu-29.0.3/src/api/surface.rs:386-392`):
```rust
/// Surface from `CoreAnimationLayer`.
/// # Safety
/// - layer must be a valid object to create a surface upon.
#[cfg(metal)]
CoreAnimationLayer(*mut core::ffi::c_void),
```
구현 (`wgpu_core.rs:820-822`): `self.0.instance_create_surface_metal(layer, None)` — raw pointer 그대로 사용.

### 2.4 옵션 D 기각 사유

floem-window-vibrancy 0.4.3, vibrancy-rs 등 점검한 모든 macOS vibrancy crate가 동일하게 `addSubview_positioned_relativeTo(..., Below, None)` 패턴 사용. wgpu가 host view layer를 점유한 상태에서는 모두 실패 2와 같은 결과.

### 2.5 옵션 E 기각 사유

macOS App Sandbox + Screen Recording 권한 모델은 타 앱(또는 데스크톱)의 픽셀을 wgpu shader에서 read-back하는 path를 막는다. ScreenCaptureKit 같은 별도 capture 채널을 frame마다 호출하면 권한 prompt + 성능 비용 + entitlement 필요. system vibrancy(`NSVisualEffectMaterial::HudWindow` 등)의 dynamic tint + saturation boost + blur를 재현 불가. 차후 wgpu-only 효과(예: 자체 콘텐츠 blur, drop shadow)에는 유효하지만 본 목표(데스크톱 vibrancy)와 어긋남.

## 3. 권장 path — 옵션 F (Hybrid WgpuOverlay)

### 3.1 view tree 구조

```
NSWindow                                          isOpaque=NO, backgroundColor=clearColor
                                                  (winit `with_transparent(true)`로 이미 설정됨)
└─ contentView = winit_view (NSView, layer-backed) backing layer = vanilla CALayer (transparent)
                                                   ↳ 우리는 이 layer 안 건드림
   │
   ├─ subview[0] BehindVibrancy: NSVisualEffectView
   │     material = HudWindow (step 4에서 테마별 분기)
   │     state = Active
   │     blendingMode = BehindWindow
   │     autoresizingMask = width|height
   │     frame = winit_view.bounds
   │
   └─ subview[1] WgpuOverlay: 신규 plain NSView ("Pj001WgpuOverlay")
         wantsLayer = YES
         layer = 우리가 직접 만든 CAMetalLayer
         autoresizingMask = width|height
         frame = winit_view.bounds
         hitTest: = nil 반환 (이벤트 pass-through)
         acceptsFirstResponder = NO
```

bottom-to-top 합성 stack:
```
1. NSWindow background (clearColor 투명)
2. winit_view.layer            (vanilla CALayer, 빈 transparent)
3. NSVE.layer                  (vibrancy material — window server BehindWindow 합성)
4. WgpuOverlay.layer (CAMetalLayer)  ← 우리 cell/text. cell.bg.alpha < 1 영역에서 NSVE 비침
```

### 3.2 실패 1/2와 본질적 차이

- **실패 1과 차이**: `setContentView` 호출 없음. winit_view는 NSWindow.contentView 그대로 유지. winit의 RefCell 재진입 path 미발생.
- **실패 2와 차이**: wgpu가 winit_view.layer를 더 이상 점유하지 않음. winit_view.layer는 winit이 만든 빈 transparent CALayer 그대로. NSVE가 sub-view라 "host layer 위"에 합성되지만 host layer가 비어있어 NSVE가 보임. WgpuOverlay는 NSVE의 sibling subview이고 subview 순서로 NSVE 위에 합성됨. NSVE가 cell 콘텐츠를 가리지 않음.

핵심: NSVE와 WgpuOverlay가 **sibling**이라는 것. "subview는 host layer 위에 합성된다" 룰은 둘 다 동일하게 winit_view 위로 올리지만, 둘 사이에는 subview 인덱스 순서가 적용된다.

### 3.3 wgpu surface 생성 변경

현재 (`crates/pj001-core/src/app/mod.rs:3017-3018`):
```rust
let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
let surface = instance.create_surface(window.clone())?;
```

변경 후 개념:
1. winit_view 핸들 획득 (raw_window_handle로 NSView pointer).
2. WgpuOverlay NSView + CAMetalLayer 생성 (objc2-quartz-core 추가 의존성 필요할 수 있음 — 또는 objc2 msg_send로 CAMetalLayer 직접 alloc/init).
3. CAMetalLayer 속성 설정: `pixelFormat = MTLPixelFormatBGRA8Unorm_sRGB`, `framebufferOnly = YES`, `contentsScale = window.scale_factor()`, **`isOpaque = NO`** (default YES — 미설정 시 vibrancy가 sibling NSVE를 가려 silent하게 실패. M-P3-2a에선 시각 변화 없어 발견 못 하고 M-P3-2b에서 "NSVE attach 깨짐"으로 오진할 위험), `presentsWithTransaction = NO` (default, CATransaction 결합 시 `surface.present()`와 충돌).
4. WgpuOverlay.wantsLayer = YES; WgpuOverlay.layer = CAMetalLayer.
5. winit_view.addSubview(WgpuOverlay) — NSVE보다 나중에 추가하거나 `positioned:Above relativeTo:NSVE`로 명시.
6. `instance.create_surface_unsafe(SurfaceTargetUnsafe::CoreAnimationLayer(layer_ptr))` 호출.

attach 순서:
- `App::window_event` Resumed 단계 (현재 `mod.rs:1208-1217` 영역)에서 NSVE attach.
- `AppState::new_with_size` (현재 `mod.rs:3017` 영역)에서 wgpu surface 생성 전에 WgpuOverlay attach + CAMetalLayer 생성 후 그 raw pointer로 surface 생성.

### 3.4 raw_window_handle 호환

wgpu가 `Instance::create_surface(window)`로 받던 RawWindowHandle::AppKit path와 무관해진다. raw_window_handle 0.6은 그대로 NSView 핸들 얻는 용도(NSVE attach + IME wake-up)로 유지.

### 3.5 의존성 변화

신규 필요(예상):
- `objc2-quartz-core` (CAMetalLayer 타입) 또는 msg_send 기반 raw alloc — 후자가 의존성 부담 적음.
- objc2-app-kit feature 추가: `NSVisualEffectView` (현재 `NSView`, `NSResponder`, `NSTextInputContext`만 enabled — `Cargo.toml:30`).

기존 유지:
- `window-vibrancy` 0.7: 시각 검증 단계까지는 옵션 F가 검증되기 전 fallback으로 보존, 검증 후 제거 가능 (NSVE attach를 직접 작성하면 더 이상 불필요).

## 4. 단계 분해 (M-P3-2 sub-milestones)

### M-P3-2a — WgpuOverlay sibling only, NSVE 미부착

목표: wgpu가 winit_view 외부 layer로 cell을 그리되 NSVE 없음. 시각적으로는 step 1과 동일(데스크톱 직접 비침).

작업:
- WgpuOverlay NSView + CAMetalLayer 신규 모듈 (`crates/pj001-core/src/app/macos_overlay.rs` 가칭).
- `AppState::new_with_size`에서 `create_surface(window.clone())` → `create_surface_unsafe(CoreAnimationLayer)` 교체.
- winit_view에 WgpuOverlay sibling 추가, hitTest=nil 구현.
- contentsScale 동기화 (초기 + `ScaleFactorChanged`).
- Resize: WgpuOverlay autoresizingMask + `surface.configure(new_size)` 동작 확인.

검증:
- cargo test 280개 회귀 0.
- 시각: 윈도우 transparent로 데스크톱 비치고 cell text 정상.
- IME: 한글 입력 정상(`macos_ime.rs` wake-up path 유지). first-key escape 회귀 없음.
- 마우스: click/drag로 winit_view에 이벤트 도달 (hitTest=nil → AppKit이 sub-view 무시).

위험:
- contentsScale 누락 시 retina에서 콘텐츠 흐릿/2배 크기.
- WgpuOverlay frame이 winit_view bounds를 못 따라가면 resize 시 cell 영역 깨짐.
- winit_view에 wgpu layer 없으니, AppKit이 winit_view 자체에 draw cycle을 trigger하지 않을 수 있음 — winit redraw scheduler가 RedrawRequested를 정상 발화하는지 확인. 발화 안 되면 `CAMetalLayer.setNeedsDisplay`를 명시 호출하거나 별도 displaylink 도입.

### M-P3-2b — NSVE sibling 추가

목표: M-P3-2a 위에 NSVE를 sibling으로 추가, vibrancy가 cell.bg.alpha<1 영역에서 비침.

작업:
- `macos_backdrop.rs`를 직접 NSVE alloc/init/setMaterial 코드로 재작성 (window-vibrancy 의존성 제거 가능).
- `winit_view.addSubview_positioned_relativeTo(nsve, Below, None)` 호출 — NSVE를 subview[0]에 배치.
- WgpuOverlay는 이미 subview[1]로 그 위에 있음 (M-P3-2a에서 추가됨).
- attach 순서 점검: NSVE를 WgpuOverlay 추가 **전**에 attach, 또는 추가 후 `positioned:Below relativeTo:WgpuOverlay`로 명시.
- NSVE autoresizingMask: width|height.

검증:
- cell.bg.alpha = 1.0 (현재 default)이면 시각 변화 0.
- 임시로 테스트용 cell.bg.alpha = 0.6 한 케이스 만들어 vibrancy material이 비치는지 시각 검증.
- IME/mouse 회귀 0.

위험:
- subview 순서가 의도와 다를 경우 NSVE가 위로 올라와 다시 cell 가림. attach 직후 `winit_view.subviews` 배열 로깅 + 순서 assert.
- NSVE BehindWindow는 `NSWindow.isOpaque == NO` 전제. winit `with_transparent(true)` 이미 설정 (`window_delegate.rs:659-663`). 재확인.

### M-P3-2c — 입력 + hitTest 시스템 검증

목표: WgpuOverlay가 모든 입력 경로를 winit_view로 통과시킴.

작업:
- 시스템 매트릭스 수동 검증:
  - 키 입력 / 한글 IME (조합 + commit, 입력 소스 전환 직후 first-key escape 미발생)
  - 마우스: left/right click, drag, scroll, hover cursor 변경
  - 윈도우: titlebar drag, resize handle drag, traffic light buttons
  - 포커스: 다른 앱으로 switch 후 복귀 시 IME 재활성
- cargo test 전체.
- `winit_view.layer.class` 로그로 vanilla CALayer 확인 (CAMetalLayer가 아니어야 함). 만약 CAMetalLayer라면 wgpu가 어떤 path로 layer를 점유한 것 — 설계 가정 깨짐.

검증 acceptance:
- 280 cargo test pass.
- 4가지 입력 카테고리 회귀 0.
- 시각: cell.bg.alpha 변화에 따라 vibrancy 비치는 정도 가시적으로 변함.

### M-P3-3 — 테마별 NSVisualEffectMaterial 분기 (기존 계획 유지)

본 재설계와 무관, M-P3-2 완료 후 기존 step 3/4 plan 유지.

## 5. 위험 항목 종합

| 위험 | 영향 | 완화 |
|---|---|---|
| `RedrawRequested` 발화 변동 | 화면 freezing | M-P3-2a 검증에서 frame 카운트 로깅. 미발화 시 manual `setNeedsDisplay` 또는 CADisplayLink |
| contentsScale 미동기화 | retina 흐림/잘림 | 초기 + ScaleFactorChanged에서 명시 설정 |
| hitTest pass-through 누락 | 마우스 dead zone | hitTest nil 구현 + 모든 마우스 이벤트 수동 검증 |
| NSVE의 hitTest가 self 반환 | 마우스가 NSVE에서 멈춤 | window-vibrancy 사용처에서 실증되지 않으나, NSVE userInteractionEnabled=NO 또는 hitTest override 검토 |
| objc2 raw msg_send 안전성 | unsafe block 증가, lifetime 실수 | layer는 NSView가 retain. raw pointer는 surface 생성에만 사용 후 wgpu가 자체 retain (`wgpu_core.rs:820-822` 동작 확인 필요) |
| winit_view.layer를 wgpu가 여전히 점유 | 설계 전제 깨짐, 실패 2 재발 | M-P3-2a `create_surface_unsafe` 호출 **직후** `winit_view.layer.class` 로깅 (hidden fallback path가 window handle layer를 잡았는지 즉시 발견) |
| WgpuOverlay CAMetalLayer.isOpaque 기본값 YES | M-P3-2b에서 vibrancy가 보이지 않음. M-P3-2a 시각엔 무영향이라 발견 지연 | layer 생성 시점에 명시적 `isOpaque = NO`. 로그로 값 확인 |
| window-vibrancy 제거 후 시스템 호환 | NSVE attach 자체 실수 | M-P3-2b 단계에서 window-vibrancy를 fallback으로 보존, 비교 동작 확인 후 제거 |

## 6. 검증 acceptance 체크리스트

M-P3-2 전체 완료 조건:

- [ ] cargo test 전체 (280+) pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] 한글 IME 시나리오: 영→한 전환 직후 첫 자모 정상 (first-key escape 회귀 0)
- [ ] 마우스: left/right click, drag, scroll, cursor change 정상
- [ ] 윈도우: titlebar drag, resize, traffic light 정상
- [ ] Retina(2x) HiDPI에서 cell 렌더 sharp
- [ ] Resize 동안 cell 영역이 NSVE 영역과 일치 (winit_view bounds 추종)
- [ ] cell.bg.alpha 변화에 따라 vibrancy 가시성 변화 (e.g. 1.0 → 불투명, 0.5 → 반투명에 vibrancy 비침, 0.0 → 완전히 vibrancy 노출)
- [ ] `PJ001_BACKDROP=0` 또는 미설정 시 attach skip path 유지 (회귀 대비 escape hatch)
- [ ] `winit_view.layer.class == CALayer` (CAMetalLayer 아님) 로그 확인 — `create_surface_unsafe` 호출 **직후** 시점 (hidden fallback path 검출용)
- [ ] WgpuOverlay CAMetalLayer `isOpaque == NO` 로그 확인 (vibrancy 가시성 전제)
- [ ] WgpuOverlay CAMetalLayer `presentsWithTransaction == NO` (wgpu surface.present 호환)

## 7. 미해결 — 코드 작성 단계에서 결정할 항목

- WgpuOverlay 구현 path: `objc2 declare_class!` macro로 명시 subclass 만들 것인지 vs. plain NSView + hitTest는 별도 helper로 구현할 것인지. hitTest override는 subclass가 필요할 수 있음 (NSView의 `hitTest:` 메서드 override).
- CAMetalLayer 생성 path: `objc2-quartz-core` 의존성 추가 vs. msg_send 기반 raw alloc. 후자가 dependency 부담 적으나 unsafe 면적 증가.
- NSVE/WgpuOverlay attach 호출 순서를 `App::resumed`(window 생성 직후) vs. `AppState::new_with_size`(surface 생성 직전)로 둘지. surface 생성과 layer가 결합되므로 `new_with_size`가 자연.
- window-vibrancy 의존성 제거 시점 (M-P3-2b 완료 후 제거 권장).

이 항목들은 구현 단계에서 결정. 본 문서는 설계만 다룬다.

## 8. 출처

- winit 0.30.13 `view.rs:863, 901` — NSView ivars `RefCell<CursorState>`, `inputContext().expect()`
  로컬: `/Users/derek/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/winit-0.30.13/src/platform_impl/macos/view.rs`
- winit 0.30.13 `window_delegate.rs:652, 659-663` — `view.setWantsLayer(true)`, transparent window `setOpaque(false)` + `setBackgroundColor(clearColor())`
- window-vibrancy 0.7.1 `src/macos/vibrancy.rs:59` — `addSubview_positioned_relativeTo(..., NSWindowOrderingMode::Below, None)`
- wgpu 29.0.3 `src/api/surface.rs:386-392` — `SurfaceTargetUnsafe::CoreAnimationLayer(*mut c_void)` 정의
- wgpu 29.0.3 `src/backend/wgpu_core.rs:820-822` — `instance_create_surface_metal(layer, None)` dispatch
- Apple AppKit: layer-backed view + subview compositing 문서 (NSView class reference, "Drawing into views" 섹션)
- 본 프로젝트 `crates/pj001-core/src/app/macos_ime.rs` — NSTextInputContext.activate 워크어라운드 (Codex thread `019e2491`)
