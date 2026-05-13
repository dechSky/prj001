# pj001 settings and context menu plan

**상태**: draft, 2026-05-13.  
**목적**: Mac Terminal에 가까운 일상 UX를 만들기 위해 설정, keybinding, profile, 우클릭 동작을 어떤 순서와 정책으로 구현할지 정한다.

## 1. Web analysis summary

### 1.1 Apple Terminal

Apple Terminal은 `Terminal > Settings` 아래에 General, Profiles, Window Groups, Encodings를 둔다. Profiles는 색, 폰트, 커서, 배경뿐 아니라 Window/Tab/Shell/Keyboard/Advanced 설정까지 포함한다. Keyboard profile에는 custom key list, Option-as-Meta, alternate screen scroll 설정이 있다.

출처:
- Apple Terminal settings: https://support.apple.com/guide/terminal/trml789a1819/mac
- Apple Terminal profiles: https://support.apple.com/guide/terminal/trml107/mac
- Apple Terminal keyboard profile settings: https://support.apple.com/guide/terminal/trmlkbrd/mac

기획 반영:
- pj001도 “앱 전역 설정”과 “profile별 설정”을 분리한다.
- 우선 GUI보다 config file을 먼저 둔다.
- Option-as-Meta와 alternate-screen scroll은 Mac Terminal 호환 핵심 옵션으로 P1에 포함한다.

### 1.2 iTerm2

iTerm2는 profile/gesture/action이 강하고, right click/context menu/paste/open URL/semantic history 같은 pointer action을 preference에서 세밀하게 연결한다. terminal 기본값 하나로 고정하기보다 profile 또는 pointer binding으로 동작을 바꾸는 모델이다.

출처:
- iTerm2 preferences/actions: https://iterm2.com/documentation-preferences.html

기획 반영:
- 우클릭은 hard-coded copy/paste가 아니라 action enum으로 둔다.
- URL/파일 경로 semantic action은 OSC 8/semantic history 도입 뒤 연결한다.

### 1.3 Ghostty

Ghostty는 config 중심이다. keybinding은 `keybind = trigger=action` 모델이며 macOS global keybind, unconsumed/performable 같은 prefix를 지원한다. right-click은 `context-menu`, `paste`, `copy`, `copy-or-paste`, `ignore` 중 선택 가능하고 기본값은 `context-menu`다. copy-on-select도 옵션이다.

출처:
- Ghostty keybinds: https://ghostty.org/docs/config/keybind
- Ghostty config reference: https://ghostty.org/docs/config/reference

기획 반영:
- pj001 기본 우클릭은 `context-menu`가 맞다. 지금처럼 “선택 있으면 즉시 copy”는 빠르지만 Mac 앱 기대와 다르다.
- `copy-or-paste`는 옵션으로 제공하되 기본값으로 두지 않는다.
- keybinding config는 string DSL보다 우선 TOML table 기반으로 시작한다.

### 1.4 WezTerm

WezTerm은 key/mouse binding이 모두 config로 열린다. 기본 mouse 동작은 left drag selection, double word, triple line, Shift+click extend, Alt+drag block selection이다. 앱이 mouse reporting을 켜면 mouse events는 앱으로 전달되고, Shift를 누르면 reporting을 우회해 terminal selection으로 처리한다.

출처:
- WezTerm mouse bindings: https://wezterm.org/config/mouse.html
- WezTerm key bindings: https://wezterm.org/config/keys.html
- WezTerm default keys: https://wezterm.org/config/default-keys.html

기획 반영:
- M14 mouse reporting 설계에서 Shift bypass는 필수다.
- Selection UX 잔여분은 Shift+click extend, Alt+drag block selection, context menu ordering 순으로 진행한다.
- keybinding은 physical key 기준을 기본으로 삼아 keyboard layout 변동과 `Cmd+Option+K` 같은 입력을 안정화한다.

## 2. Recommended pj001 policy

### 2.1 Configuration model

