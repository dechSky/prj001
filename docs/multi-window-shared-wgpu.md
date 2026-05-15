# Multi-window wgpu 공유 (M-W-6) — Cmd+N freeze 회피

작성일: 2026-05-15
상태: design + 본 구현
선행: M-W-5 timing (`a588405`)

## 1. 문제

`WindowState::new_with_size` (현재 line ~3388)는 매 호출마다:
1. `wgpu::Instance::new(...)` — 동기, ms 단위
2. `instance.request_adapter(&opts).await` — **async**, GPU 탐색
3. `adapter.request_device(&desc).await` — **async**, device 생성
4. surface init / configure / Renderer::new (cell metrics, font atlas 등)

`App::create_new_window`은 이것을 `pollster::block_on(...)`로 호출 → 메인 스레드(winit + AppKit
이벤트 루프) **블록**. Cmd+N 누르는 순간 UI 멎고 수백 ms (대략 200~800ms macOS Metal) freeze.

첫 윈도우 시점엔 OK — startup 중이라 사용자 체감 영향 작음. 그러나 **두 번째 윈도우부터**는
실 사용 중 발생 → "터미널이 잠깐 죽었다"는 인상.

## 2. 목표

두 번째 윈도우 이후 `create_new_window`이 **동기적으로 즉시 반환**. async work 없음.

## 3. 분석

| 단계 | 첫 윈도우 | 이후 윈도우 |
|---|---|---|
| wgpu::Instance::new | 동기, 1회 비용 | **재사용** |
| request_adapter | **async**, GPU 탐색 | **재사용** (같은 GPU) |
| request_device | **async**, device 생성 | **재사용** (한 device로 multi-surface) |
| Instance::create_surface_unsafe | 동기 | 동기 (윈도우별 surface는 unique) |
| Surface::configure | 동기 | 동기 |
| Renderer::new | 동기 (font atlas / pipeline) | 동기 (재사용 device 위) |
| Layout / sessions / PTY spawn | 동기 | 동기 |

→ **wgpu Instance/Adapter/Device/Queue를 공유하면 두 번째 이후 async 0건** → block_on 불필요.

## 4. wgpu multi-surface 안전성

wgpu Examples + 공식 패턴 — 단일 Instance/Device가 multi-Surface 지원. 한 device로 여러 윈도우의
surface를 동시에 render 가능. macOS Metal에서도 검증된 패턴.

함정:
- adapter 선택 시 `compatible_surface: Some(&first_surface)`로 결정 → 다른 윈도우의 surface는
  같은 GPU (macOS는 통상 단일 GPU)라 호환. 별도 검증 필요 없음.
- `surface.get_capabilities(&adapter)`의 결과 (formats, alpha_modes, present_modes)는 윈도우별
  미세 차이 가능성. 다만 같은 GPU + 같은 macOS Metal layer면 동일.
  → 첫 surface가 선택한 `format` + `alpha_mode`를 그대로 재사용 (보수적). 두 번째 surface도
  `surface.get_capabilities()` 호출해서 그 format이 지원되는지 assert 정도.

## 5. 설계

### 5.1 `WgpuShared` struct

```rust
struct WgpuShared {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Codex M-W-6 1차 critical: "선호값". 두 번째 surface caps에 있으면 사용,
    /// 없으면 caps에서 재선택 (Surface::configure unsupported format이면 panic).
    preferred_surface_format: wgpu::TextureFormat,
    preferred_alpha_mode: wgpu::CompositeAlphaMode,
}
```

### 5.2 `App` 변경

```rust
struct App {
    windows: HashMap<WindowId, WindowState>,
    active_window: Option<WindowId>,
    pending_window: Option<Arc<Window>>,
    startup_waited_once: bool,
    proxy: EventLoopProxy<UserEvent>,
    config: Config,
    /// M-W-6: 첫 윈도우 init 시 채워짐. 이후 윈도우는 reuse.
    wgpu_shared: Option<WgpuShared>,
}
```

### 5.3 `WindowState::new_with_size` 시그니처 분기

옵션 A: 단일 시그니처 + Option<&WgpuShared>
```rust
async fn new_with_size(
    window, proxy, config, size,
    shared: Option<&WgpuShared>,
) -> Result<(Self, Option<WgpuShared>)>
```
shared=None이면 새로 init + 새 WgpuShared 반환. Some이면 reuse + None 반환.

