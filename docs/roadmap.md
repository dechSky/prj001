# pj001 — MVP+ 로드맵 (M11~M18)

**상태**: 로드맵 초안 (2026-05-08). 코드/세부 설계 미반영.
**목적**: M7까지 도달한 "표시·실행" 영역 위에, **Mac 터미널 일상 사용 격차**를 메우고 그 위로 확장.
**스타일**: high-level 1장. 각 단계 진입 직전 별도 `M??-design.md` 작성(M7 cursor / M8 keyboard와 동일 패턴).

---

## 0. 현재 상태 (2026-05-08)

| 영역 | 상태 |
|---|---|
| M1~M7 | 완료 — 창/PTY/렌더/색/alt screen/한글/IME/cursor 고도화 |
| **M8 (keyboard)** | **다른 세션 진행 중** — `docs/keyboard-design.md` |
| **M9 (modifier 인코딩 + DECPAM/DECPNM)** | **다른 세션 예약** — keyboard-design §10.1 |
| **M10 (bracketed paste + focus reporting)** | **다른 세션 예약** — keyboard-design §10.2 |
| M11~ | **본 로드맵 범위** |

본 로드맵은 M8~M10이 완료된 후의 작업을 다룬다. M10에서 노출될 `Term::bracketed_paste_active()` 등의 게이트는 M11 페이스트 처리에서 그대로 활용한다.

---

## 1. 전체 그림

| # | 제목 | 크기 | 일상 가치 | 학습 가치 | 의존 |
|---|---|---|---|---|---|
| **M11** | 마우스 인프라 + 선택 + 클립보드 | L | ★★★ | ★★ | M10 (bracketed paste flag) |
| **M12** | OSC bundle — 창 제목 + 하이퍼링크 | S~M | ★★ | ★★ | 독립 |
| **M13** | xterm mouse reporting (1000/1002/1003/1006) | M | ★★ | ★★ | M11 (마우스 좌표 인프라) |
| **M14** | 폰트 줌 단축키 (Cmd +/-/0) | S | ★★ | ★ | M8-1(modifier 인프라) — Cmd는 app-side라 M9 PTY encoding과 무관 |
| **M15** | Cmd+F find (scrollback 검색) | M | ★★ | ★★ | M11 (selection 렌더 재사용) |
| **M16** | resize reflow (정책 변경) | L | ★★ | ★★★ | 독립 (grid 모델 침습) |
| **M17** | 탭 / 멀티 창 | L | ★★ | ★★ | 아키텍처 변경 |
| **M18** | 환경설정 (config file → UI) | L | ★ | ★★ | M14, M17 (테마/단축키 분리) |

**굵직한 분기점**: M11(일상 사용 가능 시점) · M16(MVP 정책 변경) · M17(단일창 가정 깨짐).

**"Mac 터미널 평행선" 도달**: M11~M15 완료 시점.
**그 이상**: M16~M18.

---

## 2. M11. 마우스 + 선택 + 클립보드  [L]

**목표**: 드래그 선택 → Cmd+C → Cmd+V로 일상 작업 가능.

### 2.1 스코프 (in)

- **마우스 좌표 인프라** — `WindowEvent::MouseInput` / `CursorMoved` (휠은 M6-10 이미 있음). 픽셀 → cell `(row, col)` 변환. scrollback view offset, WIDE_CONT, alt screen 모두 정확.
- **선택 모델** — 1차: `Selection { anchor: (row, col), head: (row, col), kind: Char | Word | Line }`. row는 logical(scrollback 포함, 단조 증가) 좌표.
- **인터랙션** — left-down anchor / drag 확장 / left-up 확정. **double-click word**, **triple-click line**, **shift+click anchor 확장**.
- **렌더** — selection overlay를 별도 instance batch로. blend mode = source-over, 색 = `bg + alpha*0.3` 또는 시스템 selection 색.
- **클립보드** — `arboard` 크레이트. Cmd+C: selection을 텍스트로 직렬화(WIDE_CONT 스킵, line break 처리) → `Clipboard::set_text`. Cmd+V: `Clipboard::get_text` → PTY write. **bracketed paste 활성 시** `\e[200~` … `\e[201~` 래핑 (M10 노출 플래그 사용).
- **선택 vs mouse reporting 우선순위 정책**: M13 도입 후 reporting ON이면 raw 마우스는 reporting로 가고, **Shift 누르면 selection 모드**로 분리 (xterm/iTerm 표준).

### 2.2 스코프 제외