1차는 `~/.config/pj001/config.toml` 파일만 지원한다. GUI Preferences는 P3로 미룬다.

권장 구조:

```toml
[general]
confirm_before_closing = "non_shell_processes"
restore_apple_shell_sessions = false

[profile.default]
font_family = "Menlo"
font_size = 14.0
scrollback_lines = 10000
option_as_meta = false
scroll_alt_screen = false
copy_on_select = false
right_click_action = "context_menu"
confirm_multiline_paste = true
bell = "visual"

[profile.default.colors]
foreground = "#f2f2f2"
background = "#3f3f4b"
cursor = "#f2f2f2"
cursor_text = "#3f3f4b"
selection = "#7da5b8"
bold = "#ffffff"
ansi = [
  "#000000", "#cc5555", "#55cc55", "#cccc55",
  "#5555cc", "#cc55cc", "#55cccc", "#cccccc",
  "#555555", "#ff7777", "#77ff77", "#ffff77",
  "#7777ff", "#ff77ff", "#77ffff", "#ffffff",
]

[keybindings]
"phys:cmd+k" = "clear_and_redraw"
"phys:cmd+shift+k" = "clear_scrollback"
"phys:cmd+option+k" = "clear_scrollback"
"phys:cmd+f" = "find"
"phys:cmd+comma" = "open_config"

[mouse]
shift_bypasses_mouse_reporting = true
alt_drag = "block_selection"
```

### 2.2 Config load order

1. built-in defaults
2. config file
3. CLI flags
4. runtime changes, future GUI

잘못된 config는 앱 시작 실패보다 warning + default fallback이 낫다. 단축키 충돌은 마지막 binding wins 정책을 사용하고 log로 남긴다.

### 2.3 Profiles

profile은 terminal behavior와 visual style을 함께 묶는다.

P1 필수:
- font family / font size
- color scheme: fg/bg/cursor/cursor text/selection/bold + 16 ANSI colors
- scrollback lines
- Option-as-Meta
- alternate-screen scroll
- shell command / cwd inheritance policy
- right-click action
- copy-on-select
- multi-line paste confirmation
- bell: none / audible / visual

P2 이후:
- cursor shape defaults
- window default size
- tab title format
- environment overrides

Profile과 quick spawn preset의 경계:
- profile은 visual/behavior default 묶음이다.
- quick spawn preset은 “무엇을 실행할지”를 고른다.
- preset이 profile을 지정하지 않으면 active pane의 profile을 상속한다.
- preset이 shell command를 지정하면 profile shell보다 우선한다.
- 새 tab/new pane은 active pane의 `cwd`와 profile을 기본 상속한다.

## 3. Context menu and right-click plan

### 3.1 Current behavior

현재 pj001 우클릭:
- clicked pane 활성화
- selection이 있으면 clipboard copy
- selection이 없으면 아무 동작 없음

문제:
- Mac 앱의 일반 기대인 context menu가 없다.
- 사용자는 “우클릭 메뉴가 없다”, “붙여넣기 메뉴가 없다”고 느낀다.
- copy-or-paste류 즉시 실행은 편하지만, terminal에서는 오입력/명령 실행 위험이 있다.

### 3.2 Default behavior

기본값은 `right_click_action = "context_menu"`로 한다.

context menu trigger:
- mouse right click
- trackpad secondary click
- Control+left click

세 이벤트는 같은 `ContextMenuTrigger`로 정규화한다.

우클릭 메뉴 항목:
- Copy: selection 있을 때 enabled
- Paste: clipboard text 있을 때 enabled
- Paste Selection: terminal selection buffer 도입 뒤 future
- Paste Escaped Text: future
- Clear Scrollback
- Clear and Redraw
- Select All
- Open Link: OSC 8/url under pointer 있을 때 enabled
- Open File: semantic path under pointer 있을 때 future
- Split Right
- Split Down
- New Tab
- Close Pane
- Settings / Open Config

