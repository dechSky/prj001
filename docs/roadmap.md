# pj001 — MVP+ 로드맵 (M11~M19)

**상태**: 로드맵 v2 (2026-05-08, historical). 코드/세부 설계 미반영. **2026-05-11부로 정본 이전 — 실제 milestone trajectory는 `archive/docs/architecture/m12-m16-pj001-sessions-tabs-bridge-plan.md` 참조**.
**목적**: M7까지 도달한 "표시·실행" 영역 위에, **Mac 터미널 일상 사용 격차**를 메우고 그 위로 확장.
**스타일**: high-level 1장. 각 단계 진입 직전 별도 `M??-design.md` 작성(M7 cursor / M8 keyboard와 동일 패턴).

**v2 변경 (vs v1, 누락 점검 반영)**: M11 신설(VT 인프라 보강) → M12~M19로 시프트. M13(OSC) 스코프 확장. M15(단축키) 확장. §15 out-of-scope 박스 신설. §16 post-MVP+ cleanup 추가.

---

## 2026-05-11 갱신 — 정본 이전 + milestone 번호 재배치

**완료 상태**: M1~M10 + M17(reflow) + **M11 workspace 분리**(commit `396a536`, cargo test 66 → 75) 완료. archive-bridge MVP crate 생성(archive `f18b257`).

**상용 급 OSS 지향 확장**: 학습 목적에서 "macOS 전용 빠르고 정확한 GPU 터미널, Alacritty/Ghostty 하위 80% 사용 사례 통과, AI CLI 깨뜨리지 않는 host terminal"(Codex 정의)로 1차 목표 확장. 광역 웹 분석(Warp/iTerm2/Ghostty/WezTerm/Alacritty/Kitty/Terminal.app) + Codex 2nd opinion 완료.

### 새 milestone 번호 (정본: archive m12-m16 plan)

| # | 본 로드맵 (v2, deprecated) | 새 번호 (m12-m16 plan) | 주제 |
|---|---|---|---|
| ~~M11~~ | VT 인프라 보강 (ICH/DCH/IL/DL/ECH/DECSTR/RIS/G0+DEC line drawing) | **재배치 필요** — 새 trajectory 어디에 들어가는지 미확정 (M13/M14 단계 외부) |
| M12 (신규) | — | **Session 추출** (PTY ↔ visible slot 분리, refactor only) |
| M13 (신규) | — | **Layout tree** (BSP, flexible split + divider drag + 3+ pane) |
| M14 (신규) | — | **Tabs** (tab bar, Cmd+T, Cmd+숫자) |
| M15 (신규) | — | **AgentKind + dynamic spawn** |
| M16 (신규) | — | **Bridge graph** (multi-select N-target routing) |

### 본 로드맵 잔여 항목 처리

본 로드맵 §2~§10의 M11~M19 모두 새 trajectory와 충돌 또는 미정렬. **archive m12-m16 plan + 상용 갭 분석 P0~P3 권고**가 정본이며, 본 로드맵 잔여 항목은 다음 분류로 재처리 예정:

1. **VT 인프라 보강** (원래 M11): 새 trajectory와 직교 — 별도 milestone(M13 layout tree 진입 전 또는 그 사이)로 재배치 권장. P0 보안 트랙(paste/drop sanitization + fuzzing) 동반.
2. **마우스 selection + Cmd+C** (원래 M12): M14 tabs 이후 또는 P0 보안 트랙과 묶음.
3. **OSC bundle / mouse reporting / find / 폰트 줌** (원래 M13~M16): m12-m16 plan과 직교 — 별도 trajectory로 유지, P1.
4. **resize reflow** (원래 M17): ✅ M17로 이미 완료(commit `4ebc513`).
5. **탭/멀티창** (원래 M18): m12-m16 plan의 M14 Tabs로 흡수.
6. **환경설정** (원래 M19): 상용 갭 분석에서 config file + keybindings + profiles는 P1로 격상.

### 의도적 미지원 재검토 (Codex 권고)

- **OSC 52** 미지원 → **opt-in (기본 off)**: `allow_osc52 = off/ask/local-only/on`. Claude Code류 `/copy` 호환.
- **Kitty graphics** 미지원 → **P3 experimental flag**. AI 인라인 이미지 트렌드 대비.
- **Sparkle** 미지원 → **공개 사용자 1000+ 시점 도입**. 지금은 수동 GitHub release + notarized DMG.
- **Triggers / OSC 1337 / tmux deep / i18n / telemetry / Hotkey window** 미지원 유지.

### 상용 급 P0~P3 (Codex 합의)

