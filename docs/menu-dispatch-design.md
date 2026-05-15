# Menu click → AppCommand dispatch 설계

작성일: 2026-05-15
관련 코드: `crates/pj001-core/src/app/macos_menu.rs`, `app/event.rs`, `app/mod.rs::handle_user_event`

## 1. 문제

NSMenu 1~3라운드까지는 custom 항목이 `keyEquivalent`만 표시 + winit keyboard
chain이 실 동작. menu item을 마우스로 클릭해도 no-op (Codex 4차에서 `setAutoenablesItems(false)`로
disabled 시각만 fix). UX 개선 — menu click도 실 동작.

## 2. 선택지

| 옵션 | 패턴 | 장점 | 단점 |
|---|---|---|---|
| A — selector 13개 | 각 명령마다 selector 분리 | 명확 | 코드 폭발 (define_class!에 13 method) |
| B — single selector + tag | menuAction: 하나 + NSMenuItem.tag 매핑 | 단순, 확장 쉬움 | tag enum 매핑 명시 필요 |
| C — IPC 채널 | 별도 thread, channel | thread-safe | overengineering |

**채택: B**. 단일 `menuAction:` selector + NSInteger tag로 명령 식별.

## 3. 데이터 흐름

```
NSMenuItem (tag=N, action=menuAction:, target=MenuTarget)
  └ user click
      └ AppKit dispatch menuAction:
          └ MenuTarget::menu_action 안에서 tag 읽음
              └ MENU_PROXY.send_event(UserEvent::MenuCommand(AppMenuCommand::X))
                  └ winit main loop wake
                      └ handle_user_event(UserEvent::MenuCommand)
                          └ AppState::dispatch_menu_command(cmd)
                              └ 기존 CmdShortcut handler 재활용
```

## 4. enum

```rust
// app/event.rs
#[derive(Debug, Clone, Copy)]
pub enum AppMenuCommand {
    // Shell
    NewTab = 1,
    CloseActive = 2,
    SplitVertical = 3,
    SplitHorizontal = 4,
    CloseTab = 5,
    // Edit
    Copy = 10,
    Paste = 11,
    SelectAll = 12,
    Find = 13,
    ClearBuffer = 14,
    ClearScrollback = 15,
    // View
    ZoomIn = 20,
    ZoomOut = 21,
    ZoomReset = 22,
}

pub enum UserEvent {
    SessionRepaint(SessionId),
    SessionExited { ... },
    SessionPtyError { ... },
    MenuCommand(AppMenuCommand),
}
```

## 5. MENU_PROXY 보관

`pub static MENU_PROXY: OnceLock<EventLoopProxy<UserEvent>>` in `macos_menu`.
`AppState::new_with_size`에서 `MENU_PROXY.set(proxy.clone())`. menu_target init은
attach_menu_bar 안에서 한 번. send_event는 thread-safe (EventLoopProxy is Send + Sync).

## 6. handle_user_event dispatch

```rust
UserEvent::MenuCommand(cmd) => state.dispatch_menu_command(cmd),
```

`dispatch_menu_command` 메서드는 기존 cmd_shortcut handler를 직접 호출:
- NewTab → handle_new_tab
- SplitV/H → handle_split
- Copy → handle_copy (clipboard)
- Paste → handle_paste
- ZoomIn/Out/Reset → set_logical_font_size
- ... 등

## 7. 구현 단계

### Step 1 — event.rs
- AppMenuCommand enum 추가 (13 variant + repr i64)
- UserEvent::MenuCommand variant 추가

### Step 2 — macos_menu.rs
- MENU_PROXY OnceLock 신설
- pub fn init_menu_proxy(proxy)
- MenuTarget에 menu_action: selector 추가
- NSMenuItem 생성 helper에 tag 인자 추가

### Step 3 — mod.rs
- AppState::new_with_size에서 MENU_PROXY 초기화
- handle_user_event에 MenuCommand 분기
- dispatch_menu_command 메서드 — 기존 handler 재활용

### Step 4 — Codex 리뷰

### Step 5 — test
cargo test 회귀 0 + smoke

## 8. 위험

- `MENU_PROXY.set` race: AppState::new_with_size가 finish_startup 한 번만 호출 → race 없음.
- AppMenuCommand tag 값 충돌: 1부터 시작, separator 등은 0 (default tag).
- NSMenuItem.tag가 NSInteger (i64 on 64-bit). 우리 enum repr i64.
- 기존 keyEquivalent + winit keyboard chain은 그대로 — menu click과 단축키 둘 다 동일 명령 dispatch.

## 9. 알려진 한계

- Find: dispatch만으로는 부족 (Cmd+F는 input 모드 진입 + UI overlay). 첫 cut에선 단순 trigger.
- New Window: multi-window milestone 전. dispatch는 등록하되 handler가 placeholder (log warn).
