# macOS menu bar 설계 — pj001 NSMenu

작성일: 2026-05-15
관련 코드: `crates/pj001-core/src/app/macos_menu.rs` (신규 예정),
`crates/pj001-core/src/app/mod.rs::finish_startup` (attach 위치)

## 1. 문제 정의

현재 pj001은 macOS bundle은 있지만(M6-2 `scripts/bundle.sh`) NSMenu 미부착 →
상단 menu bar가 비어있고 기본 단축키(Cmd+Q 등)가 menu 시각 hint 없이 동작.
다른 macOS 표준 터미널(Terminal.app/iTerm2/Alacritty/Ghostty/WezTerm)은 모두
NSMenu 부착으로 사용자에게 단축키 + 명령 시각 노출.

목표: pj001에 표준 macOS menu bar 부착 — 6 menu (App / Shell / Edit / View /
Window / Help) + 표준 단축키 + 기존 기능과 정합.

## 2. macOS 표준 터미널 menu 구조 비교

### Terminal.app (Apple) — minimal reference

| Menu | Items |
|---|---|
| Terminal | About / Preferences / Services / Hide / Hide Others / Show All / Quit |
| Shell | New Window / New Tab / New Command / New Remote Connection / Open / Save / Save as / Save Selected Text / Export / Print / Print Selection / Close Window / Close Tab |
| Edit | Undo / Redo / Cut / Copy / Paste / Paste Escaped / Paste Selection / Clear Scrollback / Find / Save Search String / Use Selection for Find / Jump to Selection / Spelling / Substitutions / Speech / Start Dictation / Emoji & Symbols |
| View | Show Tab Bar / Show All Tabs / Show Customize Toolbar / Bigger / Smaller / Zoom In / Zoom Out / Make Text Larger / Smaller / Show Inspector / Use Background as System Background / Toggle Full Screen |
| Window | Minimize / Zoom / Bring All to Front / Window 목록 |
| Help | Search / Terminal Help |

### iTerm2 — 더 풍부 (Profiles, Triggers, Scripts 등)

우리 1차 cut에선 Terminal.app 수준만.

## 3. pj001 menu 매핑 (1차 cut)

기존 기능(`docs/keyboard-design.md` + `roadmap.md` + 본 세션 추가)과 매핑.

### App menu (pj001)

| Item | 단축키 | 구현 |
|---|---|---|
| About pj001 | — | Apple 표준 selector `orderFrontStandardAboutPanel:` 매핑 |
| Preferences… | Cmd+, | placeholder (현재 GUI 없음, `~/.config/pj001/config.toml` 안내) |
| Services | — | NSMenu submenu — system services 자동 |
| Hide pj001 | Cmd+H | Apple 표준 `hide:` |
| Hide Others | Opt+Cmd+H | `hideOtherApplications:` |
| Show All | — | `unhideAllApplications:` |
| Quit pj001 | Cmd+Q | Apple 표준 `terminate:` (또는 우리 Cmd+Q 핸들러) |

### Shell menu

| Item | 단축키 | 구현 |
|---|---|---|
| New Window | Cmd+N | placeholder (multi-window 별도 milestone) |
| New Tab | Cmd+T | 기존 Cmd+T (M14) |
| Split Vertically | Cmd+D | 기존 (M13) |
| Split Horizontally | Cmd+Shift+D | 기존 (M13) |
| Close | Cmd+W | 기존 (Cmd+W escalation) |
| Close Tab | Cmd+Shift+W | 기존 |

### Edit menu

| Item | 단축키 | 구현 |
|---|---|---|
| Copy | Cmd+C | 기존 |
| Paste | Cmd+V | 기존 (M10-6) |
| Select All | Cmd+A | 기존 (c9fc1f4) |
| Find… | Cmd+F | 기존 (슬라이스 6.5) |
| Find Next | Enter | 기존 (Cmd+F 내) |
| Find Previous | Shift+Enter | 기존 |
| Clear Buffer | Cmd+K | 기존 |
| Clear Scrollback | Cmd+Shift+K | 기존 |

### View menu

| Item | 단축키 | 구현 |
|---|---|---|
| Bigger | Cmd+= | 기존 (FontZoomIn) |
| Smaller | Cmd+- | 기존 (FontZoomOut) |
| Actual Size | Cmd+0 | 기존 (FontReset) |
| Toggle Full Screen | Ctrl+Cmd+F | Apple 표준 selector `toggleFullScreen:` |

### Window menu

Apple 자동 처리 (NSWindow 표준):
| Item | 단축키 | 구현 |
|---|---|---|
| Minimize | Cmd+M | `performMiniaturize:` 자동 |
| Zoom | — | `performZoom:` 자동 |
| Bring All to Front | — | `arrangeInFront:` 자동 |