- **P0**: m12-m16 trajectory(M12 layout 일반화 → M13 BSP) + VT 인프라 보강 + 마우스 selection + paste/drop sanitization + fuzzing + crash log local bundle
- **P1**: OSC 8/133/7 + title stack + config + keybindings + profiles + KKP + mouse reporting + find + buffer clear + font zoom
- **P2**: Developer ID signing + notarization + release DMG + 보안 정책 문서 + ligature + accessibility minimal tree
- **P3**: preferences GUI + vibrancy/blur + Sparkle + Quick terminal + Kitty graphics experimental + AppleScript/Shortcuts

---

## (이하 historical — 2026-05-08 v2 본문)

---

## 0. 현재 상태 (2026-05-08)

| 영역 | 상태 |
|---|---|
| M1~M7 | 완료 — 창/PTY/렌더/색/alt screen/한글/IME/cursor 고도화 |
| **M8 (keyboard)** | **완료** (다른 세션, 7 sub-step + M8-7 OSC 7 보강 커밋 `87a025a`) |
| **M9 (modifier 인코딩 + DECPAM/DECPNM)** | **다른 세션 예약** — keyboard-design §10.1 |
| **M10 (bracketed paste + focus reporting)** | **다른 세션 예약** — keyboard-design §10.2 |
| M11~ | **본 로드맵 범위** |

**M8-7 효과로 본 로드맵에 영향**:
- `vte::Perform::osc_dispatch` 분기가 이미 정비됨 (OSC 0/2 + OSC 7 파싱 + URL decode + home-relative).
- `Term::{title, set_title, take_title_if_changed}` API 존재.
- `pty/mod.rs`가 `TERM=xterm-256color` + `TERM_PROGRAM=Apple_Terminal` env 설정 — `/etc/zshrc_Apple_Terminal` (그리고 bash 변종)이 자동 OSC 7 송신. **shell-side 사용자 hook 불필요(macOS 기본 zsh/bash 한정)**.
- M13-2(OSC 7) 스코프 축소 — Term에 `cwd: Option<PathBuf>` 필드 추가 + 기존 OSC 7 분기에서 같이 저장하면 끝.
- M11 DA/DA2 응답 레벨 기본 결정: **`xterm-256color`** (env가 이미 그 값으로 셋업).

본 로드맵은 M8~M10이 완료된 후의 작업을 다룬다. M10에서 노출될 `Term::bracketed_paste_active()` 등의 게이트는 M12 페이스트 처리에서 그대로 활용한다.

---

## 1. 전체 그림

| # | 제목 | 크기 | 일상 가치 | 학습 가치 | 의존 |
|---|---|---|---|---|---|
| **M11** | **VT 인프라 보강** (charset · 라인 편집 · DA/DSR · 리셋) | M | ★★★(이미 깨진 부분 복구) | ★★★ | 독립 |
| **M12** | 마우스 인프라 + 선택 + 클립보드 + drag-drop | L | ★★★ | ★★ | M10 (bracketed paste flag) |
| **M13** | OSC bundle — title · palette · CWD · hyperlink · cursor color · semantic prompt · title stack | M~L | ★★ | ★★ | 독립 (M12 OSC infra와는 무관) |
| **M14** | xterm mouse reporting (1000/1002/1003/1006) | M | ★★ | ★★ | M12 (마우스 좌표 인프라) |
| **M15** | 폰트 줌 + buffer/scrollback clear + prompt jump | S~M | ★★ | ★ | M8-1, M13 (OSC 133) |
| **M16** | Cmd+F find (scrollback 검색) | M | ★★ | ★★ | M12 (selection 렌더 재사용) |
| **M17** | resize reflow (정책 변경) | L | ★★ | ★★★ | 독립 (grid 모델 침습) |
| **M18** | 탭 / 멀티 창 | L | ★★ | ★★ | **M13 OSC 7** (같은 디렉토리 새 탭) |
| **M19** | 환경설정 (config file → UI) | L | ★ | ★★ | M15, M18 |

**굵직한 분기점**: M11(보이지 않는 인프라 복구) · M12(일상 사용 가능 시점) · M17(MVP 정책 변경) · M18(단일창 가정 깨짐).

**"Mac 터미널 평행선" 도달**: M11~M16 완료 시점.
**그 이상**: M17~M19.

---

## 2. M11. VT 인프라 보강  [M]

**목표**: 현재 깨져 있는 VT primitive를 메워서 **mc / nethack / ncurses 박스가 정상 그려지고 vim startup hang이 사라지는** 상태.

### 2.1 배경

현재 `vt::perform.rs` csi_dispatch가 처리하는 action: `m / A / B(e) / C(a) / D / H(f) / G(\`) / d / J / K / S / T / r / SP q`. esc_dispatch는 `7 / 8`. **누락된 핵심 primitive로 인해 readline 편집 / DEC line drawing / 앱 startup probe가 부분 미동작**. M5에서 약속한 "shell 풀가동" 검증이 실제로는 vim/git/htop에 한정되어 있고, mc/nethack 같은 ncurses-heavy 앱에서 박스가 깨짐.