- **Block(box) selection** — Alt+drag. Mac 터미널은 지원, 우리는 1차 cut 제외 (M11+1 또는 future).
- **자동 URL 감지**(정규식 기반 hyperlink) — OSC 8(M12)과 분리. URL 검출은 M12+1.
- **drag-to-scroll** — 화면 위/아래로 드래그 넘기면 scrollback 자동 스크롤. 1차 cut 제외 (사용 시 휠로 대체).
- **wrapped line의 정확한 줄 합치기** — logical line 정보 없으므로 wrap 위치에 `\n` 그대로. M16에서 해결. §12 결정 참고.

### 2.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M11-1 | 마우스 좌표 인프라 + `Selection` 타입 + char-level drag | 로그로 `(row, col)` 정확 확인 |
| M11-2 | overlay 렌더 (selection 색 칠) | 시각 |
| M11-3 | double-click word / triple-click line / shift+click | 시각 |
| M11-4 | arboard + Cmd+C / Cmd+V (+ bracketed paste 래핑) | shell 라인에 paste 확인 |
| M11-5 | **file drag-and-drop** — `WindowEvent::DroppedFile` 받으면 shell-quoted path를 PTY write. bracketed paste 활성 시 래핑 동일 적용 | Finder에서 파일 끌어다 놓으면 path 입력 |

### 2.4 리스크 / 결정 포인트

- **`arboard` macOS 동작 확인** — 진입 시 web/docs.rs 재확인. 대안: `objc2_app_kit::NSPasteboard` 직접.
- **logical row 좌표계의 stability** — scrollback이 10k cap에 도달해 oldest row를 evict하면 raw index가 shift됨. heavy output 중 selection이 drift. **해결: `Term`에 `total_lines_emitted: u64` 단조 증가 카운터 추가, selection anchor는 이 카운터로 저장하고 render 시점에 `(scrollback_idx | grid_row)`로 resolve.** evict된 anchor는 selection cancel.
- **WIDE_CONT 직렬화** — Cell 두 개 차지하지만 텍스트는 한 글자.
- **trailing whitespace** — 선택 시 라인 끝 공백 제거할지 (Mac 터미널은 제거).
- **wrapped line 줄바꿈 처리** — M16(reflow) 도입 전이라 logical line 정보 없음. wrap된 라인을 copy하면 `\n`이 wrap 위치에 들어감. **수용**: M11에서 문서화, M16에서 logical line 모델 도입 시 자연 해결. (§12 결정 참고)
- **선택 ↔ scrollback view ↔ 렌더 race** — single snapshot 패턴(M8-1과 같은 lock-once) 적용.

### 2.5 cross-cut

- 마우스 좌표 인프라는 **M13 mouse reporting**과 직접 공유. 추상화 단위 정확히 잡기.
- selection 렌더 instance batch는 **M15 Cmd+F find 결과 highlight**에서 거의 그대로 재사용.

---

## 3. M12. OSC bundle — 창 제목 + 하이퍼링크  [S~M]

**목표**: htop/ssh가 창 제목 갱신, 클릭 가능한 URL 출력 지원.

### 3.1 스코프 (in)

- **`vte::Perform::osc_dispatch` 활성화** — 현재 무시 중. 첫 번째 인자(파라미터 ID) 분기.
- **OSC 0** (`set icon name + window title`) → `window.set_title(s)`.
- **OSC 1** (`icon name only`) → 무시 또는 디버그 로그.
- **OSC 2** (`window title only`) → `set_title(s)`.
- **OSC 8** (hyperlink) — `OSC 8 ; params ; URI ST`. ST는 `\e\\` 또는 `\x07`. 빈 URI는 hyperlink off.
  - `Term`에 `hyperlinks: HashMap<u32, String>` registry + 단조 증가 ID.
  - `Cell.hyperlink_id: Option<u32>` 추가. 현재 hover/active hyperlink 추적.
  - 렌더: hyperlink가 있는 cell은 항상 underline + (옵션) 색 변경.
  - **Cmd+hover** 시 cursor를 pointing-hand로 변경.
  - **Cmd+Click** → `open` 명령(macOS) 또는 `webbrowser` 크레이트.

### 3.2 스코프 제외

- **OSC 4 / 10 / 11** (color palette) — 환경설정(M18) 영역.
- **OSC 52** (clipboard via terminal) — 보안 이슈. 미지원 정책.
- **자동 URL 감지** — 정규식 기반. 별도 단계.

### 3.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M12-1 | OSC dispatch 인프라 + OSC 0/1/2 title | `printf '\e]2;hello\e\\'` |
| M12-2 | OSC 8 parse + Cell.hyperlink_id + underline 렌더 | `printf '\e]8;;https://example.com\e\\link\e]8;;\e\\'` |
| M12-3 | Cmd+hover cursor 변경 + Cmd+Click open | 클릭으로 브라우저 |