옵션 B (권장): 두 path 명시적 분리
```rust
// 첫 윈도우 — async (request_adapter/request_device 호출)
async fn new_first(
    window, proxy, config, size,
) -> Result<(Self, WgpuShared)>

// 이후 윈도우 — 동기, await 없음
fn new_with_shared(
    shared: &WgpuShared,
    window, proxy, config, size,
) -> Result<Self>
```

**옵션 B 채택 이유**: freeze 위험이 어느 path에 있는지 시그니처에서 즉시 보임.
`new_with_shared`는 동기 함수라 `pollster::block_on` 호출 자체가 컴파일러에서 강제 차단.

### 5.4 `App::create_new_window` 변경

Codex M-W-6 1차 개선: `wgpu_shared` guard를 window 생성 전으로 이동 (invariant를
코드로 고정).

```rust
fn create_new_window(&mut self, event_loop: &ActiveEventLoop) {
    let Some(shared) = self.wgpu_shared.as_ref() else {
        log::warn!("create_new_window: wgpu_shared not initialized — Cmd+N ignored");
        return;
    };
    let attrs = ...;
    let window = Arc::new(event_loop.create_window(attrs)?);
    let size = window.inner_size();
    // 동기 호출 — freeze 없음.
    let state = match WindowState::new_with_shared(shared, window.clone(), ...) {
        Ok(s) => s,
        Err(e) => { log::warn!(...); return; }
    };
    self.windows.insert(window.id(), state);
    self.active_window = Some(window.id());
}
```

### 5.5 첫 윈도우 init path

`App::resumed` + `finish_startup`에서 첫 윈도우 init:
- 기존: `pollster::block_on(WindowState::new_with_size(window, proxy, config, size))`
- 변경: `let (state, shared) = pollster::block_on(WindowState::new_first(window, proxy, config, size))?; self.wgpu_shared = Some(shared); self.windows.insert(id, state);`

여전히 block_on 한 번 — 첫 윈도우는 어차피 startup이라 OK.

## 6. 회귀 위험

- **첫 윈도우 동작 변경 0** — `new_first`은 기존 `new_with_size`을 그대로 옮긴 것. 추가로
  WgpuShared 반환만.
- **두 번째 윈도우의 format/alpha_mode 결정**: 첫 surface 기준을 재사용 — 다른 윈도우의
  capabilities와 mismatch 가능성 미미하지만 가능성 0 아님. assert + fallback log 추가.
- **Renderer::new가 device를 take/borrow 형태**: 현재 `&device, &queue` 받음 — borrow. WindowState에는
  자체 device/queue 보관하지 않고 매번 shared를 통해 접근하도록 변경?

기존 `WindowState`는 `device: wgpu::Device`와 `queue: wgpu::Queue`를 **field로 보관**. wgpu의
Device/Queue는 `Clone` 가능 (내부적으로 Arc) → 첫 윈도우 init에서 받아 clone 해서 보관.
이후 윈도우도 shared.device.clone() / shared.queue.clone()로 받음 → 기존 코드 경로 0 변경.

## 7. 변경 영향

- `crates/pj001-core/src/app/mod.rs`
  - `WgpuShared` struct 추가
  - `App.wgpu_shared: Option<WgpuShared>` field
  - `WindowState::new_with_size` → 내부 helper로 강등 + `new_first` / `new_with_shared` 두 entry
  - `App::finish_startup` 첫 윈도우 path가 `new_first` 호출 + wgpu_shared 보관
  - `App::create_new_window` 동기 호출로 변경
- `cargo test` 회귀 0 기대 — single window 동작 변경 없음
- 시각 검증: 두 번째 윈도우 생성 시 UI freeze 없음 (manual QA)

## 8. 비목표

- per-window 다른 GPU 선택 (multi-GPU 시스템)
- format/alpha_mode가 윈도우별로 다른 케이스 처리 — assert + 경고 log만
- async path 전체 제거 (첫 윈도우는 여전히 block_on 필요)

## 9. Codex 리뷰 포인트

- wgpu Device clone 공유 안전성 (Arc 내부, Send/Sync 보장)
- 첫 surface의 capabilities를 두 번째 윈도우에 강제 적용해도 안전?
- multi-Surface가 macOS Metal에서 알려진 함정?
- `new_first` 호출 후 self.wgpu_shared write — 두 번째 호출 시점에 이미 있어야 한다는
  invariant 검증 (Codex 6차 ID overflow와 비슷한 패턴).