### 2.2 스코프 (in)

| 시퀀스 | 이름 | 영향 |
|---|---|---|
| `CSI Pn @` | **ICH** insert chars | bash/zsh readline 라인 중간 삽입 |
| `CSI Pn P` | **DCH** delete chars | readline `Ctrl+D` 등 |
| `CSI Pn L` | **IL** insert lines | full-screen editor |
| `CSI Pn M` | **DL** delete lines | full-screen editor |
| `CSI Pn X` | **ECH** erase chars | scroll 없이 지우기 |
| `CSI c` / `CSI > c` | **DA / DA2** device attributes | vim/screen startup probe |
| `CSI 6n` | **DSR** cursor pos report | bash COLUMNS 자동 감지 |
| `CSI ! p` / `ESC c` | **DECSTR / RIS** soft/full reset | `reset` 명령어 |
| `ESC ( B` / `ESC ( 0` / SI(0x0F) / SO(0x0E) | **G0 charset + LS0/LS1** (DEC line drawing) | **mc / nethack 박스** |

### 2.3 스코프 제외

- **G1/G2/G3 + SS2/SS3** — 거의 안 쓰임. 일단 G0만. 발견 시 M19 직전 cleanup.
- **REP / CHT / HTS / TBC** — 영향 작음. §16 post-MVP+ cleanup으로.
- **DECSCNM** (reverse video screen) — 거의 안 쓰임.
- **DECRQM** (request mode) — 응답 형식만 정의되면 추후.

### 2.4 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M11-1 | ICH/DCH/IL/DL/ECH — Term 메서드 + csi_dispatch 분기 | unit test (Term API 직접) + bash readline 시각 |
| M11-2 | DA/DA2/DSR — Term이 응답 byte를 PTY write 채널로 송신 (구조 변경 작음) | unit test + vim startup hang 사라짐 |
| M11-3 | DECSTR/RIS — 현재 상태 reset 메서드 정리 | `reset` 명령 동작 |
| M11-4 | G0 charset + DEC line drawing — Cell 직전 글리프 변환 (charset state in Term, vt::perform이 print 콜백에서 변환) | mc 실행 → 박스 정상. ncurses test apps |

### 2.5 리스크

- **DEC line drawing 글리프 매핑** — `0x6a~0x78` (j~x) 영역에 `┘ ┐ ┌ └ ┼ ─ ├ ┤ ┴ ┬ │` 매핑. Unicode 전환 후 cosmic-text fallback이 처리 → atlas insert (M5 인프라 그대로).
- **PTY write 채널** — DA/DSR 응답을 Term이 직접 PTY로 못 씀. `Arc<Mutex<...>>` 또는 mpsc 채널로 reader thread → main thread → PTY writer 경로 필요. 진입 시 advisor 검토.
- **Term 메서드 신설량** — 5~10개 메서드 추가 + 단위 테스트.

### 2.6 cross-cut

- 이 단계의 **PTY 응답 채널**(DA/DSR)은 **M13 OSC 7 응답 query** / **M14 mouse reporting**에서 그대로 재사용.

---

## 3. M12. 마우스 + 선택 + 클립보드 + drag-drop  [L]

**목표**: 드래그 선택 → Cmd+C → Cmd+V로 일상 작업 가능.

### 3.1 스코프 (in)

- **마우스 좌표 인프라** — `WindowEvent::MouseInput` / `CursorMoved` (휠은 M6-10 이미 있음). 픽셀 → cell `(row, col)` 변환. scrollback view offset, WIDE_CONT, alt screen 모두 정확.
- **선택 모델** — 1차: `Selection { anchor: AbsLine, head: AbsLine, kind: Char | Word | Line }`. AbsLine은 `total_lines_emitted: u64` 단조 카운터 기반.
- **인터랙션** — left-down anchor / drag 확장 / left-up 확정. **double-click word**, **triple-click line**, **shift+click anchor 확장**, **Cmd+A select all**.
- **렌더** — selection overlay를 별도 instance batch로. blend mode = source-over, 색 = `bg + alpha*0.3`.
- **클립보드** — `arboard` 크레이트. Cmd+C: selection을 텍스트로 직렬화(WIDE_CONT 스킵, line break 처리) → `Clipboard::set_text`. Cmd+V: `Clipboard::get_text` → PTY write. **bracketed paste 활성 시** `\e[200~` … `\e[201~` 래핑.
- **file drag-and-drop** — `WindowEvent::DroppedFile` 받으면 shell-quoted path를 PTY write. bracketed paste 동일 적용.
- **panic hook + 로그 파일** — 저비용으로 같이 도입. `~/.config/pj001/crash.log`에 panic backtrace + env_logger를 파일로도 남김. 사용자 리포트 수집.
- **선택 vs mouse reporting 우선순위 정책**: M14 도입 후 reporting ON이면 raw 마우스는 reporting로, **Shift 누르면 selection 모드**로 (xterm/iTerm 표준).

