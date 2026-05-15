# pj001 config schema — `~/.config/pj001/config.toml`

작성일: 2026-05-15
관련 코드: `crates/pj001/src/main.rs::FileConfig`

pj001은 in-app Preferences GUI 대신 표준 TOML 설정 파일을 사용한다. macOS 메뉴
"Preferences…"(Cmd+,) 클릭 시 시스템 기본 editor로 자동 열린다.

## 전체 schema

```toml
[general]
# 색 테마. aurora | obsidian | vellum | holo | bento | crystal
theme = "obsidian"
# Shell 경로. 미지정 시 CLI --shell, $SHELL, /bin/zsh 순.
shell = "/bin/zsh"

[block]
# OSC 133 명령 블록 UI 렌더 모드. "auto" (default) | "off"
mode = "auto"

[backdrop]
# macOS NSVisualEffectView vibrancy backdrop 활성화. default true.
# 환경변수 PJ001_NO_BACKDROP=1 이 더 우선.
enabled = true

[font]
# 폰트 크기 (pt). default 14.0. MIN/MAX clamp 적용.
# CLI 미지원 (config 전용).
size = 14.0

[bell]
# Visual bell (dock bounce + shader cell bg flash). default true.
visible = true
# Audible bell (NSBeep). default false (macOS Terminal.app 표준 정합).
audible = false
```

## 환경 변수 (config TOML보다 우선)

| Variable | 효과 |
|---|---|
| `PJ001_CONFIG=<path>` | config 파일 경로 override |
| `PJ001_NO_BACKDROP=1` | vibrancy backdrop 비활성 (config.backdrop.enabled 무시) |
| `RUST_LOG=info` 등 | log level |
| `LANG=ko_KR.UTF-8` | 한국어 menu 강제 (NSLocale 외) |

## CLI 인자 (config TOML보다 우선)

| Arg | 효과 |
|---|---|
| `--shell <path>` | shell override |
| `--theme <name>` | theme override |
| `--block-mode <auto\|off>` | block UI 모드 override |
| `-h, --help` | 도움말 |
| `-V, --version` | 버전 |

## section별 동작

### `[general]`
- `theme`: 시각 즉시 변경 (재시작 안 필요한 cut은 차후).
- `shell`: PTY spawn 시 사용. 재시작 필요.

### `[block]`
- `mode = "auto"`: OSC 133;A 수신 시 gutter + 카드 시각 발동.
- `mode = "off"`: parse만 하고 시각 발동 안 함.

### `[backdrop]`
- `enabled = false`: NSVE attach skip. 단순 transparent 윈도우 (cell.bg.alpha < 1이면 데스크톱 직접 비침, vibrancy blur 없음).

### `[font]`
- `size`: Renderer 초기 logical font size. CLI runtime Cmd+= / Cmd+- / Cmd+0로 조정 가능 (재시작 시 config 값으로 복귀).

### `[bell]`
- `visible = true`: macOS dock bounce (background) + shader cell bg flash 250ms (foreground).
- `audible = true`: NSBeep.

## 예시

### Glass aurora light (예쁘게)
```toml
[general]
theme = "aurora"

[backdrop]
enabled = true

[bell]
visible = true
audible = false

[font]
size = 14.5
```

### Power user (vibrancy 끄고, 큰 폰트, audible bell)
```toml
[general]
theme = "obsidian"

[backdrop]
enabled = false

[bell]
visible = true
audible = true

[font]
size = 16.0

[block]
mode = "auto"
```

### 한국어 사용자 (시스템 locale 자동 감지)
- macOS 시스템 언어가 한국어면 NSLocale.preferredLanguages가 "ko"로 시작 → menu가
  자동 한국어. LANG env var fallback.

## 검증

- 잘못된 theme/mode 값은 startup error.
- 알 수 없는 key는 silent ignore (`#[serde(default)]`로 missing field tolerant).
- `RUST_LOG=info`로 실행하면 어떤 config 값이 적용됐는지 startup log에 출력.

## 향후

- in-app TUI Preferences (별도 milestone).
- 색 picker / hex 입력 GUI.
- per-theme `[theme.aurora]` section으로 ANSI 색 override.