### Help menu

| Item | 단축키 | 구현 |
|---|---|---|
| pj001 Help | Cmd+? | 1차 cut: README URL을 'open'으로 열기 (또는 placeholder) |

## 4. 구현 옵션

### A — selector 직접 라우팅 (full custom)

각 menu item에 `target/action` 설정 + 우리 `Pj001AppDelegate` 같은 NSObject 서브
클래스에 해당 selector 구현. winit이 NSApplicationDelegate를 점유하므로 별도
controller object 필요.

복잡도 ★★★ — objc2 declare_class + Mutex/channel로 우리 AppState에 routing.

### B (권장 1차 cut) — keyEquivalent + 기존 keyboard chain

NSMenuItem의 `keyEquivalent`/`keyEquivalentModifierMask`만 설정 + `action`은 nil.
AppKit이 menu item 시각화 + keyboard event는 그대로 우리 winit handler에 전달.
즉 menu는 **시각 hint + 표준 macOS UX**만 제공, 실 동작은 기존 단축키 chain.

장점:
- 단순 (selector 라우팅 없음)
- 기존 단축키 로직 1:1 유지 (회귀 0)
- macOS 표준 menu 시각/검색(Cmd+? Help search) 제공

한계:
- menu item 클릭으로 실 동작 안 함 (단순 시각만). 사용자 클릭은 단축키 입력이
  더 빠른 path. 1차 cut UX로 수용 가능.
- Apple 표준 selector(About/Hide/Quit/toggleFullScreen 등)는 nil action 안 됨 —
  그 항목들은 Apple selector 직접 사용 (NSApplication first-responder chain).

### C — 하이브리드

Apple 표준 selector(About/Hide/Hide Others/Show All/Quit/Services/Minimize/Zoom/
Bring All/Full Screen)는 selector 사용, 우리 custom 명령(New Tab/Split/Find/Zoom
+/-/0)은 keyEquivalent만. 즉 B를 기본으로 깔고 Apple 표준은 C.

**1차 cut 채택: C (하이브리드)**.

## 5. 구현 단계

### Step 1 — macos_menu.rs 신규 모듈
- objc2-app-kit::{NSMenu, NSMenuItem, NSApplication}
- `attach_menu_bar(mtm: MainThreadMarker)` 함수
- App / Shell / Edit / View / Window / Help 6 menu 생성
- Apple selector(`orderFrontStandardAboutPanel:` 등)는 typed binding 또는 raw
  `selector!()` 매크로
- 기타 항목은 keyEquivalent만 set

### Step 2 — finish_startup에서 호출
`AppState::new_with_size` 또는 finish_startup 안에서 한 번. winit이 NSApplication
초기화한 후라야 함 — finish_startup 시점이 안전.

### Step 3 — Cargo.toml feature
`objc2-app-kit` features에 `NSMenu`, `NSMenuItem`, `NSApplication` 추가.

### Step 4 — 검증
- cargo test 310 회귀 0.
- release smoke로 panic 없음 + 상단 menu 시각 확인 (사용자 시각 QA 필요).
- 단축키 회귀 0 (기존 Cmd+T/W/F 등 정상).

## 6. 알려진 한계

- Preferences GUI 없음 → menu item 클릭하면 placeholder (또는 `open ~/.config/pj001/config.toml`).
- New Window는 multi-window milestone 전에는 placeholder (disabled or no-op).
- Help는 README URL `open`.
- localization 없음 (영문).
- Services submenu — Apple 자동.
- Window 목록 — `windowsMenu` 자동 채우기.

## 7. 위험

- selector 이름/시그니처 오타 → 동작 안 함 (compile 통과). raw `sel!()` 매크로 사용 시.
- finish_startup이 winit `Window::create` 이후라 `NSApp` 이미 활성. setMainMenu 정상 동작
  추정. 만약 winit이 menu 자체 설정한다면 우리 setMainMenu가 덮어쓸 수도 — 검증 필요.
- objc2 typed binding `NSApplication::sharedApplication(mtm)` 사용.

## 8. 검증 acceptance

- [ ] cargo test 310 회귀 0
- [ ] clippy 신규 0
- [ ] release smoke panic 없음
- [ ] 상단 menu bar 6 menu 시각 표시 (사용자 수동)
- [ ] Cmd+T/W/F/A/C/V/K/D/=/-/0/Q 모두 정상 (단축키 회귀 0)
- [ ] Apple 표준 항목 (About/Hide/Minimize/Zoom/Full Screen) 정상 작동
- [ ] menu item 클릭으로 Apple 표준 항목은 실 동작 (Cmd+H, About panel 등)