### 3.2 스코프 제외

- **Block(box) selection** — Alt+drag. 1차 cut 제외. M12+1 또는 future.
- **자동 URL 감지**(정규식 기반 hyperlink) — OSC 8(M13)과 분리.
- **drag-to-scroll** — 1차 cut 제외 (휠로 대체).
- **wrapped line의 정확한 줄 합치기** — logical line 정보 없으므로 wrap 위치에 `\n` 그대로. M17에서 자연 해결. §13 결정 참고.

### 3.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M12-1 | 마우스 좌표 인프라 + `Selection` 타입 + `total_lines_emitted` 카운터 + char-level drag | 로그로 `(AbsLine, col)` 정확 확인 |
| M12-2 | overlay 렌더 (selection 색 칠) | 시각 |
| M12-3 | double-click word / triple-click line / shift+click / Cmd+A | 시각 |
| M12-4 | arboard + Cmd+C / Cmd+V (+ bracketed paste 래핑) | shell 라인에 paste |
| M12-5 | file drag-and-drop | Finder에서 파일 끌어다 놓으면 path 입력 |
| M12-6 | panic hook + log 파일 출력 | `panic!()` 강제 후 crash.log 확인 |

### 3.4 리스크 / 결정 포인트

- **`arboard` macOS 동작 확인** — 진입 시 web/docs.rs 재확인. 대안: `objc2_app_kit::NSPasteboard` 직접.
- **AbsLine 좌표 stability** — scrollback 10k cap 도달 시 oldest evict로 raw scrollback index가 shift. AbsLine으로 해결 + evict된 anchor는 selection cancel.
- **WIDE_CONT 직렬화** — Cell 두 개지만 텍스트는 한 글자.
- **trailing whitespace** — 선택 시 라인 끝 공백 제거 (Mac 터미널 따라).
- **선택 ↔ scrollback view ↔ 렌더 race** — single snapshot 패턴(M8-1 동일).

### 3.5 cross-cut

- 마우스 좌표 인프라는 **M14 mouse reporting**과 직접 공유.
- selection 렌더 instance batch는 **M16 Cmd+F find 결과 highlight**에서 거의 그대로 재사용.

---

## 4. M13. OSC bundle  [M~L]

**목표**: 창 제목 + 색 팔레트 + CWD 리포팅 + 클릭 가능 URL + cursor 색 + semantic prompts + title stack.

### 4.1 스코프 (in)

vte::Perform::osc_dispatch 활성화. 첫 인자(파라미터 ID) 분기. 현재 OSC 0/2만 처리 → 아래 전부.

| 시퀀스 | 용도 |
|---|---|
| **OSC 0** | icon name + window title — `window.set_title(s)` |
| **OSC 1** | icon name only — 무시 또는 디버그 로그 |
| **OSC 2** | window title only — `set_title(s)` |
| **OSC 4** | 256색 팔레트 set/query (`OSC 4 ; n ; rgb:RR/GG/BB`) — vim color scheme 영향 |
| **OSC 7** | **CWD 리포팅** (`OSC 7 ; file://host/path`) — M18 "같은 디렉토리 새 탭" 의존성 |
| **OSC 8** | **hyperlink** (`OSC 8 ; params ; URI ST`) — `Cell.hyperlink_id` + Term registry. Cmd+hover cursor 변경 + Cmd+Click `open` |
| **OSC 10 / 11** | default fg / bg 변경 — neovim 등이 set |
| **OSC 12** | cursor color (정책 D 외 색 통제) |
| **OSC 133** | **semantic prompts** — `OSC 133 ; A/B/C/D ST`. Cell row에 prompt mark 비트만 저장. M15 prompt jump이 사용 |
| **CSI 22;0t / 23;0t** | **title stack** push/pop — ssh/tmux nested title 복원 |

### 4.2 스코프 제외

- **OSC 52** (clipboard via terminal) — 보안 이슈. 미지원 정책.
- **OSC 1337** (iTerm2 proprietary) — 미지원.
- **자동 URL 감지** (OSC 8 외 정규식) — post-MVP+.

### 4.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M13-1 | OSC 4/10/11/12 (색 set만, query는 응답 채널 사용) — OSC 0/1/2 dispatch는 M8-7에서 이미 정비됨 | `printf '\e]4;1;rgb:ff/00/00\e\\'` 후 빨강이 변함 |
| M13-2 | **OSC 7** — `Term::cwd: Option<PathBuf>` 필드만 추가하고 기존 M8-7 OSC 7 분기에서 같이 저장. **파싱·shell 자동 송신은 M8-7에서 완료**. (비-zsh/bash 셸은 사용자 측 hook 필요 — README 한 줄 메모) | cd 후 `term.cwd()` 정확 |
| M13-3 | **OSC 8** hyperlink — Cell ID + registry + underline 렌더 + Cmd+hover/Click | `printf '\e]8;;https://example.com\e\\link\e]8;;\e\\'` |
| M13-4 | **OSC 133** prompt mark — Term에 `prompt_marks: Vec<AbsLine>` 저장. 시각 변화 없음 (M15가 사용) | 로그로 mark 추가 확인 |
| M13-5 | **title stack** (CSI 22;0t / 23;0t) | ssh 진입/탈출 시 title 복원 |

