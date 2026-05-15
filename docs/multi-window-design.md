# Multi-window milestone 설계

작성일: 2026-05-15
상태: **design only** — 본 구현은 별도 milestone.

## 1. 현재 상태

pj001은 단일 NSWindow + 단일 wgpu surface + 단일 AppState. Cmd+N(현재) =
`AppMenuCommand::NewPane` = `split_active(Vertical)` 즉 새 pane이지 새 윈도우 아님.

macOS Terminal.app/iTerm2 표준:
- Cmd+N = New Window (별도 NSWindow)
- Cmd+T = New Tab (동일 윈도우 안)
- Cmd+D = Split (동일 tab 안)

우리는 Cmd+T/D 표준, Cmd+N은 split fallback (multi-window 없음 핑계).

## 2. 목표

- 별도 NSWindow 생성 (winit `ActiveEventLoop::create_window`).
- 각 윈도우가 독립 wgpu surface + AppState? 또는 단일 AppState + WindowId → state map?
- Cmd+N이 새 윈도우 생성 + shell spawn.
- 각 윈도우의 focus / resize / redraw / event 분리.
- 단일 NSMenu (Window menu에 윈도우 목록 자동).
- 종료 시 모든 윈도우의 PTY cleanup.

## 3. 아키텍처 옵션

| 옵션 | 설명 | 복잡도 |
|---|---|---|
| A — 단일 AppState + windows map | AppState가 `HashMap<WindowId, WindowState>` 보관. 각 WindowState = wgpu surface + Renderer + tabs + sessions | 中 |
| B — Multiple App instances | 각 윈도우가 자기 ApplicationHandler. winit `run_app`이 단일 App 받으니 어렵 | 高 |
| C — 단일 wgpu Instance + multi Surface | wgpu Instance 공유, surface/Renderer는 윈도우별 | 中(권장) |

**권장: A + C** — App struct에 `windows: HashMap<WindowId, WindowState>`. WindowState
= window + surface + surface_config + renderer + tabs + sessions + overlay_attach.
공유: wgpu Instance, device, queue, EventLoopProxy, Config, hooks.

## 4. 변경 영향

### App struct
```rust
struct App {
    proxy: EventLoopProxy<UserEvent>,
    config: Config,
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    windows: HashMap<WindowId, WindowState>,
    active_window: Option<WindowId>,
    // 기존 pending_window/startup_waited_once는 첫 윈도우용
}
```

### WindowState
기존 AppState의 window-specific 필드 모두:
- window: Arc<Window>
- surface: Surface<'static>
- surface_config: SurfaceConfiguration
- renderer: Renderer
- tabs / sessions
- focused / modifiers / selection / pending_find / overlay_attach / pending_resize / ...

### 이벤트 라우팅
- `window_event(event_loop, window_id, event)` → `windows.get_mut(&window_id)`로 해당 WindowState로 dispatch.
- `user_event` (UserEvent::SessionRepaint 등): session_id 보유한 윈도우 찾아 dispatch.
- `about_to_wait`: 모든 윈도우의 pending_resize / bell / find / cursor blink 처리.

### Cmd+N dispatch
`dispatch_menu_command(M::NewPane)` 대신 `M::NewWindow` 추가 + `App::create_new_window`:
1. winit `event_loop.create_window(attrs)` 호출.
2. WindowState 생성 (wgpu surface + Renderer + 새 첫 session = shell spawn).
3. `windows.insert(window.id(), state)`.
4. `active_window = Some(new_id)`.

### NSMenu Window 메뉴
- Apple 자동 — `windowsMenu` setting하면 NSWindow마다 menu item 자동 추가.
- 우리는 이미 `app.setWindowsMenu(Some(&window_menu))` 호출 중 — multi-window 도입 후
  자동 채워짐.

### 종료
- `event_loop.exit()`은 모든 윈도우 종료. 한 윈도우만 닫기는 `windows.remove(&id)`.
- 마지막 윈도우 닫으면 `event_loop.exit()` 호출.

## 5. 변경 규모 추정

- `App`/`AppState` 큰 refactor — 단일 AppState를 `WindowState`로 rename + Map 안에 보관.
- 모든 `state.xxx` 접근을 `state` (active) 또는 명시적 WindowId 기반으로 라우팅.
- 회귀 위험: 기존 단일 윈도우 동작이 깨질 가능성. 단계 분해 필수.

대략 1주 작업. 단계:

1. AppState → WindowState rename (구조 변경 없이 단순 rename, 회귀 0).
2. App struct에 `windows: HashMap<WindowId, WindowState>` (현재 1개만).
3. 모든 이벤트 핸들러를 windows.get_mut(&id) 경유로.
4. Cmd+N → `App::create_new_window` (실 동작).
5. UserEvent에 WindowId 부착 또는 SessionId → WindowId reverse lookup.
6. Bell/pending_resize/blink 등 모든 window 순회.
7. Quit/Close 정책.

## 6. 비목표 (현 milestone 안)

- Drag/drop window across screens
- Window splitting across windows (panel을 다른 윈도우로 옮기기)
- Window-level theme override (윈도우별 다른 테마)
- Tabbed windows (macOS NSWindow.tabbingMode) — Apple 자동 처리

## 7. 작업 분해 (skeleton — 본 구현 별도 세션)

### M-W-1 (rename only)
- AppState → WindowState. 회귀 0.
- App.state: Option<AppState> → windows: HashMap<WindowId, WindowState>.
- 모든 self.state.as_mut() 접근을 single map entry로.

### M-W-2 (event routing)
- window_event/user_event/about_to_wait에 windows map 순회 + WindowId 기반 dispatch.
- 회귀 0 (단일 윈도우 동작 유지).

### M-W-3 (Cmd+N 실 동작)
- App::create_new_window 메서드. winit create_window + WindowState 생성.
- AppMenuCommand::NewWindow variant 추가 + dispatch routing.
- NSMenu Shell menu의 "New Pane" 외 "New Window" 별도 항목.

### M-W-4 (시스템 검증)
- 두 윈도우 동시 운영. 각자 shell, focus, mouse selection, IME, bell 분리.
- 한 윈도우 close → 다른 윈도우 유지. 마지막 close → exit.
- macOS Window 메뉴에 두 윈도우 목록 자동 표시.

## 8. 알려진 함정

- **wgpu Instance 공유**: 단일 Instance에서 multi-surface 가능 (검증된 패턴).
- **MainThreadMarker**: winit + AppKit 모두 main thread 전용. App struct가 main thread 전용이라 OK.
- **NSMenu MENU_PROXY**: 이미 EventLoopProxy 공유 — multi-window 도입에도 호환.
- **macos_overlay / macos_backdrop**: 각 윈도우마다 별도 attach 필요. WindowState에 각자 overlay_attach 보관.
- **wgpu Device/Queue 공유**: 가능. 모든 윈도우가 동일 device + queue 사용.

## 9. 임시 대응 (multi-window 도입 전)

- Cmd+N은 NewPane(Vertical split)로 매핑 유지. macOS 사용자가 헷갈리지 않게.
- NSMenu Shell에 "New Pane" (현재) — multi-window 도입 시 "New Window" 추가.
- 사용자 문서: README에 "Cmd+N = new pane (split), multi-window는 별도 milestone" 안내.

이 design doc은 다음 세션 개발자(또는 본인)가 즉시 진입할 수 있게 charter.