주의:
- Paste는 메뉴 항목 선택 시에만 실행한다. right-click 자체로 paste하지 않는다.
- clipboard text에 newline이 있으면 `confirm_multiline_paste`가 true일 때 확인 overlay를 띄운다.
- right-click 위치의 pane을 먼저 active로 만든다.
- selection이 있고 그 selection 위에서 우클릭하면 selection 유지.
- selection 밖에서 우클릭하면 selection clear 여부는 config로 둔다. 기본은 유지.

### 3.3 Optional right-click modes

지원할 enum:

```rust
enum RightClickAction {
    ContextMenu,
    Paste,
    Copy,
    CopyOrPaste,
    Ignore,
}
```

기본값:
- macOS app convention: `ContextMenu`
- power user optional: `CopyOrPaste`
- 현 구현과 가장 가까운 동작: `Copy`

### 3.4 Implementation slices

M19-1 config foundation:
- config parser 도입
- built-in defaults + file overlay
- keybinding/right-click enum validation
- `Cmd+,`는 1차에서 config file 경로 log/open만 수행

M19-2 right-click action enum:
- current hard-coded right-click copy를 `RightClickAction::Copy`로 이관
- default는 아직 `Copy`로 유지해서 회귀를 줄임
- config에서 `ContextMenu`를 읽을 수 있게만 준비

M19-3 context menu overlay MVP:
- winit/window layer에서 platform context menu 가능성 조사
- 단기 fallback: pj001 자체 overlay menu
- Copy/Paste/Clear/New Split/New Tab/Close Pane만 먼저 구현
- keyboard navigation과 accessibility는 P2로 미루되, Escape/dismiss와 click-outside는 MVP에 포함

M19-4 default right-click switch:
- context menu MVP가 준비된 뒤 기본값을 `ContextMenu`로 변경
- `Copy`, `Paste`, `CopyOrPaste`, `Ignore`는 옵션으로 유지

M19-5 mouse/key binding:
- `right_click_action` 적용
- keybinding table 적용
- key syntax는 `phys:`를 기본 예시에 사용하고, `mapped:`는 후속 explicit 지원

M19-6 profile:
- default profile 한 개
- startup/new tab/new pane profile 적용
- profile별 font/color/scrollback/shell

## 4. Decision table

| 주제 | 결정 | 이유 |
|---|---|---|
| 기본 우클릭 | context menu | Apple/macOS 앱 기대와 Ghostty 기본값에 가장 가까움 |
| 우클릭 paste | 옵션 | terminal 오입력 위험 때문에 기본 금지 |
| 우클릭 copy | 옵션 | 현재 동작 보존 가능, 기본은 menu Copy |
| copy-on-select | 기본 false | Apple Terminal에 가까운 보수값. Ghostty식 자동 복사는 옵션 |
| keybinding | config에서 변경 가능 | macOS global shortcut 충돌 대응 필수 |
| key matching | physical key 기본 | keyboard layout, Option 변환에 강함 |
| Option key | `option_as_meta` profile 옵션 | Apple Terminal Profiles Keyboard와 일치 |
| alt screen wheel | `scroll_alt_screen` profile 옵션 | Apple Terminal Profiles Keyboard와 일치 |
| GUI settings | P3 | 지금은 config가 빠르고 테스트 가능 |

## 5. Next recommended work

1. `Config`에 user-facing settings 구조를 추가하지 말고, 먼저 별도 `settings` 모듈에서 TOML parse + defaults를 만든다.
2. `RightClickAction` enum을 추가하고 현재 hard-coded 우클릭 copy를 `Copy` 모드로 이관한다.
3. 기본값을 `ContextMenu`로 바꾸기 전에 context menu MVP가 있어야 한다. 따라서 구현 순서는 config parser → context menu overlay → default 변경이 맞다.
4. keybinding config는 `Cmd+Shift+K` 충돌 사례 때문에 `clear_scrollback`부터 dogfood 한다.