### 3.4 리스크

- **Hyperlink ID 메모리** — scrollback 10k rows × 평균 cell hyperlink 1개 = 10k entry. registry는 ID 재사용/GC 정책 필요. 1차 cut: hard cap (예: 4096 entry, LRU evict).
- **ST terminator 편차** — `\e\\` 표준이지만 BEL(`\x07`) 종료도 흔함. 둘 다 받아야 함 (vte가 처리할지 확인).
- **macOS open**: `Command::new("open").arg(&url)` — URL 검증(스킴 whitelist) 필요. 보안 결정 포인트.

---

## 4. M13. xterm mouse reporting  [M]

**목표**: vim/tmux/htop 안에서 마우스 클릭/드래그/휠 동작.

### 4.1 스코프 (in)

- **DECSET / DECRST** — 1000(button-event), 1002(button + drag motion), 1003(any-event motion), 1006(SGR encoding mode).
- **legacy 인코딩**: `CSI M Cb Cx Cy` (Cb/Cx/Cy = byte 32+value, 1-based).
- **SGR 인코딩** (1006): `CSI < button ; col ; row M` press, `m` release.
- button 인코딩: 0=left/1=middle/2=right + 32(motion) + 64(wheel up)/65(wheel down) + modifier bits 4/8/16.
- **selection 우선순위**: reporting OFF면 selection만, ON이면 raw → reporting / **Shift+raw → selection**.

### 4.2 스코프 제외

- **mode 1005** (UTF-8 인코딩) — deprecated, 안 함.
- **mode 1015** (urxvt 스타일) — deprecated, 안 함.
- **focus reporting** (1004) — M10 영역.

### 4.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M13-1 | DECSET 1000 + legacy 인코딩 (press/release) | tmux pane click |
| M13-2 | DECSET 1002 (drag motion) | vim visual mode |
| M13-3 | DECSET 1006 SGR encoding | iTerm parity |
| M13-4 | Shift+raw → selection 분기 | reporting ON 상태에서 Shift 누르고 드래그 = 우리 selection 동작 |

### 4.4 리스크

- **mode 1003**(any-event motion)은 keystroke 이벤트량 많음 — 성능 측정 필요.
- **휠 + alt screen** — vim/less 안에서 휠 = scroll 키 인코딩. 의외로 까다로움.
- 진입 시 web/spec 재확인: `xterm/ctlseqs` 문서 — mouse 섹션.

---

## 5. M14. 폰트 줌 단축키  [S]

**목표**: Cmd+= / Cmd+- / Cmd+0 으로 폰트 크기 변경.

### 5.1 스코프 (in)

- **AppState.font_size_pt: f32** + 변경 시 `font::measure_cell` 재호출 → cell metric 갱신.
- **atlas 재생성** — M5 DPI 변경 시 인프라(정책 C "atlas 폐기 후 재생성") 그대로 사용.
- **PTY size 재통보** — col/row가 cell 크기 변화로 달라지면 `MasterPty::resize`.
- **키 매핑** — Cmd 모디파이어. **M8-1**에서 도입될 `ModifiersState` 인프라(keyboard-design §6.1) 활용. Cmd는 app-side 처리라 PTY로 안 보냄 — M9(PTY encoding)와 무관.
- **clamp**: 6pt ~ 72pt. 기본 14pt.

### 5.2 스코프 제외

- 폰트 패밀리 변경 — M18 환경설정 영역.

### 5.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M14-1 | font_size_pt 필드 + reflow 함수 + 키 매핑 | Cmd+= 시 글자 커짐 |
| M14-2 | PTY size 재통보 검증 | shell이 새 col/row 인식 |

### 5.4 리스크

- atlas 폐기 후 글리프 다시 raster — 100ms 미만 예상 (실측 필요).
- font_size 변경 중 PTY 출력 race — atlas 재생성 동안 pending text bounce.

---

## 6. M15. Cmd+F find  [M]

**목표**: scrollback + 현재 grid 텍스트 검색 + 결과 highlight + 점프.

### 6.1 스코프 (in)

- **검색 입력 UI** — 창 우상단(또는 하단) overlay. 별도 IME 비활성 입력 라인. Esc/Enter로 닫음.
- **검색 알고리즘** — literal substring. case-insensitive 토글.
- **검색 대상** — scrollback `VecDeque<Vec<Cell>>` + main grid. Cell → char로 풀어내기 (WIDE_CONT 스킵).
- **결과 highlight** — M11 selection overlay batch 재사용 (다른 색).
- **n / N (Enter / Shift+Enter)** — 다음/이전 결과로 view_offset 점프.