### 4.4 리스크

- **Hyperlink ID 메모리** — 10k rows × 1 hyperlink/row = 10k entry. registry hard cap (4096 entry, LRU evict).
- **ST terminator 편차** — `\e\\` 표준이지만 BEL(`\x07`) 종료도 흔함 (vte가 처리할지 진입 시 확인).
- **macOS open 보안** — `Command::new("open").arg(&url)`. URL 스킴 whitelist (http/https/mailto만) 정책 결정.
- **OSC 4 query 응답** — DA/DSR과 동일 PTY write 채널(M11-2 인프라).

---

## 5. M14. xterm mouse reporting  [M]

**목표**: vim/tmux/htop 안에서 마우스 클릭/드래그/휠 동작.

### 5.1 스코프 (in)

- **DECSET / DECRST** — 1000(button-event), 1002(button + drag motion), 1003(any-event), 1006(SGR encoding mode).
- **legacy 인코딩**: `CSI M Cb Cx Cy` (Cb/Cx/Cy = byte 32+value, 1-based).
- **SGR 인코딩** (1006): `CSI < button ; col ; row M` press, `m` release.
- button 인코딩: 0=left/1=middle/2=right + 32(motion) + 64(wheel up)/65(wheel down) + modifier bits 4/8/16.
- **selection 우선순위**: reporting OFF면 selection만, ON이면 raw → reporting / **Shift+raw → selection**.

### 5.2 스코프 제외

- **mode 1005** (UTF-8 인코딩) — deprecated.
- **mode 1015** (urxvt 스타일) — deprecated.
- **focus reporting** (1004) — M10 영역.

### 5.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M14-1 | DECSET 1000 + legacy 인코딩 (press/release) | tmux pane click |
| M14-2 | DECSET 1002 (drag motion) | vim visual mode |
| M14-3 | DECSET 1006 SGR encoding | iTerm parity |
| M14-4 | Shift+raw → selection 분기 | reporting ON 상태에서 Shift 드래그 = 우리 selection |

### 5.4 리스크

- **mode 1003** keystroke 이벤트량 큼 — 성능 측정.
- **휠 + alt screen** — vim/less 안에서 휠 = scroll 키 인코딩.
- 진입 시 spec 재확인: xterm `ctlseqs` mouse 섹션.

---

## 6. M15. 폰트 줌 + clear + prompt jump  [S~M]

**목표**: Cmd+= / Cmd+- / Cmd+0 줌 + Cmd+K / Cmd+Shift+K clear + Cmd+↑ / Cmd+↓ prompt 점프.

### 6.1 스코프 (in)

- **Cmd+= / Cmd+- / Cmd+0** 폰트 줌 — `AppState.font_size_pt: f32` + 변경 시 `font::measure_cell` 재호출 → cell metric 갱신 + atlas 재생성(M5 정책 C 인프라). PTY size 재통보. clamp 6pt~72pt.
- **Cmd+K** buffer clear — main grid 전체 지우기, scrollback 유지. cursor `(0,0)`.
- **Cmd+Shift+K** scrollback clear — main 유지, scrollback 비움. `total_lines_emitted` 보존(AbsLine 좌표 일관성), selection cancel.
- **Cmd+↑ / Cmd+↓** prompt jump — M13-4에서 저장한 `prompt_marks: Vec<AbsLine>`을 이전/다음 mark로 view_offset 점프.
- **키 매핑** — Cmd 모디파이어. **M8-1**의 `ModifiersState` 인프라(keyboard-design §6.1) 활용. Cmd는 app-side 처리, PTY로 안 보냄 — M9(PTY encoding)와 무관.

### 6.2 스코프 제외

- 폰트 패밀리 변경 — M19 환경설정 영역.
- prompt mark 미존재 시 fallback — 1차 cut: 동작 없음 + 한 줄 로그.

### 6.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M15-1 | font_size_pt 필드 + reflow 함수 + Cmd+=/-/0 | 글자 크기 변함 |
| M15-2 | atlas 폐기 비용 측정 | 100ms 미만 확인 |
| M15-3 | Cmd+K / Cmd+Shift+K | buffer/scrollback 분리 동작 |
| M15-4 | Cmd+↑/↓ prompt jump (M13-4 의존) | shell prompt 사이 점프 |

