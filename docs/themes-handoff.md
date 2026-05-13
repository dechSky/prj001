# pj001 themes — Claude Design handoff 매핑

**상태**: 추출본, 2026-05-13.
**출처**: `claude.ai/design` handoff bundle (Derek가 2026-05-13 07:45 UTC에 시작한 "터미널 UI 디자인" 프로젝트). 압축해제 위치 `/tmp/pj001-design-handoff/remix/`. 6 React variants + 공유 모델 + Tweaks 패널.
**목적**: handoff 디자인을 pj001 (Rust + wgpu/winit, macOS 네이티브)에 어디까지 옮길 수 있는지 매핑. 구현 슬라이스 결정 근거.
**범위 합의**: Derek가 "전체 야심차게 — theme + tabs + blocks + glass + floating" 선택. 본 문서는 토큰만 정리하며 코드 구현은 별도 milestone.

---

## 1. 핵심 모델 격차

handoff는 **command block 모델**(Warp/Wave 식 — 각 명령을 카드로 wrap, status/duration/branch metadata 포함)을 전제로 모든 6 테마가 그려진다. pj001은 현재 raw VT cell grid이고 명령 경계 정보가 없다.

| handoff 가정 | pj001 현실 | 갭 메우는 방법 |
|---|---|---|
| Block에 `cmd`, `cwd`, `branch`, `status`, `durationMs`, `out: [[role, text]]` | raw VT cell grid, 명령 경계 없음 | **OSC 133** (FinalTerm / iTerm semantic prompt). `\e]133;A\e\\` prompt start, `\e]133;C\e\\` command start, `\e]133;D;<exit>\e\\` end. shell이 prompt에 emit해야 함 |
| Color role: `plain` / `gray` / `accent` / `ok` / `red` / `ai-prompt` | ANSI 16/256/RGB SGR + Default | role → SGR 매핑 테이블 (예: `accent` = SGR 34 cyan에 매핑, theme이 색만 교체). 또는 OSC 133 안에서 별도 attr 전달 |
| Tree split: `{ dir: 'h'\|'v', children: [...] }` | M13 BSP layout tree (이미 완료) | **거의 일치**. dir 명명만 통일 (handoff: h/v, pj001: 확인 필요) |
| Tab: `{ id, title, cwd, panes, root, activePane }` | M14 Tabs (완료) | **거의 일치**. cwd는 OSC 7로 받으면 됨 (M13-2/M8-7 인프라 존재) |
| Pane: `{ id, cwd, branch, blocks, input }` | M12 Session (PTY + visible slot) | `branch`는 OSC 133 D 또는 starship 같은 prompt에서. `blocks`는 위 OSC 133 산물 |
| Detached floating window | 단일 winit 창 | **추가 milestone** (M??-multi-window). winit `WindowAttributes` 추가 인스턴스 |
| Backdrop blur + saturate | 평면 quad fragment | wgpu blur pass (gaussian) + saturate matrix |
| 한글 fallback (Gowun Batang / Pretendard) | cosmic-text 시스템 폰트 위임 (M6-7에서 D2CodingLigatureNFM로 검증) | profile에 fallback chain 명시 추가 |

---

## 2. 디자인 토큰 — 6 테마

각 테마는 (1) bg layer (2) 6 color roles (3) chrome/tab/pane/block/input style (4) accent palette 4종 (5) 권장 mono font.

### 2.1 공통 token 구조

```rust
struct ThemeTokens {
    name: &str,                // "aurora" | "obsidian" | "vellum" | "holo" | "bento" | "crystal"
    label: &str,               // "01 · Aurora Glass"
    blurb: &str,               // "라이트 글래스 · 소프트 오로라 그라데이션"
    mode: ThemeMode,           // Light | Dark
    bg: BackgroundLayer,       // 그라디언트 stops + 패턴
    colors: Roles,             // plain, gray, accent, ok, red, ai_prompt
    accents: [Color; 4],       // accent picker
    fonts: FontStack {
        mono_primary, mono_fallback,
        ui_sans, ui_serif_ko_ornamental (vellum only)
    },
    chrome: WindowChrome,      // 트래픽 라이트 색/border, 검색 chip 스타일
    tabs: TabStyle,            // Pill | Folder | Underline | PillInBar
    pane: PaneSurface {
        bg, border_active, border_inactive, blur_px, saturate,
        border_radius, shadow_active, shadow_inactive
    },
    block: BlockSurface {
        bg, border, border_radius, prompt_marker_kind, separator,
        ordinal_marker (vellum №01 case), status_kind
    },
    input: InputBar { bg, border_top, prompt_marker_size, prompt_marker_kind },
    scrollbar: ScrollbarStyle,
}
```