### 6.2 스코프 제외

- **regex** — 1차 cut literal만.
- **검색 결과 export / 고정** — 후속.

### 6.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M15-1 | overlay 입력 라인 (별도 텍스트 컴포넌트) | 입력 가능 |
| M15-2 | substring search + 결과 highlight | 일치 cell 강조 |
| M15-3 | Enter/Shift+Enter 점프 + view_offset 자동 조정 | 커서 따라감 |

### 6.4 리스크

- **검색 입력 라인은 우리의 첫 in-app 텍스트 입력 컴포넌트** — IME와 충돌하지 않도록 (`set_ime_allowed(false)` 동안만).
- 검색 결과량 — 큰 scrollback에서 수천 매치 가능. lazy iteration.

---

## 7. M16. resize reflow  [L]

**목표**: 가로 리사이즈 시 wrap된 line이 새로운 폭에 맞춰 재배치 (정책 변경: truncate → reflow).

### 7.1 스코프 (in)

- **logical line 모델** — 각 cell row에 `wrap_continuation: bool` 비트 또는 line-end 마커. main grid + scrollback 동일 구조.
- **resize 핸들링** — 이전 폭에서 logical line 추출 → 새 폭으로 wrap → grid 재구성.
- **cursor 위치 보존** — logical line + char index → 새 (row, col).
- **alt screen은 reflow 안 함** — vim 등 자체 처리. flag로 분기.

### 7.2 스코프 제외

- **세로 리사이즈에서 scrollback expansion** — 1차 cut 제외 (행 수만 늘림).
- **double-width grapheme cluster reflow** — wide 글자 폭 1 cell만 남았을 때 처리. 1차 cut: 줄 끝에 다음 줄로 밀음.

### 7.3 분해

| 단계 | 내용 | 검증 |
|---|---|---|
| M16-1 | logical line 추출/재조립 코어 (헤드리스 unit test) | grid 모듈 단독 테스트 |
| M16-2 | resize hook 통합 (main + scrollback) | 시각: 긴 라인이 잘렸다 펼쳐짐 |
| M16-3 | cursor 좌표 보존 | shell prompt 위치 유지 |

### 7.4 리스크

- **Alacritty가 한참 걸린 영역** — 학습 가치는 큼, 시간도 큼.
- **현재 정책 F**: scrollback view에서 col mismatch는 truncate-on-read. M16 채택 시 정책 갱신 필요.
- **선택 좌표 무효화** — resize 중 진행 중인 selection은 cancel.

---

## 8. M17. 탭 / 멀티 창  [L]

**목표**: Cmd+T 새 탭, Cmd+1~9 전환, Cmd+W 닫기. (대안: 멀티 창)

### 8.1 결정 포인트 (진입 시 사용자 확인)

- **단일 창 + 탭바 자체 그림** vs **macOS native NSWindow tabbing** vs **멀티 winit 창**.
- 학습 측면 추천: **단일 창 + 탭바 자체 그림** (winit 단일 창 가정 유지, 렌더 인프라 재사용).

### 8.2 스코프 (in, 단일 창 + 탭바 가정)