### 6.4 리스크

- atlas 폐기 시 모든 글리프 다시 raster — 1024 ASCII < 100ms 예상, 실측 필수.
- font_size 변경 중 PTY 출력 race — 재생성 동안 pending text bounce.
- Cmd+Shift+K 시 selection anchor가 evict된 영역에 있으면 cancel — `total_lines_emitted` 보존이라 카운터 단절 없음.

---

## 7. M16. Cmd+F find  [M]

**목표**: scrollback + 현재 grid 텍스트 검색 + 결과 highlight + 점프.

### 7.1 스코프 (in)

- **검색 입력 UI** — 창 우상단 overlay. 별도 IME 비활성 입력 라인(`set_ime_allowed(false)`). Esc/Enter로 닫음.
- **검색 알고리즘** — literal substring. case-insensitive 토글.
- **검색 대상** — scrollback `VecDeque<Vec<Cell>>` + main grid. Cell → char 풀어내기 (WIDE_CONT 스킵).
- **결과 highlight** — M12 selection overlay batch 재사용 (다른 색).
- **n / N (Enter / Shift+Enter)** — 다음/이전 결과로 view_offset 점프.

### 7.2 스코프 제외

- **regex** — 1차 cut literal만.
- **검색 결과 export / 고정** — 후속.

### 7.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M16-1 | overlay 입력 라인 (별도 텍스트 컴포넌트) | 입력 가능 |
| M16-2 | substring search + 결과 highlight | 일치 cell 강조 |
| M16-3 | Enter/Shift+Enter 점프 + view_offset 자동 조정 | 커서 따라감 |

### 7.4 리스크

- **검색 입력 라인 = 첫 in-app 텍스트 입력 컴포넌트** — IME 충돌 방지.
- 큰 scrollback에서 수천 매치 — lazy iteration.

---

## 8. M17. resize reflow  [L]

**목표**: 가로 리사이즈 시 wrap된 line이 새로운 폭에 맞춰 재배치 (정책 변경: truncate → reflow).

### 8.1 스코프 (in)

- **logical line 모델** — 각 cell row에 `wrap_continuation: bool` 비트. main grid + scrollback 동일 구조.
- **resize 핸들링** — 이전 폭 logical line 추출 → 새 폭으로 wrap → grid 재구성.
- **cursor 위치 보존** — logical line + char index → 새 (row, col).
- **alt screen reflow 안 함** — vim 등 자체 처리.
- **M12 copy 정확도 자연 개선** — wrap된 라인 copy 시 `\n` 안 들어감.

### 8.2 스코프 제외

- **세로 리사이즈에서 scrollback expansion** — 1차 cut 제외 (행 수만 늘림).
- **double-width grapheme reflow** — 줄 끝에 wide만 남으면 다음 줄로 밀음.

### 8.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M17-1 | logical line 추출/재조립 코어 (헤드리스 unit test) | grid 단독 |
| M17-2 | resize hook 통합 (main + scrollback) | 시각: 긴 라인이 잘렸다 펼쳐짐 |
| M17-3 | cursor 좌표 보존 | shell prompt 위치 유지 |
| M17-4 | M12 copy 직렬화 logical line 사용 | wrap된 라인 paste 시 `\n` 사라짐 |

### 8.4 리스크

- **Alacritty가 한참 걸린 영역** — 학습 가치 큼, 시간도 큼.
- **현재 정책 F**: scrollback view에서 col mismatch는 truncate-on-read. M17 채택 시 정책 갱신 필요.
- **선택 좌표 무효화** — resize 중 진행 selection은 cancel.

---

## 9. M18. 탭 / 멀티 창  [L]

**목표**: Cmd+T 새 탭 (같은 CWD), Cmd+1~9 전환, Cmd+W 닫기.

### 9.1 결정 포인트 (진입 시 사용자 확인)

- **단일 창 + 탭바 자체 그림** vs **macOS native NSWindow tabbing** vs **멀티 winit 창**.
- 학습 측면 추천: **단일 창 + 탭바 자체 그림**.

### 9.2 스코프 (in, 단일 창 + 탭바 가정)