### 2.2 테마별 raw token

#### 01 · Aurora Glass — light, soft pastel
| 항목 | 값 |
|---|---|
| mode | Light |
| bg | radial(`#ffe1c4` @18%22%, transparent 45%) + radial(`#d4c6ff` @78%18%, transparent 50%) + radial(`#c7f0d9` @60%92%, transparent 48%) + linear(#f6f2eb→#efe9e0) |
| plain | `#2b2734` |
| gray | rgba(43,39,52,0.5) |
| accent | `#7a5af8` (violet) |
| accent gradient end | `#ec7be0` (prompt marker) |
| ok | `#1f9d6b` |
| red | `#e0427a` |
| ai_prompt | `#7a5af8` |
| mono | JetBrains Mono |
| ui sans | Pretendard / Inter |
| tabs | Pill, frosted bg, gradient dot indicator |
| pane bg | rgba(255,255,255,0.35), blur 24px, saturate 1.6 |
| pane border active | rgba(122,90,248,0.4) + glow 2px |
| block bg | rgba(255,255,255,0.55), blur 20px, saturate 1.4 |
| block radius | 10px |
| prompt marker | 16×16 rounded square, gradient `accent→#ec7be0` |
| accents | `[#7a5af8/#ec7be0]`, `[#1f9d6b/#5ed79f]`, `[#e07a3a/#f4a13b]`, `[#3b7be0/#5aacff]` |
| scrollbar | rgba(122,90,248,0.22) thin |

#### 02 · Obsidian Glass — dark void + neon
| 항목 | 값 |
|---|---|
| mode | Dark |
| bg | radial(`#2a1f5a`, `#103355`, `#1b0e3a`) + linear(`#07091a`→`#03050f`) + dot pattern rgba(255,255,255,0.05) 32px |
| plain | `#e8e7ef` |
| gray | rgba(232,231,239,0.42) |
| accent | `#3edaff` (cyan neon) |
| ok | `#5dffaa` |
| red | `#ff6b88` |
| ai_prompt | `#c084fc` (purple) |
| mono | Geist Mono |
| ui sans | Geist / Pretendard |
| tabs | Pill, neon glow accent dot |
| pane bg | linear(rgba(20,24,42,0.85)→rgba(14,16,30,0.85)), blur 28px, saturate 1.4 |
| pane border active | `accent`66 + shadow `0 0 24px -8px accent55` (neon glow) |
| block bg | linear(rgba(255,255,255,0.04)→rgba(255,255,255,0.015)) |
| block radius | 8px |
| prompt marker | 16×16 rounded square, gradient `accent→#c084fc`, glow `0 0 8px accent80` |
| accents | `#3edaff`, `#c084fc`, `#5dffaa`, `#ff7ea0` |
| scrollbar | rgba(62,218,255,0.28) thin w/ inset cyan glow |

#### 03 · Vellum — warm paper, ink red
| 항목 | 값 |
|---|---|
| mode | Light |
| bg | linear(`#efe7d2`→`#e7dec5`) + paper noise SVG turbulence + radial paper grain |
| plain | `#2a2419` |
| gray | rgba(42,36,25,0.5) |
| accent | `#b8451f` (ink red) |
| ok | `#1f7a4a` |
| red | `#c43a3a` |
| ai_prompt | `#7a5f1a` |
| mono | IBM Plex Mono |
| ui sans | Pretendard |
| ui serif (한글 ornamental) | Gowun Batang, Noto Serif KR |
| ui serif (영문 ornamental) | Instrument Serif |
| tabs | Underline only, 2px accent bottom border on active |
| pane bg | linear(`#faf5e9`→`#f5efde`), no blur |
| pane border active | `accent` solid |
| pane border inactive | rgba(120,90,40,0.22) |
| block bg | linear(`#fdfaf2`→`#f9f4e8`) |
| block radius | 3px (sharp paper feel) |
| block separator | 1px dashed rgba(120,90,40,0.2) |
| ordinal marker | `№01`, `№02` mono 9.5px rgba(42,36,25,0.32), left margin -24px |
| prompt marker | `$` glyph in accent color (no shape) |
| traffic lights | `#c97b54`, `#d9a45a`, `#83a468` (muted paper-tinted) |
| accents | `#b8451f`, `#1f7a4a`, `#7a5f1a`, `#3a4a7a` |
| scrollbar | rgba(184,69,31,0.3), 8px, sharp corners |
| 폰트 정책 | 기능 텍스트(탭/라벨/식별자) → mono. ornamental(빈 상태 "새 페이지", 보조 텍스트 "· 2 panes") → 한글 serif |

#### 04 · Holo Prism — iridescent dichroic
| 항목 | 값 |
|---|---|
| mode | Dark |
| bg | radial(`#2c1a5e`, `#1a3d6e`, `#5a1a4a`) + linear(`#0a081e`→`#060414`) + dot pattern 40px |
| plain | `#ebe8f6` |
| gray | rgba(235,232,246,0.4) |
| accent | `#a0f0ff` (default) — actually 4-stop choice |
| ok | `#7dffb0` |
| red | `#ff8aa6` |
| ai_prompt | `#ffcfa3` |
| mono | Geist Mono |
| ui sans | Geist |
| **prism gradient** (signature) | `linear-gradient(125deg, #a0f0ff 0%, #c3a5ff 20%, #ff9fd6 40%, #ffd29a 60%, #c3ffa5 80%, #a0f0ff 100%)` |
| tabs | Pill (border-radius 999), prism gradient border on active (mask composite) |
| pane bg | linear(rgba(16,14,32,0.85)→rgba(10,8,24,0.85)), blur 28px, saturate 1.5 |
| pane border | prism gradient via WebKitMaskComposite (1.5px active / 1px inactive, opacity 0.9 / 0.3) |
| block bg | linear(rgba(20,18,40,0.92)→rgba(14,12,30,0.92)) |
| block radius | 9px |
| **prompt marker** | hex shape `polygon(25% 5%, 75% 5%, 100% 50%, 75% 95%, 25% 95%, 0 50%)` with prism gradient outer + dark inner |
| traffic lights | gradient pink/orange/green (linear-gradient 135deg) |
| accents | `#c3a5ff`, `#a0f0ff`, `#ffcfa3`, `#c3ffa5` |
| scrollbar | prism gradient thumb |

#### 05 · Bento Pop — chunky 3D blocks, neobrutalism
| 항목 | 값 |
|---|---|
| mode | Light |
| bg | `#e9dfc1` + dot pattern rgba(31,26,20,0.15) 20px |
| plain | `#1f1a14` |
| gray | rgba(31,26,20,0.55) |
| accent | `#2563eb` (saturated blue) |
| ok | `#10894b` |
| red | `#d83a3a` |
| ai_prompt | `#8044d6` |
| mono | Berkeley Mono |
| ui sans | Pretendard |
| tabs | Folder tabs (rounded top, flat bottom, 1.5px solid `#1f1a14` border) |
| pane bg | `#faf2dd` |
| pane border | 1.5px solid `#1f1a14` |
| pane shadow active | `3px 4px 0 rgba(31,26,20,0.9)` (hard offset, no blur) |
| pane header | bg `#1f1a14` (black), text `#faf2dd` |
| block bg | `#fffdf6` |
| block border | 1px solid `#1f1a14` |
| block radius | 7px |
| block left stripe | 5px wide, color = status (ok=green, run=accent, err=red) |
| block output bg | `#f4ede0` (inset) |
| block shadow | `2px 3px 0 rgba(31,26,20,0.85)` + inset white top |
| prompt marker | `RUN` black chip badge, mono 9.5px bold letter-spacing 0.4 |
| traffic lights | pure `#ff5f57`/`#febc2e`/`#28c840` + 1px solid `#1f1a14` border |
| accents | `#2563eb`, `#d83a3a`, `#10894b`, `#8044d6` |
| scrollbar | 12px chunky `#1f1a14` thumb on `#faf2dd` track |

#### 06 · Crystal — heavy refraction, depth
| 항목 | 값 |
|---|---|
| mode | Dark |
| bg | radial(`#1f5b6e`, `#3a1f6e`, `#1f3a5a`) + linear(`#0a0f24`→`#050817`) + 2 blur(40px) bloom layers (top-left teal, bottom-right purple) |
| plain | `#e6e8f4` |
| gray | rgba(230,232,244,0.45) |
| accent | `#5ed2c5` (teal) |
| ok | `#7ee8b0` |
| red | `#ff7ea0` |
| ai_prompt | `#cfa9ff` |
| mono | Geist Mono |
| ui sans | Geist |
| tabs | Rounded rect inside bordered "pill bar" container |
| pane bg | linear(rgba(255,255,255,0.085)→rgba(255,255,255,0.02)@35%→rgba(255,255,255,0.03)) + blur 36px + saturate 1.7 |
| pane radius | 14px |
| pane shadow active | multi-layer: inset top white, inset bottom teal bloom, outer teal glow + drop shadow |
| block bg | linear(rgba(255,255,255,0.06)→rgba(255,255,255,0.015)) |
| block radius | 10px |
| **prompt marker** | radial bubble `radial-gradient(circle at 35% 30%, rgba(255,255,255,0.7) 0%, rgba(94,210,197,0.7) 30%, rgba(207,169,255,0.5) 70%)` + specular highlight (white blur dot at top-left) + teal glow |
| traffic lights | radial-gradient 3D bubble (highlight + shadow inset) |
| accents | `#5ed2c5`, `#cfa9ff`, `#ffd29a`, `#ff7ea0` |
| scrollbar | gradient teal→purple |

---

## 3. pj001 매핑 — 구성요소별

### 3.1 Config schema (`~/.config/pj001/config.toml`)

`docs/settings-and-context-menu-plan.md` §2.1 schema를 확장 — themes 섹션을 6 preset으로 채움.

```toml
[general]
theme = "obsidian"           # active preset name
accent = "auto"              # "auto" or hex from theme.accents

[profile.aurora]
mode = "light"
mono = "JetBrains Mono"
mono_fallback = ["ui-monospace", "monospace"]
[profile.aurora.colors]
plain = "#2b2734"; gray = "rgba(43,39,52,0.5)"
accent = "#7a5af8"; ok = "#1f9d6b"; red = "#e0427a"; ai_prompt = "#7a5af8"
[profile.aurora.bg]
kind = "radial-gradient"
stops = [
  { at = "18% 22%", color = "#ffe1c4", to = 45 },
  { at = "78% 18%", color = "#d4c6ff", to = 50 },
  { at = "60% 92%", color = "#c7f0d9", to = 48 },
]
base = "linear-gradient(180deg, #f6f2eb 0%, #efe9e0 100%)"
[profile.aurora.surface]
pane_bg = "rgba(255,255,255,0.35)"; pane_blur = 24; pane_saturate = 1.6
block_bg = "rgba(255,255,255,0.55)"; block_blur = 20; block_saturate = 1.4
[profile.aurora.accents]
options = [
  ["#7a5af8", "#ec7be0"], ["#1f9d6b", "#5ed79f"],
  ["#e07a3a", "#f4a13b"], ["#3b7be0", "#5aacff"],
]
```

(나머지 5 테마 동일 구조. obsidian / vellum / holo / bento / crystal.)

### 3.2 Render layer 매핑

| 디자인 요소 | pj001 코드 위치 | 신규 vs 수정 | 비고 |
|---|---|---|---|
| BG radial/linear gradient | `crates/pj001/src/render/` — 새 background pass | **신규** | wgpu fullscreen quad + multi-stop gradient shader. radial은 distance-from-point 기반. paper noise는 텍스처 |
| Backdrop blur (pane / block) | render pass | **신규** | wgpu 2-pass gaussian (downsample → blur → composite). Aurora/Obsidian/Holo/Crystal에 필수 |
| Saturate matrix | shader | 신규 | 4x4 color matrix, 보통 1.4~1.7 |
| ANSI 16/256/RGB SGR | `crates/pj001-core/src/vt/` 이미 있음 | **테마 정의만** | role을 SGR로 매핑하거나 별도 color stack |
| 6 role color (plain/gray/accent/ok/red/ai_prompt) | `crates/pj001/src/render/font.rs` color resolver | **수정** | profile에서 lookup |
| Mono 폰트 fallback | M5에서 cosmic-text FontStack | **확장** | `mono_primary` → `mono_fallback` → ui-monospace. profile마다 다름 |
| Cell metrics (font_size, line_height) | M5 measure_cell | 그대로 | profile.font_size 14 기본 |
| Tab bar 시각 | M14 코드 (`d3f8dcc`, `f246e07`, `aceb129`) | **수정** | 6 variant 분기. Pill / Folder / Underline / PillInBar 4종 + 테마 색 |
| Tab close 버튼 | M14 코드 | **수정 (작음)** | 색만 |
| 새 탭 + 버튼 | M14 코드 | **수정 (작음)** | shape/색 |
| Split divider | M13 BSP layout | **수정** | gap 7-10px, divider 색 = theme.pane.border_inactive |
| Pane header (cwd · branch · split/close) | **신규** | **신규** | 현재 pj001 pane은 header 없음 (raw VT). 28px minHeight bar 추가. OSC 7 (cwd) + OSC 133 D (exit) + starship/git env (branch) 필요 |
| Block 카드 렌더 | **신규** | **OSC 133 선행 필수** | grid 별도 그룹화 모드. M13-4 영역 |
| Prompt marker (rounded square / `$` / hex / RUN chip / bubble) | **신규** | **신규** | 16-18px, theme별 shape. SVG path 또는 quad+shader |
| Block status badge (✓ 184ms) | **신규** | **신규** | OSC 133 D 종료 코드 + duration timer |
| Block 좌측 stripe (Bento) | **신규** | **신규** | 5px column, status 색 |
| Block ordinal (`№01`, Vellum) | **신규** | **신규** | 카운터 + left margin 텍스트 |
| Input row (prompt + 텍스트) | 현재 PTY 직접 입력 | **시각만** | 별도 input row는 안 만듦. cursor 위치를 prompt marker로 그리는 정도 |
| Window chrome (traffic lights + title) | macOS native (winit) | **수정 (큼)** | 현재 native title bar 사용 — handoff는 custom chrome. winit `with_decorations(false)` + 자체 그리기. 또는 native 유지하고 chrome 흉내는 포기 |
| 검색 chip (⌘K) | **신규** | P1 (`Cmd+F` find) | M16에 묶음 |
| Floating window | **신규** | **신규 — multi-window milestone** | winit `EventLoop::create_window()` 추가, 자체 z-order, drag handle. 그러나 macOS native 분리창과 충돌 — design 결정 필요 |
| Scrollbar 테마 | 현재 native | **수정 (중간)** | 자체 그리기 — minimap-style 보조 vs OS native? |
| 한글 폰트 fallback (Gowun Batang 등) | M6-7 cosmic-text 시스템 위임 | **수정** | profile에 명시 fallback chain |

### 3.3 Tab style 4종 분류

| Style | 사용 테마 | 구현 |
|---|---|---|
| Pill (frosted) | aurora | borderRadius 8, blur bg |
| Pill (neon) | obsidian | borderRadius 7, accent border + glow |
| Underline | vellum | 하단 2px solid accent on active, 나머지 transparent |
| Pill (round) + prism border | holo | borderRadius 999, prism gradient via mask |
| Folder | bento | 상단 모서리만 둥근, 1.5px solid border |
| Pill-in-bar | crystal | bordered rounded rect container, 내부 rounded rect 탭 |

Tab style은 **6개 모두 미세히 달라서** 한 abstract `TabStyle` enum + 6 variant 분기 또는 6 shader/path. 토큰만 다르게 한 단일 path는 부족.

### 3.4 Prompt marker 6종

| Theme | Shape | 크기 | 효과 |
|---|---|---|---|
| Aurora | rounded square w/ gradient | 16-20px | accent→pink gradient + drop shadow |
| Obsidian | rounded square w/ gradient | 16-20px | accent→purple gradient + neon glow |
| Vellum | `$` glyph (텍스트) | 12.5-13px | accent색, 폰트 |
| Holo | hexagon (clip-path) | 17-44px | prism gradient outer + dark inner + optional bright glow |
| Bento | `RUN` chip | h 1px 6px | 검은 bg + 흰 텍스트 chip |
| Crystal | radial bubble (3D orb) | 18-22px | radial gradient + specular highlight + glow |

Shader 1개 + shape uniform으로 묶을 수 있음. Holo hexagon은 SDF(signed distance field) clip 권장.

---

## 4. 구현 가능성 분류

### 4.1 ★ Direct (config + 기존 render 코드 수정만)
- 6 테마 color tokens → `profile.<theme>.colors` 6 role
- Mono 폰트 (M5 fallback chain 확장)
- ANSI 16/256 색을 테마 accent에 맞춰 재정의
- Tab 색 (M14 코드 분기)
- Split divider gap + 색 (M13 코드)
- Scrollbar 색 (자체 그리기 시작)

### 4.2 ★★ Medium (신규 render pass / shader)
- Background radial/linear gradient (fullscreen quad shader)
- Pane backdrop blur (gaussian 2-pass)
- Saturate matrix
- Prompt marker shader (6 shape SDF)
- Block 카드 bg + border-radius + shadow
- Pane header bar (28px, cwd · branch · 버튼)

### 4.3 ★★★ Hard (인프라 / 단계 선행 필요)
- **명령 블록 그룹화** — OSC 133 prompt/command/end 처리, shell side prompt 설정 가이드 (P0 보안 트랙과 묶음)
- **`branch` metadata** — git 정보 어디서? starship 같은 prompt가 OSC133 D에 emit하거나 우리가 cwd watcher로 별도 조회 (성능)
- **Multi-window 분리창** — winit multi-window + 자체 z-order + drag handle. macOS native 분리 vs 자체 vs 포기
- **Custom window chrome** — `with_decorations(false)` + traffic lights + drag region. Apple HIG 충돌 가능, 접근성 영향
- **Paper noise (Vellum)** — fractalNoise SVG filter는 wgpu에서 텍스처 미리 생성

### 4.4 ✗ Drop / 변형
- React block animations (running → ok 전환 페이드) — pj001은 한 frame redraw, 페이드 효과는 별도 timeline
- `⌘K` 검색 chip — M16 find와 분리. 시각만 흉내는 안 만드는 게 나음
- Inline AI 응답 (handoff `runFakeCommand` `ai` 패턴) — pj001은 host terminal, AI는 별도 agent process (M15)
- Quick start welcome 카드 — pj001 첫 실행 시 useful하지만 핵심 아님. M??-welcome으로 분리
- Floating window의 React handle — pj001 native window는 macOS가 chrome 그림. 별도 chrome 그리려면 4.3 항목과 묶음

---

## 5. 구현 슬라이스 (제안)

전체 야심차게 가는 경로 — 단계 분해해야 함. 각 단계 끝에서 사용자 가시 확인.

### Phase 1 — 토큰 인프라 (1~2주)
- `pj001-core` 또는 별도 `pj001-theme` crate
- `~/.config/pj001/config.toml` parser (TOML → ThemeTokens)
- 6 preset 빌트인 (위 §2.2 그대로 코딩)
- ANSI 16 color → role 매핑 (theme별)
- Cell render에서 role color 적용
- Tab bar M14 코드에 theme 색 주입
- Split divider gap + 색
- `accent` switcher (4 options)
- `mono` font fallback chain
- **체크포인트**: 6 테마 색이 ANSI / 탭 / divider에 정상 적용. 빌드/테스트 회귀 없음.

### Phase 2 — Background + 시각 chrome (2~3주)
- wgpu background pass: radial/linear gradient shader
- Vellum paper noise 텍스처 생성 (offline tool 또는 빌드 타임)
- Holo/Crystal bg multi-layer + bloom blur
- Native title bar 유지 결정 vs custom chrome 결정 (Derek 필요)
- **체크포인트**: 6 테마 bg가 fullscreen으로 보임. 텍스트 가독성 OK.

### Phase 3 — Pane surface (2~3주)
- wgpu backdrop blur pass (2-pass gaussian downsample/upsample)
- Saturate matrix
- Pane border + shadow (theme-specific)
- Pane header bar (28px) — 일단 cwd만 (OSC 7 기존)
- Split focus 표시 (active 강조)
- **체크포인트**: 6 테마 pane이 시각적으로 일치.

### Phase 4 — OSC 133 + Block 모델 (3~4주, **가장 큼**)
- `vte` parser OSC 133 분기 (`A`/`B`/`C`/`D` 4가지)
- pj001-core grid에 BlockStream 별도 모델 — VT cell grid은 그대로 두고 별도 metadata track 추가
- Block 시작/끝 boundary cell index 저장
- Block render mode toggle: classic raw VT (현재) vs block 카드. profile `block_mode = "auto" | "off"` 옵션
- Auto는 OSC 133 detected 시 자동 ON, else raw
- Block 카드 렌더 (theme별 bg/border/radius/shadow)
- Prompt marker 6 shape shader (SDF 기반)
- Status badge (✓/× + ms) — exit code + duration timer
- Vellum ordinal counter
- Bento left stripe
- Shell side guide doc — zsh/bash/fish prompt 설정 (starship.toml 예시)
- **체크포인트**: starship 설정한 zsh에서 명령 블록이 그려짐. block_mode=off로 회귀 가능.

### Phase 5 — Floating window (2주)
- winit multi-window
- 자체 z-order + focus
- Drag handle (36px) + close 버튼
- Theme switch 시 close-all
- **체크포인트**: Cmd+Shift+D 같은 단축키로 분리창 spawn.

### Phase 6 — Tweaks UI (1주)
- Cmd+, 또는 우측 하단 토글 UI overlay
- TUI 폼 (text-based, native dialog 아님)
- theme / accent / mono 라이브 변경
- **체크포인트**: 런타임에 테마 전환 시 깜빡임 없이 적용.

총 11~15주. P0 보안/마우스 selection / M15 AgentKind와 우선순위 경합. **순서 결정 필요**.

---

## 6. 미해결 결정 포인트

1. **Window chrome**: macOS native title bar 유지 vs custom chrome (traffic lights 직접 그리기). custom으로 가면 접근성/cmd+drag/표준 UX 충돌. 권장: **native 유지** (Phase 1~6 외부)
2. **OSC 133 fallback**: shell이 송신 안 하면? `block_mode = "off"`로 raw VT로 falls back. 권장 prompt(starship) 사전 셋업 안내 doc 필요
3. **Multi-window**: 별도 winit 창 vs macOS tabbing (native) vs 포기. native macOS tabbing은 winit 0.30 지원 미확정
4. **Backdrop blur 비용**: 60fps × 1080p × 2-pass gaussian는 M1 Mac에서 OK여도 Intel/iGPU에서 떨어질 수 있음. profile에 `blur_quality = "off" | "low" | "high"` 옵션 도입
5. **Vellum paper noise**: 빌드 타임 PNG 생성 vs 런타임 shader noise (Perlin/simplex). Phase 2에서 측정
6. **6 테마 다 만들 vs 2~3개만**: Phase 4까지 완료해도 한 사용자가 6개 다 쓰지 않음. **권장 — Phase 1~3은 6 테마 다, Phase 4~5는 1~2 테마 (obsidian + vellum 정도)에 먼저 입히고 나머지는 P1으로 후순위**
7. **Pretendard / Geist / IBM Plex / Berkeley / Fira / JetBrains 폰트 ship 정책**: 시스템 의존 (handoff CDN) vs .app 번들 포함. 라이선스 체크 필요 (Berkeley는 유료. JetBrains/Geist/IBM Plex/Fira는 OFL)
8. **Block height 제한 + scrollback**: block_mode일 때 scrollback 페이지업/다운이 block 단위? line 단위? handoff는 block 단위 자연 흐름. pj001 M6-8 line-based scrollback 재설계 필요

---

## 7. 다음 액션 (즉시 가능)

이 문서가 1차 산출. 다음 결정에 사용:

- **A. 슬라이스 순서 확정** — Phase 1~6 순차 vs Phase 1 + Phase 4 우선 vs Phase 1만 먼저
- **B. Window chrome 정책** (§6.1)
- **C. block_mode 기본값** (§6.2) + shell prompt 설정 가이드 doc 필요 여부
- **D. Phase 1 시작 — config.toml schema + 6 preset 빌트인 + role mapping**

`docs/themes-handoff.md` 본 문서는 추출본. 코드 구현 시작 시 별도 `themes-design.md`로 phase별 detail design 분리.

## 참고

- handoff bundle: `/tmp/pj001-design-handoff/remix/`
- chat 원본: `/tmp/pj001-design-handoff/remix/chats/chat1.md`
- 변형 코드: `/tmp/pj001-design-handoff/remix/project/variants/{aurora,obsidian,vellum,holo,bento,crystal}.jsx`
- 공유 모델: `/tmp/pj001-design-handoff/remix/project/terminal-core.jsx`
- Tweaks 패널: `/tmp/pj001-design-handoff/remix/project/tweaks-panel.jsx`
- 스크린샷: `/tmp/pj001-design-handoff/remix/project/screenshots/*.png` (12-final.png 포함)
- archive 정본 plan과 정합: `archive/docs/architecture/m12-m16-pj001-sessions-tabs-bridge-plan.md`, pj001 `docs/roadmap.md` §15 (out-of-scope에 OSC 1337 명시되어 있으나 OSC 133은 P1 M13-4로 이미 잡혀 있음)