- **`Session`** — `{ pty: Pty, term: Arc<Mutex<Term>>, reader_thread: JoinHandle }` 묶음.
- **AppState.sessions: Vec<Session>** + `active: usize`.
- **렌더** — 활성 세션 grid만 그림 + 상단 탭바 (cell row 하나 점유 또는 별도 영역).
- **단축키** — Cmd+T (새 세션 spawn) / Cmd+1~9 / Cmd+W / Cmd+Shift+] / Cmd+Shift+[.
- **PTY 리더 thread × N** — 각 세션 별 thread. EventLoopProxy로 `RepaintRequested(session_idx)` 통지.

### 8.3 스코프 제외

- **분할 창 (split)** — tmux 영역. 미지원.
- **세션 dump/restore** — 후속.

### 8.4 리스크

- **wgpu 자원 단일 device 유지** — atlas/pipeline은 모든 세션 공유. surface는 1.
- **활성 세션 전환 시 스크롤백 view_offset 등 per-session 상태**.
- **탭바 클릭** — M11 마우스 인프라 재사용 (탭바 영역 hit-test).

---

## 9. M18. 환경설정  [L]

**목표**: 폰트/색/단축키 사용자 설정.

### 9.1 분해

| 단계 | 내용 |
|---|---|
| **M18-1 config file** | `~/.config/pj001/config.toml` — `font.{family, size}`, `color_scheme.*`(16색 + bg/fg + cursor + selection), `keybindings.*` |
| **M18-2 hot-reload** | `notify` 크레이트로 watcher. 변경 시 atlas 재생성/색 갱신 |
| **M18-3 Cmd+, settings GUI** (선택) | 별도 창 또는 in-app overlay. 1차 cut: 파일 편집만 |

### 9.2 리스크

- **schema 검증** — 잘못된 toml에서 에러 메시지.
- **단축키 충돌** — 매핑 검증 (Cmd+T를 두 곳에 binding 등).
- **GUI 빌드 시 wgpu 멀티 surface** — M17와 같은 영역.

---

## 10. cross-cutting / 인프라 변경 추적

| 인프라 | 도입 단계 | 재사용 단계 |
|---|---|---|
| 마우스 좌표 hit-test | M11 | M13, M17(탭바), M12-3(Cmd+Click) |
| Selection overlay batch | M11 | M15 (find highlight) |
| atlas 재생성 (DPI/font size) | M5 (정책 C) → M14 확장 | M18-1 |
| logical line 모델 | M16 | M11(scrollback 직렬화), M15(검색) — **M16를 앞당기는 게 깔끔할 수도** |
| OSC dispatch 인프라 | M12 | 미래 OSC 4/10/11 등 |
| modifier 인프라 | M9 (다른 세션) | M11 (Shift+Click), M13 (Shift+raw → selection), M14 (Cmd+=) |

**진행 중 재고려 포인트**: M16 logical line 모델을 M11 직렬화 시점에 미리 도입할지(선택 텍스트 줄바꿈 처리 일관). 진입 직전 advisor 검토.

---

## 11. 진행 규칙

1. **각 M??-design.md 작성**을 코드 진입 직전에 — M7 cursor / M8 keyboard 패턴 그대로.
2. **advisor 게이트** — design.md 작성 후 advisor 호출, 응답 반영 후 코드.
3. **시각 검증 + cargo test 자동** — 둘 다. 학습 프로젝트지만 회귀 방지 우선.
4. **CLAUDE.md 환각 방지 4원칙 유지** — spec 인용 시 출처 표기, 진입 시 docs.rs/spec 재확인.
5. **다른 세션 충돌 회피** — 같은 파일 동시 편집 금지. 진입 전 `git status` + 다른 세션과 합의.

---

## 12. 결정 / 미해결

### 12.1 결정 (advisor 검토 반영)

- **M16 vs M11 순서** — **M11 먼저, wrapped line 줄바꿈 한계 문서화로 진행** 결정. 이유:
  - 일상 가용성 이득(선택/복사/붙여넣기)을 먼저 확보.
  - "wrap 위치에 `\n`" 한계는 다수 use case에서 수용 가능(짧은 라인 / shell 명령 paste).
  - M16(logical line 모델 도입) 시점에 자연 해결 — copy 로직이 logical line 단위로 변경.
  - 대안(M16 먼저)은 큰 grid 모델 변경을 user-facing 개선 없이 선행 — 학습 동기 측면 비추천.
- **selection vs mouse reporting (M13)** — xterm/iTerm 표준 따름: reporting ON이면 raw → reporting, **Shift+raw → selection**.

### 12.2 미해결 (사용자 결정 필요)

- **M17 탭 구현 방식**: 단일 창+자체 탭바(추천, 학습 가치 큼) / macOS native tabbing / 멀티 winit 창. 진입 시 결정.
- **M18 GUI 여부**: config 파일만으로 끝낼지, 별도 settings 창까지 갈지. 진입 시 결정.
- **Block selection / 자동 URL 검출**: 후속(MVP++) 또는 폐기. 보류.

---

## 13. 진입 시 web/docs.rs 재확인 항목

| 단계 | 항목 |
|---|---|
| M11 | `arboard` macOS 동작 + `objc2-app-kit` NSPasteboard 대안 |
| M12 | OSC 8 spec (gnome-terminal 표준 문서) + OSC 0/1/2 byte sequence |
| M13 | xterm `ctlseqs` mouse 섹션 — 1000/1002/1003/1006 byte 형식 |
| M14 | wgpu atlas 텍스처 폐기 비용 (M5 실측) |
| M15 | overlay 텍스트 입력 컴포넌트 — winit IME off + 자체 캐럿 |
| M16 | Alacritty 0.x reflow 구현 참고 (`grid::resize`) |
| M17 | wgpu 단일 device + per-session state 추적 패턴 |
| M18 | `notify` watcher debounce, toml schema 검증 |