- **`Session`** — `{ pty: Pty, term: Arc<Mutex<Term>>, reader_thread: JoinHandle }` 묶음.
- **AppState.sessions: Vec<Session>** + `active: usize`.
- **렌더** — 활성 세션 grid만 + 상단 탭바 (cell row 하나 점유).
- **단축키** — Cmd+T / Cmd+1~9 / Cmd+W / Cmd+Shift+] / Cmd+Shift+[.
- **Cmd+T 의 CWD 이어받기** — **M13 OSC 7 의존**. 새 세션 spawn 시 active session의 `term.cwd()`를 `CommandBuilder::cwd`로 주입.
- **PTY 리더 thread × N** — 각 세션 별. EventLoopProxy로 `RepaintRequested(session_idx)` 통지.

### 9.3 스코프 제외

- **분할 창 (split)** — tmux 영역. 미지원.
- **세션 dump/restore** — 후속.

### 9.4 리스크

- **wgpu 자원 단일 device 유지** — atlas/pipeline 모든 세션 공유. surface 1.
- **활성 전환 시 per-session 상태** — view_offset, selection 등.
- **탭바 클릭** — M12 마우스 인프라 재사용 (탭바 영역 hit-test).

---

## 10. M19. 환경설정  [L]

**목표**: 폰트/색/단축키 사용자 설정.

### 10.1 분해

| 단계 | 내용 |
|---|---|
| **M19-1 config file** | `~/.config/pj001/config.toml` — `font.{family, size}`, `color_scheme.*`(16색 + bg/fg + cursor + selection), `keybindings.*` |
| **M19-2 hot-reload** | `notify` 크레이트로 watcher. atlas 재생성 / 색 갱신 |
| **M19-3 Cmd+, settings GUI** (선택) | 별도 창 또는 in-app overlay. 1차 cut: 파일 편집만 |

### 10.2 리스크

- schema 검증 — 잘못된 toml 에러 메시지.
- 단축키 충돌 — 매핑 검증.
- GUI 빌드 시 wgpu 멀티 surface — M18 영역.

---

## 11. cross-cutting / 인프라 변경 추적

| 인프라 | 도입 단계 | 재사용 단계 |
|---|---|---|
| **PTY 응답 채널** (Term → main → PTY write) | **M11-2 (DA/DSR)** | M13-1(OSC 4 query), M14(mouse encoding) |
| 마우스 좌표 hit-test | M12 | M14, M18(탭바), M13-3(Cmd+Click) |
| Selection overlay batch | M12 | M16 (find highlight) |
| `total_lines_emitted` AbsLine | M12 | M13-4(prompt mark), M15(prompt jump), M17 |
| atlas 재생성 (DPI/font size) | M5 (정책 C) → M15 확장 | M19-1 |
| logical line 모델 | M17 | M12 copy 정확도 자연 개선, M16 검색 라인 결합 |
| OSC dispatch 인프라 | M13 | 미래 OSC 확장 |
| modifier 인프라 | M8-1 (다른 세션) | M12(Shift+Click), M14(Shift+raw), M15(Cmd+*) |
| panic hook + log 파일 | M12-6 | 모든 후속 단계 (디버그 인프라) |

**진행 중 재고려 포인트**: M17 logical line 모델을 M12 직렬화 시점에 미리 도입할지(선택 텍스트 줄바꿈 처리 일관). 진입 직전 advisor 검토. 현재 결정: **§13.1 옵션 — M12 한계 문서화로 진행, M17이 자연 해결**.

---

## 12. 진행 규칙

1. **각 M??-design.md 작성**을 코드 진입 직전에 — M7 cursor / M8 keyboard 패턴 그대로.
2. **advisor 게이트** — design.md 작성 후 advisor 호출, 응답 반영 후 코드.
3. **시각 검증 + cargo test 자동** — 둘 다. 학습 프로젝트지만 회귀 방지 우선. M11은 unit test 가능 비중 큼(헤드리스).
4. **CLAUDE.md 환각 방지 4원칙** — spec 인용 시 출처 표기, 진입 시 docs.rs/spec 재확인.
5. **다른 세션 충돌 회피** — 같은 파일 동시 편집 금지. 진입 전 `git status` + 다른 세션과 합의.

---

## 13. 결정 / 미해결

### 13.1 결정 (advisor 검토 + 누락 점검 반영)

- **M11 우선** — VT 인프라 보강을 마우스/선택 앞에 둠. 이유: M5 "shell 풀가동" 검증의 부분 미달 부분(mc/nethack/ncurses 박스 + readline 편집 + vim startup probe) 복구가 학습 목표 정렬에 우선.
- **M17 vs M12 순서** — **M12 먼저, wrapped line 줄바꿈 한계 문서화로 진행**. 일상 가용성 이득 먼저, 한계는 다수 use case에서 수용 가능, M17이 자연 해결.
- **selection vs mouse reporting (M14)** — xterm/iTerm 표준: reporting ON이면 raw → reporting, **Shift+raw → selection**.
- **OSC 7 ↔ M18 의존성** — 명시. M18 진입 전 M13-2 완료 필수.
- **OSC 133 ↔ M15 prompt jump 의존성** — 명시. M15-4 진입 전 M13-4 완료 필수.

### 13.2 미해결 (사용자 결정 필요)

- **M11 DA/DA2 응답 레벨**: **`xterm-256color`로 잠정 결정** — pty/mod.rs가 이미 `TERM=xterm-256color` env 설정(M8-7). DA/DA2 응답이 env와 일치해야 vim/tmux feature set이 일관. M11 detail design에서 최종 byte 형식 확인.
- **M15 Cmd+K 컨벤션**: Mac Terminal은 `Cmd+K = Clear to Start`(buffer + scrollback 모두 지움) 가능성 — `Cmd+K=buffer only / Cmd+Shift+K=scrollback only` 분리는 iTerm2 컨벤션일 수 있음. M15 진입 전 web 확인 후 사용자가 학습된 동작과 일치하도록 결정.
- **M18 탭 구현 방식**: 단일 창+자체 탭바(추천) / macOS native tabbing / 멀티 winit 창. 진입 시.
- **M19 GUI 여부**: config 파일만 / 별도 settings 창. 진입 시.
- **OSC 8 URL 스킴 whitelist**: http/https/mailto만? 진입 시.
- **Block selection / 자동 URL 검출**: 후속(MVP++) 또는 폐기. 보류.

---

## 14. 진입 시 web/docs.rs 재확인 항목

| 단계 | 항목 |
|---|---|
| M11 | xterm `ctlseqs` — ICH/DCH/IL/DL/ECH/DA/DSR byte 형식 + DEC special graphics 코드 0x6a~0x7e 매핑 + **DA/DA2 응답 레벨 컨벤션**(`xterm-256color` 후보) |
| M12 | `arboard` macOS 동작 + `objc2-app-kit` NSPasteboard 대안 + **winit 0.30 macOS Cmd → `ModifiersState::SUPER` 매핑 확인** (M12/M15/M18 단축키 전부 영향) |
| M13 | OSC 8 spec(gnome-terminal) + OSC 7 form (`file://host/path`) + OSC 133 (FinalTerm/iTerm 변종) + title stack(CSI 22/23 t) |
| M14 | xterm mouse 섹션 — 1000/1002/1003/1006 byte 형식 |
| M15 | wgpu atlas 텍스처 폐기 비용 (M5 실측) + **Mac Terminal vs iTerm2 Cmd+K 컨벤션** (Mac Terminal은 buffer+scrollback 모두 지우는 가능성, iTerm2는 분리) |
| M16 | overlay 텍스트 입력 컴포넌트 — winit IME off + 자체 캐럿 |
| M17 | Alacritty `grid::resize` 참고 |
| M18 | wgpu 단일 device + per-session state 패턴 |
| M19 | `notify` watcher debounce, toml schema 검증 |

---

## 15. 명시적 out-of-scope (미사용 결정)

학습 목표 + 단일 창/탭 가정 + 보안 정책상 다음은 **로드맵 외**:

| 항목 | 사유 |
|---|---|
| Sixel / Kitty graphics protocol | 별도 거대 단계. 학습 가치 큼지만 본 로드맵 범위 외. |
| Triggers / autoresponders (iTerm2) | 정규식 + 액션 디스패처 — 별도 시스템. |
| Hotkey window | macOS global hotkey 통합 — 별도. |
| Profile autoswitch | M19 + 추가 룰 엔진. |
| Print / Save text as file | 빈도 낮음. |
| Quick Look on selection | macOS 통합 깊음, 빈도 낮음. |
| OSC 52 (clipboard via terminal) | **보안 정책상 의도적 미지원**. |
| OSC 1337 (iTerm proprietary) | 비표준. |
| Sparkle update mechanism | 학습 프로젝트, 배포 가정 안 함. |
| Localization (i18n) | 학습 프로젝트. |
| Telemetry | 학습 프로젝트. |
| Tmux integration / split panes | 학습 프로젝트, M18 분할 창 미지원과 같은 결정. |
| 다중 G1/G2/G3 charset + SS2/SS3 | 거의 안 쓰임. M11 G0+LS0/LS1만. |
| DECSCNM (reverse video screen) | 거의 안 쓰임. |
| Color emoji 멀티 컬러 raster path | swash mono raster만. 발견 시 §16 cleanup으로. |

---

## 16. post-MVP+ cleanup (발견 시 fix)

영향 작거나 빈도 낮은 항목 — 별도 단계 안 만들고 발견 시 patch:

- **REP** (`CSI Pn b` repeat last char) — 일부 prompt 최적화.
- **CHT** (`CSI Pn I` cursor forward tabs).
- **HTS / TBC** (`ESC H` / `CSI g` set/clear tab stops) — 현재 8칸 hard-coded.
- **DECSCNM** reverse video screen.
- **DECRQM** request mode (앱이 mode 상태 query).
- **G1/G2/G3 charset + SS2/SS3 + LS2/LS3**.
- **Color emoji raster** (swash multi-color path).
- **Ligature 동작 검증** (D2Coding Ligature 자동 처리 여부).

각 항목 발견 시: 한 줄 issue 추가 → 영향 측정 → 5분~30분 patch.
