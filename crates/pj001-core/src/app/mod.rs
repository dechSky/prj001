pub mod event;
mod input;
mod layout;
#[cfg(target_os = "macos")]
mod macos_backdrop;
#[cfg(target_os = "macos")]
mod macos_ime;
#[cfg(target_os = "macos")]
mod macos_overlay;
mod session;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use unicode_width::UnicodeWidthStr;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ImePurpose, Window, WindowId};

use crate::block::BlockState;
use crate::error::{Error, Result};
use crate::grid::{MouseProtocol, Term};
use crate::pty::PtyHandle;
use crate::render::{CellMetrics, CursorRender, Renderer, SelectionRange, ThemePalette};
use event::{IdAllocator, PaneId, SessionId, TabId, UserEvent};
#[cfg(test)]
use layout::SplitRatio;
use layout::{Layout, RatioDirection, SplitAxis};
use session::Session;

const DEFAULT_FONT_SIZE: f32 = 14.0;
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 72.0;
const FONT_SIZE_STEP: f32 = 1.0;
const MIN_WINDOW_WIDTH: u32 = 720;
const MIN_WINDOW_HEIGHT: u32 = 420;
const MIN_PANE_COLS: usize = 30;
const MIN_PANE_ROWS: usize = 5;
const TAB_BAR_ROWS: usize = 1;
const CURSOR_BLINK_MS: u64 = 500;
const QUICK_SPAWN_TIMEOUT_MS: u64 = 3_000;
const MULTI_CLICK_MS: u64 = 500;
const TAB_BAR_BG: [f32; 4] = [0.10, 0.11, 0.13, 1.0];
const TAB_ACTIVE_BG: [f32; 4] = [0.18, 0.32, 0.42, 1.0];
const TAB_INACTIVE_BG: [f32; 4] = [0.15, 0.16, 0.18, 1.0];
const STATUS_FG: [f32; 4] = [0.92, 0.94, 0.96, 1.0];
const STATUS_ACTIVE_BG: [f32; 4] = [0.14, 0.30, 0.42, 1.0];
const STATUS_INACTIVE_BG: [f32; 4] = [0.12, 0.13, 0.15, 1.0];
const STATUS_DEAD_BG: [f32; 4] = [0.40, 0.12, 0.12, 1.0];
const DIVIDER_BG: [f32; 4] = [0.22, 0.23, 0.26, 1.0];

fn scale_factor_or_default(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

fn clamp_font_size(font_size: f32) -> f32 {
    if font_size.is_finite() {
        font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        DEFAULT_FONT_SIZE
    }
}

fn physical_font_size(logical_font_size: f32, scale_factor: f64) -> f32 {
    clamp_font_size(logical_font_size) * scale_factor_or_default(scale_factor) as f32
}

fn posix_single_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for ch in text.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn shell_quoted_path_for_drop(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    let mut text = posix_single_quote(text);
    text.push(' ');
    Some(text)
}

fn paste_payload_bytes(text: &str, bracketed: bool) -> Option<Vec<u8>> {
    // 보안: paste/drop 데이터에서 위험한 control char 제거.
    // 정책 — keep tab/LF만, ESC + 기타 C0 + C1 + DEL + CR 전부 strip.
    // (CR strip 이유: bracketed paste off일 때 shell이 paste 내용을 즉시 실행하는
    //  paste-execution 취약점을 닫음. 호환성 vs 보안 트레이드오프 — 보안 선택.)
    let sanitized = sanitize_paste_text(text);
    if bracketed && sanitized.contains("\x1b[201~") {
        // sanitize 이후엔 ESC가 없어야 하지만 방어적으로 한 번 더.
        return None;
    }
    if !bracketed {
        return Some(sanitized.into_bytes());
    }

    let mut bytes = Vec::with_capacity(sanitized.len() + b"\x1b[200~".len() + b"\x1b[201~".len());
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(sanitized.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    Some(bytes)
}

/// 슬라이스 6.6: xterm mouse report 인코딩. SGR(1006) 또는 legacy.
/// `button`: 0=left, 1=middle, 2=right, 64=wheel-up, 65=wheel-down.
/// `motion` true면 +32 (1002/1003 드래그/모션). press/release는 SGR 'M'/'m', legacy는 항상 'M' + button=3 release.
/// col/row: 0-based 입력, encoding은 1-based.
#[allow(clippy::too_many_arguments)]
fn encode_mouse_report(
    button: u8,
    press: bool,
    motion: bool,
    shift: bool,
    alt: bool,
    ctrl: bool,
    col: usize,
    row: usize,
    sgr: bool,
) -> Vec<u8> {
    let mut b = button;
    if shift {
        b += 4;
    }
    if alt {
        b += 8;
    }
    if ctrl {
        b += 16;
    }
    if motion {
        b += 32;
    }
    let col1 = col.saturating_add(1);
    let row1 = row.saturating_add(1);
    if sgr {
        let final_byte = if press || motion { 'M' } else { 'm' };
        format!("\x1b[<{};{};{}{}", b, col1, row1, final_byte).into_bytes()
    } else {
        // legacy X10: col/row 클램프 223 (인코딩 한도 224 + 32 base).
        let legacy_b = if press || motion { b } else { 3 };
        let c_byte = 32u8.saturating_add(legacy_b);
        let cx = 32u8.saturating_add(col1.min(223) as u8);
        let cy = 32u8.saturating_add(row1.min(223) as u8);
        vec![0x1b, b'[', b'M', c_byte, cx, cy]
    }
}

/// paste/drop 데이터에서 control char 제거. 보존: LF, HT(tab).
/// 제거: ESC(0x1B) + 그 외 C0(0x00-0x1F except LF/HT) + DEL(0x7F) + C1(0x80-0x9F) + CR(0x0D).
/// CR은 LF로 변환 — 줄바꿈 의도는 보존하되 paste-execution은 차단.
pub(crate) fn sanitize_paste_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            '\r' => out.push('\n'),
            // C0 controls + DEL
            '\x00'..='\x08' | '\x0B' | '\x0C' | '\x0E'..='\x1F' | '\x7F' => {}
            // C1 controls (U+0080..U+009F)
            '\u{0080}'..='\u{009F}' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// M8-5: PageUp/Down 분기 첫 발생 시점 1회 log. 같은 dispatch는 다시 안 찍음.
fn log_page_dispatch_once(target: &'static str) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static LOGGED: AtomicU8 = AtomicU8::new(0);
    let bit: u8 = if target.starts_with("PTY") { 1 } else { 2 };
    let prev = LOGGED.fetch_or(bit, Ordering::Relaxed);
    if prev & bit == 0 {
        log::info!("page-key dispatch: target={target}");
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub sessions: Vec<SessionSpec>,
    pub initial_layout: InitialLayout,
    pub quick_spawn_presets: Vec<QuickSpawnPreset>,
    pub hooks: Hooks,
    /// 활성 테마 팔레트. `None`이면 `ThemePalette::default_theme()` 사용.
    pub theme: Option<ThemePalette>,
    /// Block UI 렌더 모드. default Auto — OSC 133 수신 시 gutter/card visual ON.
    /// Off — 절대 visual ON 안 함. Phase 4b부터 실제 시각 분기.
    pub block_mode: BlockMode,
    /// Phase 3 step 3: macOS NSVisualEffectView vibrancy backdrop 활성화.
    /// None = default ON. env var `PJ001_NO_BACKDROP=1`이 더 우선 (escape hatch).
    /// macOS 외 OS에서는 무의미.
    pub backdrop_enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BlockMode {
    #[default]
    Auto,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSpec {
    pub title: String,
    pub command: CommandSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickSpawnPreset {
    pub key: char,
    pub spec: SessionSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandSpec {
    Shell,
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitialLayout {
    Single {
        session: usize,
    },
    Split {
        direction: SplitDirection,
        first: usize,
        second: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Vertical,
}

/// Optional integration hooks for embedders.
///
/// `control_sink` receives an `AppControl` after the event loop proxy exists,
/// so embedders can send policy-approved bytes to a known session. `lifecycle_sink`
/// is invoked for session start and child-exit events. PTY errors currently mark
/// panes dead but are not reported as lifecycle events.
/// `route_sink` is reserved for future in-core route gestures and is not invoked
/// by `AppControl::write_to_session`.
#[derive(Clone, Default)]
pub struct Hooks {
    pub control_sink: Option<Arc<dyn ControlSink>>,
    pub route_sink: Option<Arc<dyn RouteSink>>,
    pub lifecycle_sink: Option<Arc<dyn LifecycleSink>>,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks")
            .field(
                "control_sink",
                &self.control_sink.as_ref().map(|_| "<ControlSink>"),
            )
            .field(
                "route_sink",
                &self.route_sink.as_ref().map(|_| "<RouteSink>"),
            )
            .field(
                "lifecycle_sink",
                &self.lifecycle_sink.as_ref().map(|_| "<LifecycleSink>"),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct AppControl {
    /// Embedder write path 폐기 후 미사용. struct 자체는 외부 API 호환성 위해 유지.
    #[allow(dead_code)]
    proxy: EventLoopProxy<UserEvent>,
}

impl fmt::Debug for AppControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppControl").finish_non_exhaustive()
    }
}

impl AppControl {
    /// WriteToSession variant 제거됨. embedder write path는 별도 정책 영역으로 이전.
    /// 호출 시 no-op + warn log.
    pub fn write_to_session(&self, _id: SessionId, _bytes: Vec<u8>) -> bool {
        log::warn!("AppControl::write_to_session: WriteToSession variant removed, ignored");
        false
    }
}

pub trait ControlSink: Send + Sync {
    fn on_control(&self, control: AppControl);
}

pub trait RouteSink: Send + Sync {
    fn on_route(&self, event: RouteEvent);
}

/// M12-2: route_sink는 reserved (M16에서 호출됨). 시그니처는 SessionId로 미리 정합.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteEvent {
    pub from_session: SessionId,
    pub to_sessions: Vec<SessionId>,
    pub bytes: Vec<u8>,
}

pub trait LifecycleSink: Send + Sync {
    fn on_lifecycle(&self, event: LifecycleEvent);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    SessionStarted {
        session_id: SessionId,
        title: String,
    },
    SessionExited {
        session_id: SessionId,
        code: i32,
    },
}

/// Hook trait objects are intentionally ignored for equality. This keeps CLI
/// parser tests focused on structural config and avoids pretending sinks have
/// useful value equality.
impl PartialEq for Config {
    fn eq(&self, other: &Self) -> bool {
        self.sessions == other.sessions
            && self.initial_layout == other.initial_layout
            && self.quick_spawn_presets == other.quick_spawn_presets
    }
}

impl Eq for Config {}

impl Config {
    pub fn single_shell(shell: Option<String>) -> Self {
        Self {
            sessions: vec![SessionSpec {
                title: "shell".to_string(),
                command: shell.map_or(CommandSpec::Shell, CommandSpec::Custom),
            }],
            initial_layout: InitialLayout::Single { session: 0 },
            quick_spawn_presets: default_quick_spawn_presets(),
            hooks: Hooks::default(),
            theme: None,
            block_mode: BlockMode::default(),
            backdrop_enabled: None,
        }
    }

    pub fn vertical_split(first: SessionSpec, second: SessionSpec) -> Self {
        Self {
            sessions: vec![first, second],
            initial_layout: InitialLayout::Split {
                direction: SplitDirection::Vertical,
                first: 0,
                second: 1,
            },
            quick_spawn_presets: default_quick_spawn_presets(),
            hooks: Hooks::default(),
            theme: None,
            block_mode: BlockMode::default(),
            backdrop_enabled: None,
        }
    }

    pub fn with_block_mode(mut self, mode: BlockMode) -> Self {
        self.block_mode = mode;
        self
    }

    pub fn with_quick_spawn_presets(mut self, presets: Vec<QuickSpawnPreset>) -> Self {
        self.quick_spawn_presets = presets;
        self
    }

    pub fn with_hooks(mut self, hooks: Hooks) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn with_theme(mut self, theme: ThemePalette) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Phase 3 step 3: macOS backdrop vibrancy 활성/비활성. None = default(ON).
    pub fn with_backdrop_enabled(mut self, enabled: Option<bool>) -> Self {
        self.backdrop_enabled = enabled;
        self
    }

    fn pane_specs(&self) -> Result<Vec<SessionSpec>> {
        let indices = match self.initial_layout {
            InitialLayout::Single { session } => vec![session],
            InitialLayout::Split {
                direction: SplitDirection::Vertical,
                first,
                second,
            } => {
                if first == second {
                    return Err(Error::Args(
                        "split layout requires two distinct sessions".to_string(),
                    ));
                }
                vec![first, second]
            }
        };

        indices
            .into_iter()
            .map(|idx| {
                self.sessions.get(idx).cloned().ok_or_else(|| {
                    Error::Args(format!("layout references missing session index {idx}"))
                })
            })
            .collect()
    }
}

fn default_quick_spawn_presets() -> Vec<QuickSpawnPreset> {
    vec![QuickSpawnPreset {
        key: 's',
        spec: SessionSpec {
            title: "shell".to_string(),
            command: CommandSpec::Shell,
        },
    }]
}

impl CommandSpec {
    fn resolve(&self) -> String {
        match self {
            CommandSpec::Shell => std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()),
            CommandSpec::Custom(command) => command.clone(),
        }
    }
}

pub fn run(config: Config) -> Result<()> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Regular)
            .with_activate_ignoring_other_apps(true);
    }
    let event_loop = builder.build()?;
    let proxy = event_loop.create_proxy();
    if let Some(sink) = &config.hooks.control_sink {
        sink.on_control(AppControl {
            proxy: proxy.clone(),
        });
    }
    let mut app = App::new(proxy, config);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    state: Option<AppState>,
    /// M12-5 회귀 fix v3 (Codex thread 019e1659): spawn을 첫 winit `Resized` 후로 미룸.
    /// resumed에서 window만 생성하고 여기 보관 → 첫 Resized에서 그 size로 PTY spawn.
    /// startup SIGWINCH 0회 → wrap된 작은 창에서도 zsh duplicate prompt 회피.
    pending_window: Option<Arc<Window>>,
    /// about_to_wait fallback — 첫 cycle은 wait, 두 번째 cycle에도 Resized 없으면 inner_size로 spawn.
    startup_waited_once: bool,
    proxy: EventLoopProxy<UserEvent>,
    config: Config,
}

struct Pane {
    id: PaneId,
    /// M12-3: 어떤 Session을 보여주는가. M14까지 1 Session = 0..1 Pane.
    session: SessionId,
    /// M12-5: cell + pixel 단위 viewport. 직접 field mutation 금지 — `compute_viewports`만 통과.
    viewport: PaneViewport,
}

struct Tab {
    id: TabId,
    title: String,
    panes: Vec<Pane>,
    root: Layout,
    active: PaneId,
}

/// M12-5: pane이 점유하는 시각 영역. cell 단위(cols/rows/col_offset/status_row)와
/// pixel 단위(x_px/y_px/width_px/height_px) 양쪽 보유. 직접 mutation 금지 정책 —
/// `compute_viewports`만이 새 PaneViewport를 생성. M13 BSP 진입 시 동일 struct를
/// LayoutNode resolve 결과로 받음.
#[derive(Clone, Copy)]
struct PaneViewport {
    cols: usize,
    rows: usize,
    col_offset: usize,
    row_offset: usize,
    status_row: Option<usize>,
    /// M13 BSP scissor rect / pixel-단위 hit-test 진입 시 활용. M12에는 PTY size 계산용.
    #[allow(dead_code)]
    x_px: u32,
    #[allow(dead_code)]
    y_px: u32,
    width_px: u32,
    height_px: u32,
    /// Block UI Phase 4a — prompt marker gutter 폭. 4a는 강제 0.
    /// 4b에서 block_capable && block_mode==auto일 때 발동.
    /// 4b-2c-3: AppState::render에서 gutter_cells 계산에 사용.
    gutter_px: u16,
    /// `x_px + gutter_px`. mouse/selection 좌표 변환 시 사용. 4a는 x_px와 동일.
    #[allow(dead_code)]
    content_x_px: u32,
}

/// Drag 중 cursor의 physical position을 selection.pane viewport 안 cell 좌표로 clamp.
/// 음수 좌표(창 위/왼쪽 밖) → 0, 너무 큰 좌표(아래/오른쪽 밖) → 가장자리 cell.
/// 결과는 viewport-local (row, col).
/// cell 단위 clamp helper. Phase 2 caret 전환으로 selection은 caret 사용 → 호출처 없음.
/// 다른 컨텍스트에서 활용 가능하니 보존(테스트 6개로 동작 검증).
#[allow(dead_code)]
fn clamp_pos_to_viewport_cell(
    pos: PhysicalPosition<f64>,
    viewport: PaneViewport,
    cell: CellMetrics,
) -> (usize, usize) {
    if cell.width == 0 || cell.height == 0 {
        return (0, 0);
    }
    let abs_col = if pos.x < 0.0 {
        0
    } else {
        (pos.x / cell.width as f64).floor() as usize
    };
    let abs_row = if pos.y < 0.0 {
        0
    } else {
        (pos.y / cell.height as f64).floor() as usize
    };
    let cols_max = viewport.cols.saturating_sub(1);
    let rows_max = viewport.rows.saturating_sub(1);
    let local_col = abs_col.saturating_sub(viewport.col_offset).min(cols_max);
    let local_row = abs_row.saturating_sub(viewport.row_offset).min(rows_max);
    (local_row, local_col)
}

/// Phase 1: caret 모델 helper. mouse pos를 selection.pane viewport 안 caret 좌표로 clamp.
///
/// caret은 글자 사이 위치. col 0..=cols 범위. mouse가 cell N의 왼쪽 절반이면 caret=N,
/// 오른쪽 절반이면 caret=N+1. 표준 macOS Terminal/iTerm2 selection 모델.
///
/// 음수 좌표 → caret 0, viewport 우측 밖 → caret = cols. row는 cell clamp와 동일.
#[allow(dead_code)]
fn clamp_pos_to_viewport_caret(
    pos: PhysicalPosition<f64>,
    viewport: PaneViewport,
    cell: CellMetrics,
) -> (usize, usize) {
    if cell.width == 0 || cell.height == 0 {
        return (0, 0);
    }
    let cell_w = cell.width as f64;
    let abs_caret = if pos.x < 0.0 {
        0
    } else {
        // round to nearest caret. (pos.x + cell_w/2) / cell_w를 floor.
        ((pos.x + cell_w * 0.5) / cell_w).floor() as usize
    };
    let abs_row = if pos.y < 0.0 {
        0
    } else {
        (pos.y / cell.height as f64).floor() as usize
    };
    let cols_max = viewport.cols; // caret 최대 = cols (cell index가 아닌 boundary count)
    let rows_max = viewport.rows.saturating_sub(1);
    let local_caret = abs_caret.saturating_sub(viewport.col_offset).min(cols_max);
    let local_row = abs_row.saturating_sub(viewport.row_offset).min(rows_max);
    (local_row, local_caret)
}

fn status_segment(
    group_col: usize,
    group_cols: usize,
    group_len: usize,
    index: usize,
) -> (usize, usize) {
    if group_len <= 1 {
        return (group_col, group_cols);
    }
    let start = group_col + (group_cols * index / group_len);
    let end = group_col + (group_cols * (index + 1) / group_len);
    (start, end.saturating_sub(start).max(1))
}

fn status_text(state: &str, title: &str, cols: usize) -> String {
    let state_width = UnicodeWidthStr::width(state);
    let title_width = UnicodeWidthStr::width(title);
    if cols >= state_width + title_width + 3 {
        format!(" {state} {title} ")
    } else if cols >= state_width + 2 {
        format!(" {state} ")
    } else if cols >= 3 {
        format!(" {} ", state.chars().next().unwrap_or(' '))
    } else {
        String::new()
    }
}

fn quick_spawn_hint(presets: &[QuickSpawnPreset]) -> String {
    let mut keys = presets
        .iter()
        .map(|preset| preset.key.to_ascii_lowercase())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    if keys.is_empty() {
        "SPAWN".to_string()
    } else {
        format!("SPAWN {}", keys.into_iter().collect::<String>())
    }
}

fn quick_spawn_elapsed_timed_out(started_at: Instant, now: Instant) -> bool {
    now.duration_since(started_at) > Duration::from_millis(QUICK_SPAWN_TIMEOUT_MS)
}

/// Phase 5: selection abs row 기반 text 추출. selection.start/end는 abs caret 좌표.
/// scrollback row + main row 양쪽 cell_at_abs로 접근. evict된 abs는 skip.
fn selection_text_abs(term: &Term, selection: SelectionRange) -> String {
    let mut lines = Vec::new();
    let start_row = selection.start.0;
    let end_row = selection.end.0;
    let cols = term.cols();
    for abs_row in start_row..=end_row {
        let start_col = if abs_row == start_row {
            selection.start.1
        } else {
            0
        };
        let end_col = if abs_row == end_row {
            selection.end.1
        } else {
            cols
        };
        let mut line = String::new();
        let s = start_col.min(cols);
        let e = end_col.min(cols);
        for col in s..e {
            let Some(cell) = term.cell_at_abs(abs_row as u64, col) else {
                continue;
            };
            if cell.attrs.contains(crate::grid::Attrs::WIDE_CONT) {
                continue;
            }
            line.push(cell.ch);
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// Phase 5 이전 viewport-local selection text — test에서만 사용 (selection_text_abs로 전환).
#[cfg(test)]
fn selection_text(term: &Term, selection: SelectionRange) -> String {
    // caret 모델: selection.start/end는 caret 좌표. cell col iterate은 half-open
    // [start.col, end.col)에서. middle row는 0..cols.
    let mut lines = Vec::new();
    let start_row = selection.start.0.min(term.rows().saturating_sub(1));
    let end_row = selection.end.0.min(term.rows().saturating_sub(1));
    for row in start_row..=end_row {
        let start_col = if row == selection.start.0 {
            selection.start.1
        } else {
            0
        };
        let end_col = if row == selection.end.0 {
            selection.end.1
        } else {
            term.cols()
        };
        let mut line = String::new();
        let cols = term.cols();
        let s = start_col.min(cols);
        let e = end_col.min(cols);
        for col in s..e {
            let cell = term.cell(row, col);
            if cell.attrs.contains(crate::grid::Attrs::WIDE_CONT) {
                continue;
            }
            line.push(cell.ch);
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

fn cmd_named_key_bytes(key: &winit::keyboard::NamedKey) -> Option<&'static [u8]> {
    match key {
        // macOS text-field convention: Cmd+Left/Right jump to line start/end.
        // Use the same Home/End byte sequences this app already sends for zsh compatibility.
        winit::keyboard::NamedKey::ArrowLeft => Some(b"\x1bOH"),
        winit::keyboard::NamedKey::ArrowRight => Some(b"\x1bOF"),
        _ => None,
    }
}

fn is_word_selection_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

fn word_selection_range(term: &Term, row: usize, col: usize) -> Option<SelectionRange> {
    if row >= term.rows() || col >= term.cols() {
        return None;
    }
    let cell = term.cell(row, col);
    if !is_word_selection_char(cell.ch) {
        return None;
    }
    let mut start = col;
    while start > 0 {
        let prev = term.cell(row, start - 1);
        if !is_word_selection_char(prev.ch) {
            break;
        }
        start -= 1;
    }
    let mut end = col;
    while end + 1 < term.cols() {
        let next = term.cell(row, end + 1);
        if !is_word_selection_char(next.ch) {
            break;
        }
        end += 1;
    }
    // caret 모델: end caret은 마지막 cell의 다음 boundary (= end_cell + 1).
    Some(SelectionRange::new((row, start), (row, end + 1)))
}

fn tab_text(title: &str, cols: usize) -> String {
    let title_width = UnicodeWidthStr::width(title);
    if cols >= title_width + 2 {
        format!(" {title} ")
    } else if cols >= 2 {
        format!(" {}", title.chars().next().unwrap_or(' '))
    } else {
        String::new()
    }
}

fn tab_label(index: usize, title: &str) -> String {
    format!("{} {}", index + 1, title)
}

fn tab_index_at_col(total_cols: usize, tab_count: usize, col: usize) -> Option<usize> {
    if total_cols == 0 || tab_count == 0 || col >= total_cols {
        return None;
    }
    (0..tab_count).find(|idx| {
        let (segment_col, segment_cols) = status_segment(0, total_cols, tab_count, *idx);
        col >= segment_col && col < segment_col + segment_cols
    })
}

fn ordinal_from_digit_key(
    lower: Option<&str>,
    physical_code: Option<winit::keyboard::KeyCode>,
) -> Option<usize> {
    use winit::keyboard::KeyCode;

    if let Some(ordinal) = match physical_code {
        Some(KeyCode::Digit1) => Some(1),
        Some(KeyCode::Digit2) => Some(2),
        Some(KeyCode::Digit3) => Some(3),
        Some(KeyCode::Digit4) => Some(4),
        Some(KeyCode::Digit5) => Some(5),
        Some(KeyCode::Digit6) => Some(6),
        Some(KeyCode::Digit7) => Some(7),
        Some(KeyCode::Digit8) => Some(8),
        Some(KeyCode::Digit9) => Some(9),
        _ => None,
    } {
        return Some(ordinal);
    }

    lower
        .and_then(|s| s.chars().next())
        .and_then(|ch| ch.to_digit(10))
        .filter(|digit| (1..=9).contains(digit))
        .map(|digit| digit as usize)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CmdShortcut {
    TabOrdinal(usize),
    PaneOrdinal(usize),
    PrevTab,
    NextTab,
    PrevPane,
    NextPane,
    SplitVertical,
    SplitHorizontal,
    NewPane,
    NewTab,
    QuickSpawnStart,
    RespawnSession,
    FontZoomIn,
    FontZoomOut,
    FontZoomReset,
    ClosePaneOrTab,
    CloseTab,
    Quit,
    Copy,
    Paste,
    ClearBuffer,
    ClearScrollback,
    /// 슬라이스 6.5: Cmd+F find.
    Find,
    /// Phase 5: Cmd+A — 활성 pane의 scrollback+viewport 전체 selection.
    SelectAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseDecision {
    ClosePane,
    CloseTab,
    Exit,
}

fn close_decision(shortcut: CmdShortcut, pane_count: usize, tab_count: usize) -> CloseDecision {
    match shortcut {
        CmdShortcut::CloseTab => {
            if tab_count <= 1 {
                CloseDecision::Exit
            } else {
                CloseDecision::CloseTab
            }
        }
        CmdShortcut::ClosePaneOrTab => {
            if pane_count > 1 {
                CloseDecision::ClosePane
            } else if tab_count > 1 {
                CloseDecision::CloseTab
            } else {
                CloseDecision::Exit
            }
        }
        _ => unreachable!("close_decision only accepts close shortcuts"),
    }
}

fn cmd_shortcut(
    lower: Option<&str>,
    physical_code: Option<winit::keyboard::KeyCode>,
    shift: bool,
    alt: bool,
) -> Option<CmdShortcut> {
    use winit::keyboard::KeyCode;

    if let Some(ordinal) = ordinal_from_digit_key(lower, physical_code) {
        return if alt {
            Some(CmdShortcut::PaneOrdinal(ordinal))
        } else if !shift {
            Some(CmdShortcut::TabOrdinal(ordinal))
        } else {
            None
        };
    }
    if shift && (physical_code == Some(KeyCode::BracketLeft) || lower == Some("{")) {
        return Some(CmdShortcut::PrevTab);
    }
    if shift && (physical_code == Some(KeyCode::BracketRight) || lower == Some("}")) {
        return Some(CmdShortcut::NextTab);
    }
    if lower == Some("[") {
        return Some(CmdShortcut::PrevPane);
    }
    if lower == Some("]") {
        return Some(CmdShortcut::NextPane);
    }
    if lower == Some("d") || physical_code == Some(KeyCode::KeyD) {
        return Some(if shift {
            CmdShortcut::SplitHorizontal
        } else {
            CmdShortcut::SplitVertical
        });
    }
    if lower == Some("t") || physical_code == Some(KeyCode::KeyT) {
        return Some(CmdShortcut::NewTab);
    }
    if lower == Some("n") || physical_code == Some(KeyCode::KeyN) {
        return if shift {
            Some(CmdShortcut::QuickSpawnStart)
        } else {
            Some(CmdShortcut::NewPane)
        };
    }
    if lower == Some("r") || physical_code == Some(KeyCode::KeyR) {
        return Some(CmdShortcut::RespawnSession);
    }
    if lower == Some("k") || physical_code == Some(KeyCode::KeyK) {
        return Some(if shift {
            CmdShortcut::ClearScrollback
        } else if alt {
            CmdShortcut::ClearScrollback
        } else {
            CmdShortcut::ClearBuffer
        });
    }
    if !alt && (physical_code == Some(KeyCode::Equal) || lower == Some("=") || lower == Some("+")) {
        return Some(CmdShortcut::FontZoomIn);
    }
    if !alt && (physical_code == Some(KeyCode::Minus) || lower == Some("-") || lower == Some("_")) {
        return Some(CmdShortcut::FontZoomOut);
    }
    if !alt && (physical_code == Some(KeyCode::Digit0) || lower == Some("0")) {
        return Some(CmdShortcut::FontZoomReset);
    }
    if lower == Some("w") || physical_code == Some(KeyCode::KeyW) {
        return Some(if shift {
            CmdShortcut::CloseTab
        } else {
            CmdShortcut::ClosePaneOrTab
        });
    }
    if lower == Some("q") || physical_code == Some(KeyCode::KeyQ) {
        return Some(CmdShortcut::Quit);
    }
    if lower == Some("c") || physical_code == Some(KeyCode::KeyC) {
        return Some(CmdShortcut::Copy);
    }
    if lower == Some("v") || physical_code == Some(KeyCode::KeyV) {
        return Some(CmdShortcut::Paste);
    }
    if lower == Some("f") || physical_code == Some(KeyCode::KeyF) {
        return Some(CmdShortcut::Find);
    }
    if lower == Some("a") || physical_code == Some(KeyCode::KeyA) {
        return Some(CmdShortcut::SelectAll);
    }
    None
}

fn compute_tab_viewports(
    root: &Layout,
    size: PhysicalSize<u32>,
    cell: crate::render::CellMetrics,
    gutter_px: u32,
) -> HashMap<PaneId, PaneViewport> {
    let content_size = tab_content_size(size, cell);
    let mut layouts = layout::compute_viewports(root, content_size, cell);
    let cell_w = cell.width.max(1);
    let gutter_cells = gutter_px / cell_w;
    let gutter_px_aligned = gutter_cells * cell_w;
    for viewport in layouts.values_mut() {
        viewport.row_offset += TAB_BAR_ROWS;
        viewport.status_row = viewport.status_row.map(|row| row + TAB_BAR_ROWS);
        viewport.y_px = viewport
            .y_px
            .saturating_add(cell.height * TAB_BAR_ROWS as u32);
        // Phase 4b-1c: pane gutter 발동. cell.width 단위로 align.
        // 좌측 gutter — content cell이 우측으로 shift되어야 design §8 정합.
        // 4b-1c-fix: col_offset도 같이 +=gutter_cells. 없으면 cell 위치 그대로 두고
        // cols만 줄어 우측 stripe가 생기는 버그 (block_capable=false면 시각 영향 0).
        if gutter_cells > 0 && (viewport.cols as u32) > gutter_cells {
            viewport.cols -= gutter_cells as usize;
            viewport.col_offset += gutter_cells as usize;
            viewport.gutter_px = gutter_px_aligned.min(u16::MAX as u32) as u16;
            viewport.content_x_px = viewport.x_px.saturating_add(gutter_px_aligned);
            viewport.width_px = viewport.width_px.saturating_sub(gutter_px_aligned);
        }
    }
    layouts
}

/// Phase 5: 색 mix helper. dim scrollbar thumb 등 partial blend에 사용.
fn mix_palette(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] * (1.0 - t) + b[0] * t,
        a[1] * (1.0 - t) + b[1] * t,
        a[2] * (1.0 - t) + b[2] * t,
        a[3] * (1.0 - t) + b[3] * t,
    ]
}

/// Phase 5: scrollback이 있을 때 우측에 표시할 scrollbar thumb 위치/길이(viewport row 단위).
/// scrollback_len == 0이면 None. view_offset = 0(bottom)이면 thumb이 viewport 바닥, view_offset =
/// scrollback_len(top)이면 thumb이 viewport 천장.
fn scrollbar_thumb(
    viewport_rows: usize,
    scrollback_len: usize,
    view_offset: usize,
) -> Option<(usize, usize)> {
    if scrollback_len == 0 || viewport_rows == 0 {
        return None;
    }
    let total = scrollback_len + viewport_rows;
    let thumb_size = ((viewport_rows * viewport_rows) / total).max(1);
    let bottom_anchored = scrollback_len.saturating_sub(view_offset);
    let top_offset = (bottom_anchored * viewport_rows) / total;
    // clamp: thumb이 viewport 안에 들어가도록.
    let top_offset = top_offset.min(viewport_rows.saturating_sub(thumb_size));
    Some((top_offset, thumb_size))
}

/// modifier-only key 판정 — Cmd/Shift/Ctrl/Alt/Super/Meta/Fn 단독 press는 PTY로 안 보내고
/// type-to-snap도 안 발동해야 함 (Derek 보고). winit NamedKey 매칭.
fn is_modifier_only_key(key: &winit::keyboard::Key) -> bool {
    use winit::keyboard::{Key, NamedKey};
    matches!(
        key,
        Key::Named(NamedKey::Control)
            | Key::Named(NamedKey::Shift)
            | Key::Named(NamedKey::Alt)
            | Key::Named(NamedKey::Super)
            | Key::Named(NamedKey::Meta)
            | Key::Named(NamedKey::Hyper)
            | Key::Named(NamedKey::Fn)
            | Key::Named(NamedKey::FnLock)
            | Key::Named(NamedKey::CapsLock)
            | Key::Named(NamedKey::NumLock)
            | Key::Named(NamedKey::ScrollLock)
            | Key::Named(NamedKey::Symbol)
            | Key::Named(NamedKey::SymbolLock)
    )
}

/// Phase 4c: BlockState + duration_ms → status badge text. None이면 badge 안 그림.
fn status_badge_text(state: &BlockState, duration_ms: Option<u64>) -> Option<String> {
    match state {
        BlockState::Prompt => None,
        BlockState::Command | BlockState::Running => Some(" RUN ".to_string()),
        BlockState::Completed { exit_code: Some(0) } => duration_ms.map(|ms| format!(" {}ms ", ms)),
        BlockState::Completed {
            exit_code: Some(code),
        } => Some(format!(" x {} ", code)),
        BlockState::Completed { exit_code: None } => Some(" ? ".to_string()),
        BlockState::Abandoned { .. } => Some(" -- ".to_string()),
    }
}

/// Phase 4b-1c gutter_px helper. block_visual=true이면 font_size+4를 cell.width 단위로 ceil.
fn compute_block_gutter_px(
    block_visual: bool,
    cell: crate::render::CellMetrics,
    font_size: f32,
) -> u32 {
    if !block_visual {
        return 0;
    }
    let raw = (font_size + 4.0).max(0.0) as u32;
    let cell_w = cell.width.max(1);
    raw.div_ceil(cell_w) * cell_w
}

fn tab_content_size(
    size: PhysicalSize<u32>,
    cell: crate::render::CellMetrics,
) -> PhysicalSize<u32> {
    PhysicalSize::new(
        size.width,
        size.height
            .saturating_sub(cell.height * TAB_BAR_ROWS as u32),
    )
}

struct AppState {
    window: Arc<Window>,
    proxy: EventLoopProxy<UserEvent>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// M12-3: PTY 프로세스 보관. Pane은 SessionId로 참조.
    sessions: HashMap<SessionId, Session>,
    /// M14-1: 각 tab은 독립 panes + BSP root + active pane을 보유한다.
    tabs: Vec<Tab>,
    active_tab: TabId,
    renderer: Renderer,
    hooks: Hooks,
    last_ime_cursor: Option<(usize, usize)>,
    preedit: Option<String>,
    cursor_visible: bool,
    last_blink: Instant,
    focused: bool,
    cursor_blinking_cache: bool,
    modifiers: ModifiersState,
    last_mouse_pos: Option<PhysicalPosition<f64>>,
    dragging_divider: Option<layout::DividerHit>,
    selection: Option<MouseSelection>,
    /// Phase 5: drag 중 마우스 viewport 밖 sticky scroll throttle. 마지막 auto-scroll 시점.
    last_auto_scroll_at: Option<Instant>,
    last_click: Option<MouseClick>,
    /// M17-5: resize coalesce. winit Resized burst를 about_to_wait에서 마지막 size로만 처리.
    /// 매번 reflow + PTY size 갱신하면 zsh가 따라잡지 못해 redraw 시퀀스가 잘못된 size에 적용 → tearing.
    pending_resize: Option<PhysicalSize<u32>>,
    logical_font_size: f32,
    pending_logical_font_size: Option<f32>,
    preserve_grid_on_next_resize: bool,
    current_scale_factor: f64,
    quick_spawn_presets: Vec<QuickSpawnPreset>,
    /// Phase 4b: Block UI 렌더 모드. Config에서 받음. block_visual_active() helper에서 사용.
    /// 4b-1c (viewport gutter 발동)부터 실제 사용.
    #[allow(dead_code)]
    block_mode: BlockMode,
    pending_quick_spawn: Option<Instant>,
    /// 슬라이스 6.6b (Codex review B-1): 마우스 버튼 상태 비트마스크.
    /// bit 0 = Left, bit 1 = Middle, bit 2 = Right. selection.dragging은 reporting이
    /// press를 흡수하면 false라서 1002 ButtonEvent drag motion이 안 보내짐 → 별도 추적.
    mouse_buttons_held: u8,
    /// M12-6: design §2.1의 monotonic ID 정책을 IdAllocator로 캡슐화. M15 dynamic spawn에서 활용.
    #[allow(dead_code)]
    ids: IdAllocator,
    /// 슬라이스 6.5: Cmd+F find 진행 중 상태. None이면 비활성.
    pending_find: Option<FindState>,
    /// M-P3-2a: macOS WgpuOverlay attach. Retained NSView/CAMetalLayer 보관 — drop 시
    /// AppKit 객체 자동 release. AppState가 main thread 전용이라 Send 불필요.
    #[cfg(target_os = "macos")]
    #[allow(dead_code)]
    overlay_attach: Option<macos_overlay::OverlayAttach>,
}

#[derive(Clone, Debug, Default)]
struct FindState {
    query: String,
    /// 마지막 발견된 match의 (절대 row, col_start).
    /// 슬라이스 6.5b → Codex B-2 fix: row만으론 같은 row의 다중 match 순회 불가 →
    /// (row, col) 튜플로 확장. next는 같은 row의 다음 col → 다음 row → ...로 진행.
    last_match_abs: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
struct MouseSelection {
    pane: PaneId,
    /// caret 좌표 (abs_row, caret_col). Phase 5: viewport-local row → abs_row(= term.top_visible_abs()
    /// + viewport_row) 기반으로 변경. 스크롤해도 selection이 텍스트와 같이 움직이게 함.
    /// caret_col은 0..=cols (글자 사이 boundary).
    /// word/line selection의 경우 cell index를 caret 변환 (start=cell_start, end=cell_end+1).
    anchor: (usize, usize),
    head: (usize, usize),
    dragging: bool,
    /// 빈 cell에서 drag 시작한 경우 line whole 모드 — drag 이동 시 head를 row 단위로 line 전체.
    /// 글자 cell에서 시작은 기존 char-level (line_drag=false).
    line_drag: bool,
}

/// Phase 5: viewport-local caret → abs caret. abs_row = top_visible_abs + viewport_row.
fn caret_to_abs(term: &Term, viewport_caret: (usize, usize)) -> (usize, usize) {
    let top_abs = term.top_visible_abs() as usize;
    (top_abs.saturating_add(viewport_caret.0), viewport_caret.1)
}

/// Phase 5: abs caret → viewport caret. abs_row < top_abs면 row=0(viewport 위 clip),
/// > top_abs + rows이면 row=rows(viewport 아래 clip). caret_col은 그대로.
fn abs_to_viewport_caret(term: &Term, abs_caret: (usize, usize)) -> (usize, usize) {
    let top_abs = term.top_visible_abs() as usize;
    let rows = term.rows();
    let row = abs_caret
        .0
        .saturating_sub(top_abs)
        .min(rows.saturating_sub(1).max(0));
    (row, abs_caret.1)
}

#[derive(Clone, Debug)]
struct MouseClick {
    pane: PaneId,
    cell: (usize, usize),
    count: u8,
    at: Instant,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, config: Config) -> Self {
        Self {
            state: None,
            pending_window: None,
            startup_waited_once: false,
            proxy,
            config,
        }
    }

    /// M12-5 회귀 fix v3: 첫 winit `Resized` 받은 후 그 size로 PTY spawn.
    /// startup SIGWINCH 0회 보장. Codex thread 019e1659.
    fn finish_startup(&mut self, size: PhysicalSize<u32>) {
        let Some(window) = self.pending_window.take() else {
            return;
        };
        log::info!(
            "startup initial resize: physical={}x{} inner_now={}x{}",
            size.width,
            size.height,
            window.inner_size().width,
            window.inner_size().height,
        );
        let state = pollster::block_on(AppState::new_with_size(
            window,
            self.proxy.clone(),
            self.config.clone(),
            size,
        ))
        .expect("AppState::new");
        state.window.focus_window();
        // Phase 3 step 2b/3: NSVisualEffectView를 winit_view sibling으로 추가.
        // M-P3-2a에서 WgpuOverlay sibling이 attach 성공한 경우만 NSVE 부착 — Codex 리뷰
        // 1순위 fix: overlay fallback path(기존 winit_view.layer를 wgpu가 점유)에서
        // NSVE를 attach하면 이전 실패 모드(텍스트 가림) 회귀.
        // 활성화 정책: PJ001_NO_BACKDROP=1/true/yes env > config.backdrop_enabled=false >
        // default ON. env가 set돼 있어도 값이 0/false/no/공백이면 default 적용 (Codex 권).
        #[cfg(target_os = "macos")]
        let _backdrop_attach = {
            let env_off = std::env::var("PJ001_NO_BACKDROP")
                .ok()
                .map(|v| {
                    let s = v.trim().to_ascii_lowercase();
                    matches!(s.as_str(), "1" | "true" | "yes")
                })
                .unwrap_or(false);
            let config_off = matches!(self.config.backdrop_enabled, Some(false));
            let overlay_ok = state.overlay_attach.is_some();
            if env_off {
                log::info!("macos_backdrop: skipped via PJ001_NO_BACKDROP env");
                None
            } else if config_off {
                log::info!("macos_backdrop: skipped via config [backdrop] enabled=false");
                None
            } else if !overlay_ok {
                log::warn!(
                    "macos_backdrop: skipped — WgpuOverlay attach failed (fallback path). NSVE를 attach하면 텍스트 가림 회귀 위험"
                );
                None
            } else {
                let palette = state.renderer.palette();
                macos_backdrop::attach_visual_effect(&state.window, &palette)
            }
        };
        // IME 이벤트 활성화. winit 0.30.13에서 macOS set_ime_purpose는 no-op
        // (window_delegate.rs:1569). Terminal/Normal 둘 다 동일하나 의도 표기.
        state.window.set_ime_allowed(true);
        state.window.set_ime_purpose(ImePurpose::Terminal);
        // macOS first-key escape 워크어라운드 — NSTextInputContext.activate() 직접 호출로
        // IME를 즉시 wake-up. 입력 소스 전환 직후 첫 자모가 KeyboardInput으로 escape되는
        // winit 동작 회피 (Codex thread 019e2491).
        #[cfg(target_os = "macos")]
        macos_ime::wake_input_context(&state.window);
        let window = state.window.clone();
        self.state = Some(state);
        window.request_redraw();
    }
}

/// M12-5: PaneViewport의 단일 생성 진입점. M11 `pane_layouts` rename.
/// M13에서 BSP 기반 재구현 시 같은 시그니처(또는 LayoutNode 받는 형태)로 확장.
/// `count <= 2` 가정은 M13 진입 직전까지만 유효 — M13에서 N-pane 일반화.
#[cfg(test)]
fn compute_viewports(
    size: PhysicalSize<u32>,
    cell: crate::render::CellMetrics,
    count: usize,
) -> Vec<PaneViewport> {
    debug_assert!(
        count <= 2,
        "compute_viewports currently supports at most two panes (M13 BSP에서 N-pane 일반화)"
    );
    let raw_rows = (size.height / cell.height).max(1) as usize;
    if count <= 1 {
        let cols = (size.width / cell.width).max(1) as usize;
        return vec![PaneViewport {
            cols,
            rows: raw_rows,
            col_offset: 0,
            row_offset: 0,
            status_row: None,
            x_px: 0,
            y_px: 0,
            width_px: size.width,
            height_px: size.height,
            gutter_px: 0,
            content_x_px: 0,
        }];
    }

    let left = PaneId(0);
    let right = PaneId(1);
    let root = Layout::Split {
        axis: SplitAxis::Vertical,
        ratio: SplitRatio::half(),
        primary: Box::new(Layout::Pane(left)),
        secondary: Box::new(Layout::Pane(right)),
    };
    let viewports = layout::compute_viewports(&root, size, cell);
    vec![viewports[&left], viewports[&right]]
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // M12-5 회귀 fix v3: state도 pending_window도 없을 때만 새 window 생성.
        if self.state.is_some() || self.pending_window.is_some() {
            return;
        }
        // Phase 3 step 1: transparent window 배관. wgpu surface가 alpha=1.0인 동안은
        // 시각 변화 0. step 2(NSVisualEffectView attach) + step 3(테마별 bg alpha)부터
        // 윈도우 뒤 데스크톱이 vibrancy로 비침.
        let attrs = Window::default_attributes()
            .with_title("pj001")
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(960.0, 600.0))
            .with_min_inner_size(LogicalSize::new(
                MIN_WINDOW_WIDTH as f64,
                MIN_WINDOW_HEIGHT as f64,
            ));
        let window = Arc::new(event_loop.create_window(attrs).expect("create_window"));
        window.set_min_inner_size(Some(LogicalSize::new(
            MIN_WINDOW_WIDTH as f64,
            MIN_WINDOW_HEIGHT as f64,
        )));
        // PTY spawn은 첫 Resized에서. window는 pending에 보관.
        self.pending_window = Some(window);
        self.startup_waited_once = false;
        event_loop.set_control_flow(ControlFlow::Poll);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // M12-5 회귀 fix v3: state=None일 때 첫 Resized만 startup trigger로 소비.
        if self.state.is_none() {
            if let WindowEvent::Resized(size) = event {
                self.finish_startup(size);
            }
            return;
        }
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
            }
            WindowEvent::Focused(focused) => {
                state.focused = focused;
                if focused {
                    // 포커스 회복 시 즉시 cursor 보이기 (다음 blink phase 기다리지 않음).
                    state.cursor_visible = true;
                    // 포커스 회복 시점에 입력 소스가 한국어로 바뀌었을 가능성 → IME wake-up
                    // 재호출. winit set_ime_allowed가 idempotent라 다른 trigger 필요.
                    #[cfg(target_os = "macos")]
                    macos_ime::wake_input_context(&state.window);
                }
                state.window.request_redraw();
                // M10-3: focus reporting on이면 PTY로 송신. lock drop 후 write.
                let idx = state.active_index();
                let send: Option<&[u8]> = {
                    let term = state.session_for_pane_idx(idx).term.lock().unwrap();
                    if term.focus_reporting() {
                        Some(if focused { b"\x1b[I" } else { b"\x1b[O" })
                    } else {
                        None
                    }
                };
                if let Some(bytes) = send {
                    if let Err(e) = state.session_for_pane_idx_mut(idx).pty.write(bytes) {
                        log::warn!("focus report write: {e}");
                    }
                }
            }
            WindowEvent::Resized(size) => {
                // M17-5 coalesce: 즉시 resize 안 하고 누적. about_to_wait에서 마지막 size 한 번 처리.
                state.pending_resize = Some(size);
                event_loop.set_control_flow(ControlFlow::Poll);
            }
            WindowEvent::Moved(position) => {
                log::debug!(
                    "window moved: position={:?} scale={} stored_scale={}",
                    position,
                    state.window.scale_factor(),
                    state.current_scale_factor,
                );
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                log::debug!(
                    "scale factor changed: old_scale={} new_scale={}",
                    state.current_scale_factor,
                    scale_factor,
                );
                state.log_window_frame("scale-factor-changed");
                state.prepare_scale_factor_change(scale_factor);
                event_loop.set_control_flow(ControlFlow::Poll);
            }
            WindowEvent::Occluded(false) => {
                state.window.request_redraw();
            }
            WindowEvent::Occluded(true) => {}
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::CursorMoved { position, .. } => {
                state.last_mouse_pos = Some(position);
                // 창 밖에서 button release되면 MouseInput Released를 못 받을 수 있다.
                // 다시 들어왔을 때 dragging=true + 어떤 버튼도 안 잡혀있으면 release 누락 →
                // selection drag 종료.
                state.reconcile_lost_button_release();
                if state.dragging_divider.is_some() {
                    state.drag_divider_to_mouse();
                } else if !state.modifiers.shift_key()
                    && state.try_report_mouse_motion(state.mouse_buttons_held != 0)
                {
                    // 마우스 reporting이 이벤트를 흡수.
                } else if state.update_selection_to_mouse() {
                    state.window.request_redraw();
                }
                state.update_mouse_cursor();
            }
            WindowEvent::CursorLeft { .. } => {
                state.last_mouse_pos = None;
                state.dragging_divider = None;
                state.window.set_cursor(CursorIcon::Default);
            }
            WindowEvent::DroppedFile(path) => {
                state.handle_dropped_file(&path);
            }
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Left,
                ..
            } => {
                let pressed = matches!(button_state, ElementState::Pressed);
                // 슬라이스 6.6b: button_held 추적 (reporting 흡수와 무관하게 일관 상태).
                state.track_mouse_button(pressed, 0);
                // Left release는 어떤 분기로 빠져도(hyperlink/mouse reporting/PTY 흡수 등)
                // selection.dragging이 stuck 되지 않게 즉시 finish. 두 번 호출돼도 idempotent
                // (was_dragging=false면 no-op).
                if !pressed {
                    state.finish_selection_drag();
                    state.dragging_divider = None;
                }
                // 슬라이스 6.3c: Cmd+click on hyperlink cell → 브라우저로 OSC 8 URI 열기.
                if pressed && state.modifiers.super_key() && !state.modifiers.shift_key() {
                    if state.try_open_hyperlink_at_mouse() {
                        return;
                    }
                }
                // 슬라이스 6.6: mouse reporting 활성 + Shift 미보유 → PTY로 전달.
                // Shift 보유 시 selection 모드로 우회 (xterm/iterm 표준).
                if !state.modifiers.shift_key() {
                    if state.try_report_mouse_button(pressed, 0) {
                        return;
                    }
                }
                match button_state {
                    ElementState::Pressed => {
                        if let Some(tab_id) = state.tab_at_mouse() {
                            state.selection = None;
                            state.set_active_tab(tab_id);
                            return;
                        }
                        if let Some(hit) = state.divider_hit_at_mouse() {
                            state.selection = None;
                            state.dragging_divider = Some(hit);
                            state.update_mouse_cursor();
                            return;
                        }
                        if let Some((pane_id, cell)) = state.pane_cell_at_mouse() {
                            // caret은 mouse pos 기준. caret 못 얻으면 cell의 왼쪽 boundary fallback.
                            let caret = state
                                .pane_caret_at_mouse()
                                .map(|(_, c)| c)
                                .unwrap_or(cell);
                            state.set_active(pane_id);
                            state.start_selection_at(pane_id, cell, caret);
                            state.window.request_redraw();
                        } else if let Some(pane_id) = state.pane_at_mouse(true) {
                            state.set_active(pane_id);
                            state.selection = None;
                        }
                    }
                    ElementState::Released => {
                        state.dragging_divider = None;
                        state.finish_selection_drag();
                        state.update_mouse_cursor();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Right,
                ..
            } => {
                let pressed = matches!(button_state, ElementState::Pressed);
                state.track_mouse_button(pressed, 2);
                if !state.modifiers.shift_key() {
                    if state.try_report_mouse_button(pressed, 2) {
                        return;
                    }
                }
                if pressed {
                    if let Some((pane_id, _)) = state.pane_cell_at_mouse() {
                        state.set_active(pane_id);
                        if state.selection.is_some() {
                            state.handle_copy();
                        }
                        state.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Middle,
                ..
            } => {
                let pressed = matches!(button_state, ElementState::Pressed);
                state.track_mouse_button(pressed, 1);
                if !state.modifiers.shift_key() {
                    let _ = state.try_report_mouse_button(pressed, 1);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // trackpad swipe / 마우스 휠로 scrollback 스크롤.
                // delta y > 0 = 손가락 위로 = scrollback 위로(view_offset 증가).
                let cell_h = state.renderer.cell_metrics().height as f32;
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as isize,
                    MouseScrollDelta::PixelDelta(pos) => {
                        if cell_h > 0.0 {
                            (pos.y / cell_h as f64) as isize
                        } else {
                            0
                        }
                    }
                };
                if lines != 0 {
                    let target = if state.last_mouse_pos.is_some() {
                        state.pane_index_at_mouse(false)
                    } else {
                        Some(state.active_index())
                    };
                    if let Some(idx) = target {
                        if let Ok(mut term) = state.session_for_pane_idx(idx).term.lock() {
                            term.scroll_view_by(lines);
                        }
                        state.window.request_redraw();
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                use winit::event::Ime;
                match ime {
                    Ime::Preedit(s, _range) => {
                        log::debug!("ime: Preedit({:?})", s);
                        state.preedit = if s.is_empty() { None } else { Some(s) };
                        state.window.request_redraw();
                    }
                    Ime::Commit(s) => {
                        log::debug!("ime: Commit({:?})", s);
                        state.preedit = None;
                        let idx = state.active_index();
                        if let Err(e) = state.session_for_pane_idx_mut(idx).pty.write(s.as_bytes())
                        {
                            log::warn!("pty write (ime commit): {e}");
                        }
                        state.window.request_redraw();
                    }
                    Ime::Disabled => {
                        log::info!("ime: Disabled");
                        if state.preedit.is_some() {
                            state.preedit = None;
                            state.window.request_redraw();
                        }
                    }
                    Ime::Enabled => {
                        log::info!("ime: Enabled");
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                log::debug!(
                    "key: state={:?} logical={:?} text={:?}",
                    event.state,
                    event.logical_key,
                    event.text
                );
                use winit::keyboard::{Key, NamedKey, PhysicalKey};
                // 슬라이스 6.5/6.5b: find 입력 모드 — Cmd+F 활성 중 키 입력 흡수.
                // Esc = cancel, Enter = next match, Shift+Enter = prev match.
                // Cmd-조합 키는 swallow 안 함 (fall through하여 Cmd 핸들러 실행 — Cmd+V/Cmd+Q 등 정상 동작).
                if event.state == ElementState::Pressed
                    && state.pending_find.is_some()
                    && !state.modifiers.super_key()
                    && !state.modifiers.control_key()
                {
                    let handled = match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            state.cancel_find();
                            true
                        }
                        Key::Named(NamedKey::Enter) => {
                            if state.modifiers.shift_key() {
                                state.find_prev();
                            } else {
                                state.find_next();
                            }
                            true
                        }
                        Key::Named(NamedKey::Backspace) => {
                            state.find_backspace();
                            true
                        }
                        Key::Character(s) => {
                            for ch in s.chars() {
                                state.find_append_char(ch);
                            }
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return;
                    }
                }
                if event.state == ElementState::Pressed && state.pending_quick_spawn.is_some() {
                    let handled = match &event.logical_key {
                        Key::Character(s) => state.finish_quick_spawn(s),
                        Key::Named(NamedKey::Escape) => {
                            state.cancel_quick_spawn("escape");
                            true
                        }
                        _ => {
                            state.cancel_quick_spawn("unsupported key");
                            true
                        }
                    };
                    if handled {
                        return;
                    }
                }
                // M8-6: macOS Cmd 단축키 처리 — Cmd+Q/W = 종료, Cmd+V = paste, 그 외 swallow.
                if event.state == ElementState::Pressed && state.modifiers.super_key() {
                    let physical_code = match event.physical_key {
                        PhysicalKey::Code(code) => Some(code),
                        PhysicalKey::Unidentified(_) => None,
                    };
                    if state.modifiers.shift_key() {
                        if let Key::Named(named) = &event.logical_key {
                            let handled = match named {
                                NamedKey::ArrowLeft => state.adjust_active_split(
                                    SplitAxis::Vertical,
                                    RatioDirection::ShrinkActive,
                                ),
                                NamedKey::ArrowRight => state.adjust_active_split(
                                    SplitAxis::Vertical,
                                    RatioDirection::GrowActive,
                                ),
                                NamedKey::ArrowUp => state.adjust_active_split(
                                    SplitAxis::Horizontal,
                                    RatioDirection::ShrinkActive,
                                ),
                                NamedKey::ArrowDown => state.adjust_active_split(
                                    SplitAxis::Horizontal,
                                    RatioDirection::GrowActive,
                                ),
                                _ => false,
                            };
                            if handled {
                                return;
                            }
                        }
                    }
                    if !state.modifiers.alt_key() && !state.modifiers.control_key() {
                        if let Key::Named(named) = &event.logical_key {
                            if let Some(bytes) = cmd_named_key_bytes(named) {
                                let idx = state.active_index();
                                if let Err(e) = state.session_for_pane_idx_mut(idx).pty.write(bytes)
                                {
                                    log::warn!("cmd+arrow write failed: {e}");
                                }
                                return;
                            }
                            match named {
                                NamedKey::ArrowUp => {
                                    let idx = state.active_index();
                                    if let Ok(mut term) =
                                        state.session_for_pane_idx(idx).term.lock()
                                    {
                                        term.scroll_view_by(isize::MAX);
                                    }
                                    state.window.request_redraw();
                                    return;
                                }
                                NamedKey::ArrowDown => {
                                    let idx = state.active_index();
                                    if let Ok(mut term) =
                                        state.session_for_pane_idx(idx).term.lock()
                                    {
                                        term.snap_to_bottom();
                                    }
                                    state.window.request_redraw();
                                    return;
                                }
                                _ => {}
                            }
                        }
                    }
                    let lower = match &event.logical_key {
                        Key::Character(s) => Some(s.to_lowercase()),
                        _ => None,
                    };
                    match cmd_shortcut(
                        lower.as_deref(),
                        physical_code,
                        state.modifiers.shift_key(),
                        state.modifiers.alt_key(),
                    ) {
                        Some(CmdShortcut::TabOrdinal(ordinal)) => {
                            state.focus_tab_by_ordinal(ordinal);
                            return;
                        }
                        Some(CmdShortcut::PaneOrdinal(ordinal)) => {
                            state.focus_pane_by_ordinal(ordinal);
                            return;
                        }
                        Some(CmdShortcut::PrevTab) => {
                            state.focus_adjacent_tab(false);
                            return;
                        }
                        Some(CmdShortcut::NextTab) => {
                            state.focus_adjacent_tab(true);
                            return;
                        }
                        Some(CmdShortcut::PrevPane) => {
                            state.focus_adjacent_pane(false);
                            return;
                        }
                        Some(CmdShortcut::NextPane) => {
                            state.focus_adjacent_pane(true);
                            return;
                        }
                        Some(CmdShortcut::SplitVertical) => {
                            if let Err(e) = state.split_active(SplitAxis::Vertical) {
                                log::warn!("cmd+d split failed: {e}");
                            }
                            return;
                        }
                        Some(CmdShortcut::SplitHorizontal) => {
                            if let Err(e) = state.split_active(SplitAxis::Horizontal) {
                                log::warn!("cmd+d split failed: {e}");
                            }
                            return;
                        }
                        Some(CmdShortcut::NewTab) => {
                            if let Err(e) = state.create_tab() {
                                log::warn!("cmd+t new tab failed: {e}");
                            }
                            return;
                        }
                        Some(CmdShortcut::NewPane) => {
                            if let Err(e) = state.split_active(SplitAxis::Vertical) {
                                log::warn!("cmd+n new pane failed: {e}");
                            }
                            return;
                        }
                        Some(CmdShortcut::QuickSpawnStart) => {
                            state.start_quick_spawn();
                            return;
                        }
                        Some(CmdShortcut::RespawnSession) => {
                            if let Err(e) = state.respawn_active() {
                                log::warn!("cmd+r respawn failed: {e}");
                            }
                            return;
                        }
                        Some(CmdShortcut::FontZoomIn) => {
                            state.set_logical_font_size(state.logical_font_size + FONT_SIZE_STEP);
                            return;
                        }
                        Some(CmdShortcut::FontZoomOut) => {
                            state.set_logical_font_size(state.logical_font_size - FONT_SIZE_STEP);
                            return;
                        }
                        Some(CmdShortcut::FontZoomReset) => {
                            state.set_logical_font_size(DEFAULT_FONT_SIZE);
                            return;
                        }
                        Some(CmdShortcut::CloseTab) => {
                            state.apply_close_decision(
                                event_loop,
                                close_decision(
                                    CmdShortcut::CloseTab,
                                    state.active_tab().panes.len(),
                                    state.tabs.len(),
                                ),
                                "cmd+shift+w",
                            );
                            return;
                        }
                        Some(CmdShortcut::ClosePaneOrTab) => {
                            state.apply_close_decision(
                                event_loop,
                                close_decision(
                                    CmdShortcut::ClosePaneOrTab,
                                    state.active_tab().panes.len(),
                                    state.tabs.len(),
                                ),
                                "cmd+w",
                            );
                            return;
                        }
                        Some(CmdShortcut::Quit) => {
                            log::info!("cmd+q: exit");
                            event_loop.exit();
                            return;
                        }
                        Some(CmdShortcut::Copy) => {
                            state.handle_copy();
                            return;
                        }
                        Some(CmdShortcut::Paste) => {
                            state.handle_paste();
                            return;
                        }
                        Some(CmdShortcut::Find) => {
                            state.start_find();
                            return;
                        }
                        Some(CmdShortcut::SelectAll) => {
                            state.handle_select_all();
                            return;
                        }
                        Some(CmdShortcut::ClearBuffer) => {
                            state.clear_active_buffer(false);
                            return;
                        }
                        Some(CmdShortcut::ClearScrollback) => {
                            state.clear_active_buffer(true);
                            return;
                        }
                        None => {}
                    }
                    // 그 외 Cmd+key: swallow (PTY 안 보냄).
                    log::debug!("swallowed Cmd+key: {:?}", event.logical_key);
                    return;
                }
                // M8-5: PageUp/Down 분기 — alt screen이면 PTY 전송, main이면 scrollback.
                if event.state == ElementState::Pressed {
                    if let Key::Named(named @ (NamedKey::PageUp | NamedKey::PageDown)) =
                        &event.logical_key
                    {
                        let idx = state.active_index();
                        let alt = state
                            .session_for_pane_idx(idx)
                            .term
                            .lock()
                            .map(|t| t.is_alt_screen())
                            .unwrap_or(false);
                        if alt {
                            // alt screen: encode_named_key가 byte 반환 → 일반 PTY 송신 흐름.
                            log_page_dispatch_once("PTY (alt screen)");
                        } else {
                            // main screen: scrollback view 스크롤. PTY 안 보냄.
                            if let Ok(mut term) = state.session_for_pane_idx(idx).term.lock() {
                                let page = term.rows().saturating_sub(1).max(1) as isize;
                                let delta = if matches!(named, NamedKey::PageUp) {
                                    page
                                } else {
                                    -page
                                };
                                term.scroll_view_by(delta);
                            }
                            state.window.request_redraw();
                            log_page_dispatch_once("scrollback (main screen)");
                            return;
                        }
                    }
                }
                // type-to-snap: scrollback view 활성 시 일반 키 누름 → bottom 스냅.
                // 단, Cmd/Shift/Ctrl/Alt/Fn 같은 modifier 단독 press는 PTY로 보낼 키 입력이
                // 아니므로 snap 안 함 (Derek 보고: scroll 올린 뒤 Cmd 누르면 맨 아래 내려가는 버그).
                // 또한 일반 키 input 시 selection 자동 해제 (macOS textfield 표준).
                if event.state == ElementState::Pressed && !is_modifier_only_key(&event.logical_key)
                {
                    let idx = state.active_index();
                    if let Ok(mut term) = state.session_for_pane_idx(idx).term.lock() {
                        if term.view_offset() > 0 {
                            term.snap_to_bottom();
                            state.window.request_redraw();
                        }
                    }
                    if state.selection.is_some() {
                        state.selection = None;
                        state.window.request_redraw();
                    }
                }
                // single lock snapshot (advisor 가이드).
                let idx = state.active_index();
                let mode = {
                    let term = state.session_for_pane_idx(idx).term.lock().unwrap();
                    input::InputMode {
                        cursor_keys_application: term.cursor_keys_application(),
                        alt_screen: term.is_alt_screen(),
                        modifiers: state.modifiers,
                    }
                };
                if let Some(bytes) = input::encode_key(&event, mode) {
                    if let Err(e) = state.session_for_pane_idx_mut(idx).pty.write(&bytes) {
                        log::warn!("pty write: {e}");
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // M12-5 회귀 fix v3: state=None일 때 fallback. 첫 cycle은 wait,
        // 두 번째 cycle에도 Resized 안 오면 inner_size로 spawn (앱 안 뜨는 케이스 회피).
        if self.state.is_none() {
            if let Some(window) = self.pending_window.as_ref() {
                if self.startup_waited_once {
                    let size = window.inner_size();
                    self.finish_startup(size);
                } else {
                    self.startup_waited_once = true;
                    event_loop.set_control_flow(ControlFlow::Poll);
                }
            }
            return;
        }
        let Some(state) = self.state.as_mut() else {
            return;
        };
        // M17-5: 누적된 resize를 한 번만 처리.
        if let Some(size) = state.pending_resize.take() {
            state.resize(size);
        }
        if state.quick_spawn_timed_out() {
            state.cancel_quick_spawn("timeout");
        }
        let quick_spawn_deadline = state.quick_spawn_deadline();
        // 깜빡임 정지 조건:
        // - 창 비활성 (focused=false)
        // - cursor.blinking=false (DECSCUSR steady)
        // 정지 시 cursor_visible=true 유지 (계속 보임), Wait로 idle.
        if !state.focused || !state.cursor_blinking_cache {
            if !state.cursor_visible {
                state.cursor_visible = true;
                state.window.request_redraw();
            }
            if let Some(deadline) = quick_spawn_deadline {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            return;
        }
        let blink = Duration::from_millis(CURSOR_BLINK_MS);
        let now = Instant::now();
        let mut next = if now.duration_since(state.last_blink) >= blink {
            state.cursor_visible = !state.cursor_visible;
            state.last_blink = now;
            state.window.request_redraw();
            now + blink
        } else {
            state.last_blink + blink
        };
        if let Some(deadline) = quick_spawn_deadline {
            next = next.min(deadline);
        }
        // Phase 5: continuous auto-scroll — drag 중 마우스 viewport 밖이면 마우스 멈춰 있어도
        // 50ms마다 sticky scroll. update_selection_to_mouse가 자체 throttle 처리.
        if state.is_auto_scrolling() {
            if state.update_selection_to_mouse() {
                state.window.request_redraw();
            }
            let scroll_next = Instant::now() + Duration::from_millis(50);
            next = next.min(scroll_next);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::SessionRepaint(session_id) => {
                log::debug!("repaint requested by session {}", session_id.0);
                if let Some(state) = &self.state {
                    // M12-4 design §3.3: 방어적 lookup. unknown session은 debug log + ignore.
                    if !state.sessions.contains_key(&session_id) {
                        log::debug!(
                            "SessionRepaint for unknown session {:?}, ignore",
                            session_id
                        );
                        return;
                    }
                    if state
                        .active_tab()
                        .panes
                        .iter()
                        .any(|p| p.session == session_id)
                    {
                        state.window.request_redraw();
                    }
                }
            }
            UserEvent::SessionExited { id, code } => {
                log::info!("session {} child exited (code={code})", id.0);
                if let Some(state) = &mut self.state {
                    if !state.sessions.contains_key(&id) {
                        log::debug!("SessionExited for unknown session {:?}, ignore", id);
                        return;
                    }
                    // Codex v2 권고: 단일 pane exit 분기 전에 session 상태 일관성 먼저.
                    if let Some(s) = state.sessions.get_mut(&id) {
                        s.alive = false;
                        s.exit_code = Some(code);
                    }
                    state.emit_lifecycle(LifecycleEvent::SessionExited {
                        session_id: id,
                        code,
                    });
                    if state.all_sessions_dead() {
                        event_loop.exit();
                        return;
                    }
                    let Some(tab_idx) = state.tab_index_for_session(id) else {
                        state.window.request_redraw();
                        return;
                    };
                    let tab_is_active = state.tabs[tab_idx].id == state.active_tab;
                    if state.tabs[tab_idx].panes.len() <= 1 {
                        if state.tabs.len() <= 1 {
                            event_loop.exit();
                        } else if tab_is_active {
                            state.close_active_tab();
                        } else {
                            state.remove_tab_at(tab_idx);
                            state.window.request_redraw();
                        }
                    } else if tab_is_active {
                        let active_session = state
                            .active_tab()
                            .panes
                            .iter()
                            .find(|p| p.id == state.active_tab().active)
                            .map(|p| p.session);
                        if active_session == Some(id) {
                            if let Some(next) = state.next_alive_pane_id() {
                                state.set_active(next);
                            }
                        }
                        state.window.request_redraw();
                    }
                }
            }
            UserEvent::SessionPtyError { id, message } => {
                log::error!("session {} pty error: {message}", id.0);
                if let Some(state) = &mut self.state {
                    if !state.sessions.contains_key(&id) {
                        log::debug!("SessionPtyError for unknown session {:?}, ignore", id);
                        return;
                    }
                    // Codex v2 권고: session 상태 일관성 먼저.
                    if let Some(s) = state.sessions.get_mut(&id) {
                        s.alive = false;
                    }
                    if state.all_sessions_dead() {
                        event_loop.exit();
                        return;
                    }
                    let Some(tab_idx) = state.tab_index_for_session(id) else {
                        state.window.request_redraw();
                        return;
                    };
                    let tab_is_active = state.tabs[tab_idx].id == state.active_tab;
                    if state.tabs[tab_idx].panes.len() <= 1 {
                        if state.tabs.len() <= 1 {
                            event_loop.exit();
                        } else if tab_is_active {
                            state.close_active_tab();
                        } else {
                            state.remove_tab_at(tab_idx);
                            state.window.request_redraw();
                        }
                    } else if tab_is_active {
                        let active_session = state
                            .active_tab()
                            .panes
                            .iter()
                            .find(|p| p.id == state.active_tab().active)
                            .map(|p| p.session);
                        if active_session == Some(id) {
                            if let Some(next) = state.next_alive_pane_id() {
                                state.set_active(next);
                            }
                        }
                        state.window.request_redraw();
                    }
                }
            }
        }
    }
}

impl AppState {
    fn active_tab_index(&self) -> usize {
        self.tabs
            .iter()
            .position(|tab| tab.id == self.active_tab)
            .unwrap_or(0)
    }

    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab_index()]
    }

    fn active_tab_mut(&mut self) -> &mut Tab {
        let idx = self.active_tab_index();
        &mut self.tabs[idx]
    }

    fn active_index(&self) -> usize {
        self.active_tab()
            .panes
            .iter()
            .position(|p| p.id == self.active_tab().active)
            .unwrap_or(0)
    }

    /// Phase 4b: 현재 시각적으로 Block UI를 켜야 하는지. block_mode=Auto && 어느 session이라도
    /// block_capable=true. PoC v1은 전역 boolean — 다음 단계에서 pane별로 세분화 가능.
    #[allow(dead_code)]
    fn block_visual_active(&self) -> bool {
        if self.block_mode != BlockMode::Auto {
            return false;
        }
        self.sessions
            .values()
            .any(|s| s.term.lock().map(|t| t.block_capable()).unwrap_or(false))
    }

    fn pane_index_at_mouse(&self, include_status_row: bool) -> Option<usize> {
        let pos = self.last_mouse_pos?;
        let cell = self.renderer.cell_metrics();
        if cell.width == 0 || cell.height == 0 || pos.x < 0.0 || pos.y < 0.0 {
            return None;
        }

        let col = (pos.x / cell.width as f64).floor() as usize;
        let row = (pos.y / cell.height as f64).floor() as usize;
        let sessions = &self.sessions;
        self.active_tab().panes.iter().position(|pane| {
            if !sessions
                .get(&pane.session)
                .map(|s| s.alive)
                .unwrap_or(false)
            {
                return false;
            }
            let vp = &pane.viewport;
            let col_start = vp.col_offset;
            let col_end = vp.col_offset + vp.cols;
            let row_start = vp.row_offset;
            let row_end = vp.row_offset + vp.rows;
            let in_content = row >= row_start && row < row_end;
            let in_status =
                include_status_row && vp.status_row.is_some_and(|status_row| row == status_row);
            col >= col_start && col < col_end && (in_content || in_status)
        })
    }

    /// M12-3: 모든 session이 dead면 true.
    fn all_sessions_dead(&self) -> bool {
        let sessions = &self.sessions;
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .all(|p| !sessions.get(&p.session).map(|s| s.alive).unwrap_or(false))
    }

    /// M12-3: 다음 alive pane을 찾음.
    fn next_alive_pane_id(&self) -> Option<PaneId> {
        let sessions = &self.sessions;
        self.active_tab()
            .panes
            .iter()
            .find(|p| sessions.get(&p.session).map(|s| s.alive).unwrap_or(false))
            .map(|p| p.id)
    }

    /// M12-3: PaneId → SessionId 매핑. M12-4 재작성에서 inline으로 옮겨갔지만 M13 BSP layout
    /// traversal에서 PaneId 기반 lookup에 다시 활용 예정.
    #[allow(dead_code)]
    fn session_id_for_pane(&self, pane: PaneId) -> Option<SessionId> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .find(|p| p.id == pane)
            .map(|p| p.session)
    }

    fn tab_index_for_session(&self, session_id: SessionId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.panes.iter().any(|pane| pane.session == session_id))
    }

    fn sync_active_tab_title(&mut self, title: String) {
        self.active_tab_mut().title = title;
    }

    fn set_active(&mut self, id: PaneId) {
        let sessions = &self.sessions;
        let Some(new_idx) =
            self.active_tab().panes.iter().position(|p| {
                p.id == id && sessions.get(&p.session).map(|s| s.alive).unwrap_or(false)
            })
        else {
            return;
        };
        if self.active_tab().active == id {
            return;
        }

        let old_idx = self.active_index();
        if self.focused {
            self.send_focus_report(old_idx, false);
            self.send_focus_report(new_idx, true);
        }

        let title = self.session_for_pane_idx(new_idx).title.clone();
        self.active_tab_mut().active = id;
        self.sync_active_tab_title(title.clone());
        self.window.set_title(&format!("{} — pj001", title));
        self.cursor_visible = true;
        self.last_ime_cursor = None;
        self.preedit = None;
        self.window.request_redraw();
    }

    fn send_focus_report(&mut self, idx: usize, focused: bool) {
        if idx >= self.active_tab().panes.len() {
            return;
        }
        let session = self.session_for_pane_idx_mut(idx);
        let send = session
            .term
            .lock()
            .map(|term| term.focus_reporting())
            .unwrap_or(false);
        if send {
            let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            if let Err(e) = session.pty.write(bytes) {
                log::warn!("focus report write (session {}): {e}", session.id.0);
            }
        }
    }

    fn apply_close_decision(
        &mut self,
        event_loop: &ActiveEventLoop,
        decision: CloseDecision,
        label: &str,
    ) {
        match decision {
            CloseDecision::ClosePane => self.close_active_pane(),
            CloseDecision::CloseTab => self.close_active_tab(),
            CloseDecision::Exit => {
                log::info!("{label}: exit last tab");
                event_loop.exit();
            }
        }
    }

    fn emit_lifecycle(&self, event: LifecycleEvent) {
        if let Some(sink) = &self.hooks.lifecycle_sink {
            sink.on_lifecycle(event);
        }
    }

    fn spawn_session_for_pane(
        &mut self,
        pane_id: PaneId,
        spec: SessionSpec,
        viewport: PaneViewport,
    ) -> Result<()> {
        let session_id = self.ids.new_session();
        let term = Arc::new(Mutex::new(Term::new(viewport.cols, viewport.rows)));
        let shell = spec.command.resolve();
        log::info!("pane {} shell: {}", pane_id.0, shell);
        let pty = PtyHandle::spawn(
            &shell,
            PtySize {
                rows: viewport.rows as u16,
                cols: viewport.cols as u16,
                pixel_width: viewport.width_px as u16,
                pixel_height: viewport.height_px as u16,
            },
            term.clone(),
            self.proxy.clone(),
            session_id,
        )?;
        let title = spec.title;
        self.sessions.insert(
            session_id,
            Session {
                id: session_id,
                title: title.clone(),
                command: shell,
                pty,
                term,
                alive: true,
                exit_code: None,
                created_at: Instant::now(),
            },
        );
        self.active_tab_mut().panes.push(Pane {
            id: pane_id,
            session: session_id,
            viewport,
        });
        self.emit_lifecycle(LifecycleEvent::SessionStarted { session_id, title });
        Ok(())
    }

    fn apply_layout_viewports(&mut self) {
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        self.apply_layout_viewports_for_size(size);
    }

    fn apply_layout_viewports_for_size(&mut self, size: PhysicalSize<u32>) {
        let root = self.active_tab().root.clone();
        let gutter_px = compute_block_gutter_px(
            self.block_visual_active(),
            self.renderer.cell_metrics(),
            self.logical_font_size,
        );
        let layouts = compute_tab_viewports(&root, size, self.renderer.cell_metrics(), gutter_px);
        let pane_count = self.active_tab().panes.len();
        for idx in 0..pane_count {
            let pane_id = self.active_tab().panes[idx].id;
            let Some(viewport) = layouts.get(&pane_id).copied() else {
                log::warn!("layout missing viewport for pane {}", pane_id.0);
                continue;
            };
            let prev = self.active_tab().panes[idx].viewport;
            let pty_cell_size_changed = prev.cols != viewport.cols || prev.rows != viewport.rows;
            self.active_tab_mut().panes[idx].viewport = viewport;
            if pty_cell_size_changed {
                let session = self.session_for_pane_idx_mut(idx);
                if let Ok(mut term) = session.term.lock() {
                    term.resize(viewport.cols, viewport.rows);
                }
                let _ = session.pty.resize(PtySize {
                    rows: viewport.rows as u16,
                    cols: viewport.cols as u16,
                    pixel_width: viewport.width_px as u16,
                    pixel_height: viewport.height_px as u16,
                });
            }
        }
    }

    fn split_active(&mut self, axis: SplitAxis) -> Result<()> {
        self.split_active_with_spec(
            axis,
            SessionSpec {
                title: "shell".to_string(),
                command: CommandSpec::Shell,
            },
        )
    }

    fn split_active_with_spec(&mut self, axis: SplitAxis, spec: SessionSpec) -> Result<()> {
        let new_pane = self.ids.new_pane();
        let active = self.active_tab().active;
        let previous_root = self.active_tab().root.clone();
        let mut next_layout = self.active_tab().root.clone();
        if !next_layout.split_pane(active, axis, new_pane) {
            log::warn!("split requested for missing active pane {}", active.0);
            return Ok(());
        }
        self.active_tab_mut().root = next_layout;
        self.update_min_inner_size();
        let size = self.target_inner_size_for_layout();
        if size.width > self.surface_config.width || size.height > self.surface_config.height {
            let _ = self.window.request_inner_size(size);
        }
        let gutter_px = compute_block_gutter_px(
            self.block_visual_active(),
            self.renderer.cell_metrics(),
            self.logical_font_size,
        );
        let layouts = compute_tab_viewports(
            &self.active_tab().root,
            size,
            self.renderer.cell_metrics(),
            gutter_px,
        );
        let Some(viewport) = layouts.get(&new_pane).copied() else {
            log::warn!("split produced no viewport for new pane {}", new_pane.0);
            return Ok(());
        };
        if let Err(error) = self.spawn_session_for_pane(new_pane, spec, viewport) {
            self.active_tab_mut().root = previous_root;
            self.apply_layout_viewports();
            self.window.request_redraw();
            return Err(error);
        }
        self.apply_layout_viewports_for_size(size);
        self.set_active(new_pane);
        Ok(())
    }

    fn start_find(&mut self) {
        self.pending_find = Some(FindState::default());
        self.selection = None;
        self.window.set_title("find: — pj001");
        self.window.request_redraw();
        log::info!("find mode: open");
    }

    fn cancel_find(&mut self) {
        self.pending_find = None;
        self.selection = None;
        let title = self.session_for_pane_idx(self.active_index()).title.clone();
        self.window.set_title(&format!("{} — pj001", title));
        self.window.request_redraw();
        log::info!("find mode: cancel");
    }

    fn find_backspace(&mut self) {
        if let Some(find) = self.pending_find.as_mut() {
            find.query.pop();
            find.last_match_abs = None;
            self.window
                .set_title(&format!("find: {} — pj001", find.query));
            self.find_apply_current_query(0, 0, true);
            self.window.request_redraw();
        }
    }

    fn find_append_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        if let Some(find) = self.pending_find.as_mut() {
            find.query.push(ch);
            find.last_match_abs = None;
            self.window
                .set_title(&format!("find: {} — pj001", find.query));
            self.find_apply_current_query(0, 0, true);
            self.window.request_redraw();
        }
    }

    /// Enter — 마지막 match 다음 위치(같은 row의 col+1 또는 다음 row)부터 forward.
    fn find_next(&mut self) {
        let (start_row, start_col) = self
            .pending_find
            .as_ref()
            .and_then(|f| f.last_match_abs)
            .map(|(r, c)| (r, c.saturating_add(1)))
            .unwrap_or((0, 0));
        self.find_apply_current_query(start_row, start_col, true);
        self.window.request_redraw();
    }

    /// Shift+Enter — 마지막 match 이전 위치부터 backward.
    fn find_prev(&mut self) {
        let (start_row, start_col) = self
            .pending_find
            .as_ref()
            .and_then(|f| f.last_match_abs)
            .unwrap_or((0, 0));
        self.find_apply_current_query(start_row, start_col, false);
        self.window.request_redraw();
    }

    /// 현재 query를 `(start_row, start_col)` 부터 `forward` 방향으로 검색, 발견 시
    /// view에 띄우고 `last_match_abs` 갱신, **match cells에 selection highlight 적용**.
    /// 같은 row 안에서 start_col 이후/이전 occurrence를 먼저 확인 후 다음/이전 row로 진행.
    /// `forward=false`면 start_col 이전 occurrence를 거꾸로 검색.
    fn find_apply_current_query(&mut self, start_row: usize, start_col: usize, forward: bool) {
        let query = match self.pending_find.as_ref() {
            Some(f) if !f.query.is_empty() => f.query.clone(),
            _ => return,
        };
        let idx = self.active_index();
        let pane_id = self.active_tab().panes[idx].id;
        let session = self.session_for_pane_idx(idx);
        let Ok(mut term) = session.term.lock() else {
            return;
        };
        let cols = term.cols();
        let rows = term.rows();
        let scrollback_len = term.scrollback_len();
        let total = scrollback_len + rows;
        if total == 0 {
            return;
        }
        let range: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(start_row.min(total)..total)
        } else {
            Box::new((0..=start_row.min(total.saturating_sub(1))).rev())
        };
        let query_chars: Vec<char> = query.chars().collect();
        let qlen = query_chars.len();
        let mut found: Option<(usize, usize, usize)> = None; // (abs_row, col_start, visible_row)
        for abs in range {
            let (row_chars, visible_row) = if abs < scrollback_len {
                let needed_offset = scrollback_len - abs;
                term.set_view_offset(needed_offset.min(scrollback_len));
                let chars: Vec<char> = (0..cols).map(|c| term.cell(0, c).ch).collect();
                (chars, 0usize)
            } else {
                let row = abs - scrollback_len;
                if row >= rows {
                    continue;
                }
                term.set_view_offset(0);
                let chars: Vec<char> = (0..cols).map(|c| term.cell(row, c).ch).collect();
                (chars, row)
            };
            if qlen == 0 || qlen > row_chars.len() {
                continue;
            }
            // 같은 row에서는 start_col 기준 적용, 다른 row로 넘어가면 전체 검색.
            // forward면 같은 start_row일 때 col >= start_col인 첫 매치, 그 외 row는 첫 매치.
            // backward면 같은 start_row일 때 col < start_col인 마지막 매치, 그 외 row는 마지막 매치.
            let result = if abs == start_row {
                if forward {
                    row_chars
                        .windows(qlen)
                        .enumerate()
                        .skip(start_col)
                        .find(|(_, w)| *w == query_chars.as_slice())
                        .map(|(i, _)| i)
                } else {
                    row_chars
                        .windows(qlen)
                        .enumerate()
                        .take(start_col)
                        .filter(|(_, w)| *w == query_chars.as_slice())
                        .last()
                        .map(|(i, _)| i)
                }
            } else if forward {
                row_chars
                    .windows(qlen)
                    .position(|w| w == query_chars.as_slice())
            } else {
                row_chars
                    .windows(qlen)
                    .enumerate()
                    .filter(|(_, w)| *w == query_chars.as_slice())
                    .last()
                    .map(|(i, _)| i)
            };
            if let Some(col_start) = result {
                found = Some((abs, col_start, visible_row));
                break;
            }
        }
        if found.is_none() {
            term.set_view_offset(0);
        }
        drop(term);
        // selection highlight 적용 — 새 mouse selection처럼 보이지만 dragging=false라서
        // mouse drag 충돌 없음. Esc/cancel 시 clear됨.
        if let Some((abs, col_start, visible_row)) = found {
            let query_len = query.chars().count();
            // caret 모델: head caret = match start + len (last cell의 다음 boundary).
            let head_col = col_start.saturating_add(query_len).min(cols);
            log::info!(
                "find: match abs={} visible_row={} col={}..={} query=\"{}\"",
                abs,
                visible_row,
                col_start,
                head_col,
                query,
            );
            // Phase 5: selection abs row 의미 통일. find가 view_offset 조정 후라 top_visible_abs +
            // visible_row가 정확한 abs caret row.
            let top_abs = self
                .sessions
                .get(&self.active_tab().panes[idx].session)
                .and_then(|s| s.term.lock().ok())
                .map(|t| t.top_visible_abs() as usize)
                .unwrap_or(0);
            let abs_row = top_abs + visible_row;
            self.selection = Some(MouseSelection {
                pane: pane_id,
                anchor: (abs_row, col_start),
                head: (abs_row, head_col),
                dragging: false,
                line_drag: false,
            });
            if let Some(find) = self.pending_find.as_mut() {
                find.last_match_abs = Some((abs, col_start));
            }
        } else {
            log::info!("find: no match query=\"{}\"", query);
            self.selection = None;
            if let Some(find) = self.pending_find.as_mut() {
                find.last_match_abs = None;
            }
        }
    }

    fn start_quick_spawn(&mut self) {
        self.pending_quick_spawn = Some(Instant::now());
        let hint = quick_spawn_hint(&self.quick_spawn_presets);
        self.window.set_title(&format!("{hint} — pj001"));
        self.window.request_redraw();
        log::info!("quick spawn started: {hint}");
    }

    fn cancel_quick_spawn(&mut self, reason: &str) {
        self.pending_quick_spawn = None;
        let title = self.session_for_pane_idx(self.active_index()).title.clone();
        self.window.set_title(&format!("{} — pj001", title));
        self.window.request_redraw();
        log::info!("quick spawn canceled: {reason}");
    }

    fn finish_quick_spawn(&mut self, key: &str) -> bool {
        let Some(started_at) = self.pending_quick_spawn else {
            return false;
        };
        self.pending_quick_spawn = None;
        if started_at.elapsed() > Duration::from_millis(QUICK_SPAWN_TIMEOUT_MS) {
            self.cancel_quick_spawn("timeout");
            return true;
        }
        let Some(ch) = key.chars().next().map(|c| c.to_ascii_lowercase()) else {
            self.cancel_quick_spawn("empty key");
            return true;
        };
        if ch == 'n' {
            self.pending_quick_spawn = Some(started_at);
            log::debug!("quick spawn ignored repeated trigger key");
            return true;
        };
        let Some(spec) = self
            .quick_spawn_presets
            .iter()
            .find(|preset| preset.key.to_ascii_lowercase() == ch)
            .map(|preset| preset.spec.clone())
        else {
            self.cancel_quick_spawn("unmapped key");
            return true;
        };
        if let Err(e) = self.split_active_with_spec(SplitAxis::Vertical, spec) {
            log::warn!("quick spawn failed: {e}");
            let title = self.session_for_pane_idx(self.active_index()).title.clone();
            self.window.set_title(&format!("{} — pj001", title));
        }
        true
    }

    fn quick_spawn_timed_out(&self) -> bool {
        self.pending_quick_spawn
            .is_some_and(|started_at| quick_spawn_elapsed_timed_out(started_at, Instant::now()))
    }

    fn quick_spawn_deadline(&self) -> Option<Instant> {
        self.pending_quick_spawn
            .map(|started_at| started_at + Duration::from_millis(QUICK_SPAWN_TIMEOUT_MS))
    }

    fn respawn_active(&mut self) -> Result<()> {
        let active_idx = self.active_index();
        let pane_id = self.active_tab().panes[active_idx].id;
        let old_session_id = self.active_tab().panes[active_idx].session;
        let viewport = self.active_tab().panes[active_idx].viewport;
        let Some(old_session) = self.sessions.get(&old_session_id) else {
            log::warn!("respawn requested for missing session {}", old_session_id.0);
            return Ok(());
        };
        let command = old_session.command.clone();
        let title = old_session.title.clone();
        let new_session_id = self.ids.new_session();
        let term = Arc::new(Mutex::new(Term::new(viewport.cols, viewport.rows)));
        log::info!(
            "respawn pane {} session {} -> {} command={}",
            pane_id.0,
            old_session_id.0,
            new_session_id.0,
            command,
        );
        let pty = PtyHandle::spawn(
            &command,
            PtySize {
                rows: viewport.rows as u16,
                cols: viewport.cols as u16,
                pixel_width: viewport.width_px as u16,
                pixel_height: viewport.height_px as u16,
            },
            term.clone(),
            self.proxy.clone(),
            new_session_id,
        )?;
        self.sessions.insert(
            new_session_id,
            Session {
                id: new_session_id,
                title: title.clone(),
                command,
                pty,
                term,
                alive: true,
                exit_code: None,
                created_at: Instant::now(),
            },
        );
        self.active_tab_mut().panes[active_idx].session = new_session_id;
        if let Some(mut old_session) = self.sessions.remove(&old_session_id) {
            old_session.alive = false;
            old_session.exit_code = Some(-1);
            drop(old_session);
            self.emit_lifecycle(LifecycleEvent::SessionExited {
                session_id: old_session_id,
                code: -1,
            });
        }
        self.emit_lifecycle(LifecycleEvent::SessionStarted {
            session_id: new_session_id,
            title: title.clone(),
        });
        self.sync_active_tab_title(title.clone());
        self.window.set_title(&format!("{} — pj001", title));
        self.cursor_visible = true;
        self.last_ime_cursor = None;
        self.preedit = None;
        self.window.request_redraw();
        Ok(())
    }

    fn create_tab(&mut self) -> Result<()> {
        let tab_id = self.ids.new_tab();
        let pane_id = self.ids.new_pane();
        let root = Layout::Pane(pane_id);
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let gutter_px = compute_block_gutter_px(
            self.block_visual_active(),
            self.renderer.cell_metrics(),
            self.logical_font_size,
        );
        let layouts = compute_tab_viewports(&root, size, self.renderer.cell_metrics(), gutter_px);
        let viewport = layouts[&pane_id];
        let previous_tab = self.active_tab;
        let previous_idx = self.active_index();
        self.tabs.push(Tab {
            id: tab_id,
            title: "shell".to_string(),
            panes: Vec::new(),
            root,
            active: pane_id,
        });
        self.active_tab = tab_id;
        let spawn = self.spawn_session_for_pane(
            pane_id,
            SessionSpec {
                title: "shell".to_string(),
                command: CommandSpec::Shell,
            },
            viewport,
        );
        if spawn.is_err() {
            self.tabs.retain(|tab| tab.id != tab_id);
            self.active_tab = previous_tab;
            return spawn;
        }
        if self.focused {
            self.active_tab = previous_tab;
            self.send_focus_report(previous_idx, false);
            self.active_tab = tab_id;
            self.send_focus_report(0, true);
        }
        self.update_min_inner_size();
        let title = self.session_for_pane_idx(0).title.clone();
        self.sync_active_tab_title(title.clone());
        self.window.set_title(&format!("{} — pj001", title));
        self.cursor_visible = true;
        self.last_ime_cursor = None;
        self.preedit = None;
        self.window.request_redraw();
        Ok(())
    }

    fn set_active_tab(&mut self, id: TabId) {
        if self.active_tab == id || !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }

        let old_idx = self.active_index();
        if self.focused {
            self.send_focus_report(old_idx, false);
        }
        self.active_tab = id;
        self.apply_layout_viewports();
        self.update_min_inner_size();

        let new_idx = self.active_index();
        if self.focused {
            self.send_focus_report(new_idx, true);
        }
        let title = self.session_for_pane_idx(new_idx).title.clone();
        self.sync_active_tab_title(title.clone());
        self.window.set_title(&format!("{} — pj001", title));
        self.cursor_visible = true;
        self.last_ime_cursor = None;
        self.preedit = None;
        self.window.request_redraw();
    }

    fn focus_tab_by_ordinal(&mut self, ordinal: usize) {
        if ordinal == 0 {
            return;
        }
        let Some(tab) = self.tabs.get(ordinal - 1) else {
            return;
        };
        self.set_active_tab(tab.id);
    }

    fn focus_adjacent_tab(&mut self, next: bool) {
        if self.tabs.len() <= 1 {
            return;
        }
        let current_idx = self.active_tab_index();
        let len = self.tabs.len();
        let next_idx = if next {
            (current_idx + 1) % len
        } else {
            (current_idx + len - 1) % len
        };
        self.set_active_tab(self.tabs[next_idx].id);
    }

    fn close_active_tab(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        let closing_tab = self.active_tab;
        let Some(closing_idx) = self.tabs.iter().position(|tab| tab.id == closing_tab) else {
            return;
        };
        self.remove_tab_at(closing_idx);
        self.apply_layout_viewports();
        self.update_min_inner_size();
        let active_idx = self.active_index();
        let title = self.session_for_pane_idx(active_idx).title.clone();
        self.sync_active_tab_title(title.clone());
        self.window.set_title(&format!("{} — pj001", title));
        self.cursor_visible = true;
        self.last_ime_cursor = None;
        self.preedit = None;
        self.window.request_redraw();
    }

    fn remove_tab_at(&mut self, closing_idx: usize) {
        if self.tabs.len() <= 1 || closing_idx >= self.tabs.len() {
            return;
        }
        let closing_was_active = self.tabs[closing_idx].id == self.active_tab;
        let sessions = self.tabs[closing_idx]
            .panes
            .iter()
            .map(|pane| pane.session)
            .collect::<Vec<_>>();
        for session_id in sessions {
            self.sessions.remove(&session_id);
        }
        self.tabs.remove(closing_idx);
        if closing_was_active {
            let next_idx = closing_idx.min(self.tabs.len() - 1);
            self.active_tab = self.tabs[next_idx].id;
        }
    }

    fn close_active_pane(&mut self) {
        if self.active_tab().panes.len() <= 1 {
            return;
        }
        let closing = self.active_tab().active;
        let mut next_root = self.active_tab().root.clone();
        if !next_root.close_pane(closing) {
            log::warn!("close requested for missing active pane {}", closing.0);
            return;
        }
        self.active_tab_mut().root = next_root;
        let Some(idx) = self.active_tab().panes.iter().position(|p| p.id == closing) else {
            log::warn!("close requested for missing pane {}", closing.0);
            return;
        };
        let closing_session = self.active_tab().panes[idx].session;
        self.active_tab_mut().panes.remove(idx);
        self.sessions.remove(&closing_session);
        self.apply_layout_viewports();
        self.update_min_inner_size();
        if let Some(next) = self.next_alive_pane_id() {
            self.active_tab_mut().active = next;
            self.cursor_visible = true;
            self.last_ime_cursor = None;
            self.preedit = None;
            if let Some(next_idx) = self.active_tab().panes.iter().position(|p| p.id == next) {
                if self.focused {
                    self.send_focus_report(next_idx, true);
                }
                let title = self.session_for_pane_idx(next_idx).title.clone();
                self.window.set_title(&format!("{} — pj001", title));
            }
        }
        self.window.request_redraw();
    }

    fn focus_adjacent_pane(&mut self, next: bool) {
        let order = self.active_tab().root.pane_order();
        if order.len() <= 1 {
            return;
        }
        let Some(current_idx) = order.iter().position(|id| *id == self.active_tab().active) else {
            return;
        };
        let len = order.len();
        for step in 1..=len {
            let idx = if next {
                (current_idx + step) % len
            } else {
                (current_idx + len - (step % len)) % len
            };
            let candidate = order[idx];
            if self
                .session_id_for_pane(candidate)
                .and_then(|session_id| self.sessions.get(&session_id))
                .map(|session| session.alive)
                .unwrap_or(false)
            {
                self.set_active(candidate);
                return;
            }
        }
    }

    fn focus_pane_by_ordinal(&mut self, ordinal: usize) {
        if ordinal == 0 {
            return;
        }
        let order = self.active_tab().root.pane_order();
        let Some(candidate) = order.get(ordinal - 1).copied() else {
            return;
        };
        self.set_active(candidate);
    }

    fn adjust_active_split(&mut self, axis: SplitAxis, direction: RatioDirection) -> bool {
        let active = self.active_tab().active;
        if self
            .active_tab_mut()
            .root
            .adjust_split_for_pane(active, axis, direction)
        {
            self.apply_layout_viewports();
            self.window.request_redraw();
            true
        } else {
            false
        }
    }

    fn minimum_inner_size(&self) -> PhysicalSize<u32> {
        let cell = self.renderer.cell_metrics();
        let (cols, rows) = self
            .active_tab()
            .root
            .minimum_size(MIN_PANE_COLS, MIN_PANE_ROWS);
        PhysicalSize::new(
            (cols as u32 * cell.width).max(MIN_WINDOW_WIDTH),
            ((rows + TAB_BAR_ROWS) as u32 * cell.height).max(MIN_WINDOW_HEIGHT),
        )
    }

    fn minimum_inner_size_logical(&self) -> LogicalSize<f64> {
        let size = self.minimum_inner_size();
        size.to_logical(self.window.scale_factor())
    }

    fn target_inner_size_for_layout(&self) -> PhysicalSize<u32> {
        let min = self.minimum_inner_size();
        PhysicalSize::new(
            self.surface_config.width.max(min.width),
            self.surface_config.height.max(min.height),
        )
    }

    fn update_min_inner_size(&self) {
        log::debug!(
            "set min inner size logical={:?} scale={}",
            self.minimum_inner_size_logical(),
            self.window.scale_factor(),
        );
        self.window
            .set_min_inner_size(Some(self.minimum_inner_size_logical()));
    }

    fn log_window_frame(&self, label: &str) {
        let inner = self.window.inner_size();
        log::debug!(
            "{label}: outer={:?} inner={}x{} window_scale={} stored_scale={}",
            self.window.outer_position().ok(),
            inner.width,
            inner.height,
            self.window.scale_factor(),
            self.current_scale_factor,
        );
    }

    /// M12-5 회귀 fix v3: 호출 시점에 알려진 final size로 PTY를 spawn. 호출자는
    /// 첫 winit Resized를 받아 그 size를 넘기거나 (preferred), `inner_size()` fallback.
    #[allow(dead_code)]
    async fn new(
        window: Arc<Window>,
        proxy: EventLoopProxy<UserEvent>,
        config: Config,
    ) -> Result<Self> {
        let size = window.inner_size();
        Self::new_with_size(window, proxy, config, size).await
    }

    async fn new_with_size(
        window: Arc<Window>,
        proxy: EventLoopProxy<UserEvent>,
        config: Config,
        size: PhysicalSize<u32>,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // M-P3-2a (옵션 F): macOS에선 winit_view의 backing layer를 wgpu가 점유하지 않도록
        // WgpuOverlay sibling subview를 만들고 그 CAMetalLayer로 surface 직접 생성.
        // 다른 OS는 기존 path (winit window → wgpu) 그대로.
        #[cfg(target_os = "macos")]
        let (surface, overlay_attach) = match macos_overlay::attach_overlay(&window) {
            Some(attach) => {
                let surf = unsafe {
                    instance.create_surface_unsafe(
                        wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(attach.metal_layer_ptr),
                    )?
                };
                macos_overlay::log_layer_class_after_surface(&window);
                (surf, Some(attach))
            }
            None => {
                log::warn!("macos_overlay: attach 실패 — fallback create_surface(window)");
                (instance.create_surface(window.clone())?, None)
            }
        };
        #[cfg(not(target_os = "macos"))]
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| Error::NoAdapter)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("pj001-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 4096,
                    ..wgpu::Limits::downlevel_defaults()
                },
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // Phase 3 step 1: alpha_mode 명시 선택. macOS Metal은 PostMultiplied 흔히 지원.
        // PostMultiplied: compositor가 src.rgb * src.a. PreMultiplied: src.rgb 이미 a 곱해진 가정.
        // shader가 srgb space에서 일반 RGBA 출력하므로 PostMultiplied가 자연.
        let preferred_alpha = [
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Opaque,
        ];
        let alpha_mode = preferred_alpha
            .iter()
            .copied()
            .find(|m| caps.alpha_modes.contains(m))
            .unwrap_or(caps.alpha_modes[0]);
        log::info!(
            "surface alpha modes available={:?} selected={:?}",
            caps.alpha_modes,
            alpha_mode
        );
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: caps.present_modes[0],
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let renderer = Renderer::new(
            &device,
            &queue,
            format,
            [size.width as f32, size.height as f32],
            physical_font_size(DEFAULT_FONT_SIZE, window.scale_factor()),
            config.theme.unwrap_or_else(ThemePalette::default_theme),
        );

        let hooks = config.hooks.clone();
        let pane_specs = config.pane_specs()?;
        let mut ids = IdAllocator::default();
        let pane_ids = (0..pane_specs.len())
            .map(|_| ids.new_pane())
            .collect::<Vec<_>>();
        let layout_root = Layout::from_initial_panes(&pane_ids);
        // AppState init 시점은 block_capable=false라 gutter 0.
        let layouts = compute_tab_viewports(&layout_root, size, renderer.cell_metrics(), 0);
        log::info!(
            "startup spawn size={}x{} cell={}x{} panes={} layouts={:?}",
            size.width,
            size.height,
            renderer.cell_metrics().width,
            renderer.cell_metrics().height,
            pane_specs.len(),
            layouts
                .values()
                .map(|v| (v.cols, v.rows, v.col_offset, v.status_row))
                .collect::<Vec<_>>(),
        );
        let mut panes = Vec::new();
        let mut sessions: HashMap<SessionId, Session> = HashMap::new();
        for (spec_index, spec) in pane_specs.into_iter().enumerate() {
            let pane_id = pane_ids[spec_index];
            let session_id = ids.new_session();
            let viewport = layouts[&pane_id];
            let term = Arc::new(Mutex::new(Term::new(viewport.cols, viewport.rows)));
            let shell = spec.command.resolve();
            log::info!("pane {} shell: {}", pane_id.0, shell);
            let pty = PtyHandle::spawn(
                &shell,
                PtySize {
                    rows: viewport.rows as u16,
                    cols: viewport.cols as u16,
                    pixel_width: viewport.width_px as u16,
                    pixel_height: viewport.height_px as u16,
                },
                term.clone(),
                proxy.clone(),
                session_id,
            )?;
            let title = spec.title;
            sessions.insert(
                session_id,
                Session {
                    id: session_id,
                    title: title.clone(),
                    command: shell,
                    pty,
                    term,
                    alive: true,
                    exit_code: None,
                    created_at: Instant::now(),
                },
            );
            panes.push(Pane {
                id: pane_id,
                session: session_id,
                viewport,
            });
            if let Some(sink) = &hooks.lifecycle_sink {
                sink.on_lifecycle(LifecycleEvent::SessionStarted { session_id, title });
            }
        }
        let tab_id = ids.new_tab();
        let tab = Tab {
            id: tab_id,
            title: panes
                .first()
                .and_then(|pane| sessions.get(&pane.session))
                .map(|session| session.title.clone())
                .unwrap_or_else(|| "shell".to_string()),
            panes,
            root: layout_root,
            active: PaneId::first(),
        };

        let current_scale_factor = window.scale_factor();
        let state = Self {
            window,
            proxy,
            surface,
            surface_config,
            device,
            queue,
            sessions,
            tabs: vec![tab],
            active_tab: tab_id,
            renderer,
            hooks,
            last_ime_cursor: None,
            preedit: None,
            cursor_visible: true,
            last_blink: Instant::now(),
            focused: true,
            cursor_blinking_cache: true,
            modifiers: ModifiersState::empty(),
            last_mouse_pos: None,
            dragging_divider: None,
            selection: None,
            last_auto_scroll_at: None,
            last_click: None,
            pending_resize: None,
            logical_font_size: DEFAULT_FONT_SIZE,
            pending_logical_font_size: None,
            preserve_grid_on_next_resize: false,
            current_scale_factor,
            quick_spawn_presets: config.quick_spawn_presets.clone(),
            block_mode: config.block_mode,
            pending_quick_spawn: None,
            ids,
            pending_find: None,
            mouse_buttons_held: 0,
            #[cfg(target_os = "macos")]
            overlay_attach,
        };
        state.update_min_inner_size();
        Ok(state)
    }

    /// M12-3: pane index → Session lookup. borrow scope 짧게 유지하기 위해 SessionId만 복사.
    fn session_id_for_pane_idx(&self, idx: usize) -> SessionId {
        self.active_tab().panes[idx].session
    }

    /// M12-3: pane index → Session (immutable).
    fn session_for_pane_idx(&self, idx: usize) -> &Session {
        let session_id = self.session_id_for_pane_idx(idx);
        &self.sessions[&session_id]
    }

    /// M12-3: pane index → Session (mutable). lookup이 panes/sessions 동시 borrow가 아니라 두 단계.
    fn session_for_pane_idx_mut(&mut self, idx: usize) -> &mut Session {
        let session_id = self.session_id_for_pane_idx(idx);
        self.sessions
            .get_mut(&session_id)
            .expect("session for pane not found")
    }

    /// M12-6 design §5: 마우스 hit-test로 PaneId 반환. click site에서 idx 거치지 않고 사용.
    fn pane_at_mouse(&self, include_status_row: bool) -> Option<PaneId> {
        self.pane_index_at_mouse(include_status_row)
            .map(|idx| self.active_tab().panes[idx].id)
    }

    fn pane_cell_at_mouse(&self) -> Option<(PaneId, (usize, usize))> {
        let (col, row) = self.mouse_cell()?;
        for pane in &self.active_tab().panes {
            let viewport = pane.viewport;
            // First slice: visible viewport cells only. Scrolled-back absolute
            // selection coordinates are handled by the later clipboard slice.
            let Some(local_col) = col.checked_sub(viewport.col_offset) else {
                continue;
            };
            let Some(local_row) = row.checked_sub(viewport.row_offset) else {
                continue;
            };
            if local_col < viewport.cols && local_row < viewport.rows {
                // status row는 색 띠 chrome — selection/text hit 영역 아님.
                if viewport.status_row == Some(row) {
                    return None;
                }
                // scrollbar thumb 우측 가장자리 cell — scrollback 있을 때만 표시되는 chrome.
                // selection hit 아님 + cursor I-beam도 안 함.
                if local_col + 1 == viewport.cols && self.scrollbar_visible_in(pane) {
                    return None;
                }
                return Some((pane.id, (local_row, local_col)));
            }
        }
        None
    }

    /// 해당 pane에 scrollbar thumb이 표시 중인지 — scrollback 있고 view_offset valid.
    fn scrollbar_visible_in(&self, pane: &Pane) -> bool {
        let Some(session) = self.sessions.get(&pane.session) else {
            return false;
        };
        let Ok(term) = session.term.lock() else {
            return false;
        };
        term.scrollback_len() > 0
    }

    /// Phase 2 caret 모델: mouse pos → (pane, (row, caret_col)). caret_col은 글자 사이
    /// 위치(0..=cols). pane_cell_at_mouse와 같은 viewport hit-test이나 col 차이가 caret 단위.
    fn pane_caret_at_mouse(&self) -> Option<(PaneId, (usize, usize))> {
        let pos = self.last_mouse_pos?;
        let cell = self.renderer.cell_metrics();
        if cell.width == 0 || cell.height == 0 || pos.x < 0.0 || pos.y < 0.0 {
            return None;
        }
        let cell_w = cell.width as f64;
        let abs_caret = ((pos.x + cell_w * 0.5) / cell_w).floor() as usize;
        let abs_row = (pos.y / cell.height as f64).floor() as usize;
        for pane in &self.active_tab().panes {
            let viewport = pane.viewport;
            let Some(local_caret) = abs_caret.checked_sub(viewport.col_offset) else {
                continue;
            };
            let Some(local_row) = abs_row.checked_sub(viewport.row_offset) else {
                continue;
            };
            if local_caret <= viewport.cols && local_row < viewport.rows {
                return Some((pane.id, (local_row, local_caret)));
            }
        }
        None
    }

    fn mouse_cell(&self) -> Option<(usize, usize)> {
        let pos = self.last_mouse_pos?;
        let cell = self.renderer.cell_metrics();
        if cell.width == 0 || cell.height == 0 || pos.x < 0.0 || pos.y < 0.0 {
            return None;
        }
        Some((
            (pos.x / cell.width as f64).floor() as usize,
            (pos.y / cell.height as f64).floor() as usize,
        ))
    }

    fn tab_at_mouse(&self) -> Option<TabId> {
        let (col, row) = self.mouse_cell()?;
        if row != 0 || self.tabs.is_empty() {
            return None;
        }
        let total_cols =
            (self.surface_config.width / self.renderer.cell_metrics().width).max(1) as usize;
        let idx = tab_index_at_col(total_cols, self.tabs.len(), col)?;
        Some(self.tabs[idx].id)
    }

    fn divider_hit_at_mouse(&self) -> Option<layout::DividerHit> {
        let (col, row) = self.mouse_cell()?;
        let row = row.checked_sub(TAB_BAR_ROWS)?;
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let cell = self.renderer.cell_metrics();
        layout::divider_hit_at(
            &self.active_tab().root,
            tab_content_size(size, cell),
            cell,
            col,
            row,
        )
    }

    fn update_mouse_cursor(&self) {
        // 우선순위: divider drag/hover(↔ ↕) > selection drag 중(I-beam 유지) > pane content
        // cell(I-beam) > 그 외(Default). status row/tab bar는 텍스트 편집 영역이 아니라 I-beam
        // 부적절 (pane_cell_at_mouse가 status row 제외).
        let hit = self
            .dragging_divider
            .as_ref()
            .cloned()
            .or_else(|| self.divider_hit_at_mouse());
        let dragging_selection = self
            .selection
            .as_ref()
            .map(|s| s.dragging)
            .unwrap_or(false);
        let icon = match hit.map(|hit| hit.axis()) {
            Some(SplitAxis::Vertical) => CursorIcon::ColResize,
            Some(SplitAxis::Horizontal) => CursorIcon::RowResize,
            None => {
                if dragging_selection || self.pane_cell_at_mouse().is_some() {
                    CursorIcon::Text
                } else {
                    CursorIcon::Default
                }
            }
        };
        self.window.set_cursor(icon);
    }

    /// 창 밖에서 button release → `MouseInput Released` 누락 시 dragging=true가 stuck.
    /// CursorMoved 진입에서 호출. button mask가 비어있으면 selection drag 종료 + divider drag 종료.
    fn reconcile_lost_button_release(&mut self) {
        if self.mouse_buttons_held != 0 {
            return;
        }
        let selection_was_dragging = self.selection.as_ref().map(|s| s.dragging).unwrap_or(false);
        if selection_was_dragging {
            self.finish_selection_drag();
        }
        if self.dragging_divider.is_some() {
            self.dragging_divider = None;
            self.update_mouse_cursor();
        }
    }

    /// Phase 5: continuous auto-scroll 활성 조건 — selection.dragging + 마우스 viewport 밖.
    /// about_to_wait가 매 50ms 호출해 sticky scroll 유지.
    fn is_auto_scrolling(&self) -> bool {
        let Some(selection) = self.selection.as_ref() else {
            return false;
        };
        if !selection.dragging {
            return false;
        }
        let Some(pos) = self.last_mouse_pos else {
            return false;
        };
        let Some(viewport) = self
            .active_tab()
            .panes
            .iter()
            .find(|p| p.id == selection.pane)
            .map(|p| p.viewport)
        else {
            return false;
        };
        let vp_top = viewport.y_px as f64;
        let vp_bottom = vp_top + viewport.height_px as f64;
        pos.y < vp_top || pos.y >= vp_bottom
    }

    fn update_selection_to_mouse(&mut self) -> bool {
        // Phase 2 caret 모델: dragging 중에만 mouse caret 위치로 head 갱신.
        // caret 모델은 cell A↔A+1 사이 boundary로 head가 잡혀 한 글자 선택은 cell의
        // 왼쪽 절반→오른쪽 절반 drag로 자연스럽게 시작. dragged_off_anchor latch 불필요.
        let Some(selection) = self.selection.as_ref() else {
            return false;
        };
        if !selection.dragging {
            return false;
        }
        let pane = selection.pane;
        let Some(viewport) = self
            .active_tab()
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.viewport)
        else {
            return false;
        };
        let Some(pos) = self.last_mouse_pos else {
            return false;
        };
        // Phase 5: drag 중 마우스가 viewport 위/아래로 벗어나면 auto-scroll. 위는 view_offset
        // 증가(scrollback 위로), 아래는 view_offset 감소(현재 컨텐츠로). pos.y가 viewport y_px
        // 미만이면 위, y_px + height_px 이상이면 아래.
        let viewport_top = viewport.y_px as f64;
        let viewport_bottom = (viewport.y_px as f64) + (viewport.height_px as f64);
        let scroll_delta: isize = if pos.y < viewport_top {
            1
        } else if pos.y >= viewport_bottom {
            -1
        } else {
            0
        };
        let session_id_for_scroll = self
            .active_tab()
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.session);
        if scroll_delta != 0 {
            // Phase 5: throttle — 50ms 간격으로만 scroll. 마우스 빨리 움직여도 over-scroll 방지
            // + about_to_wait sticky scroll에서 같은 throttle 재사용 (continuous scroll).
            const SCROLL_THROTTLE_MS: u64 = 50;
            let now = Instant::now();
            let throttle_ok = self
                .last_auto_scroll_at
                .map_or(true, |last| {
                    now.duration_since(last) >= Duration::from_millis(SCROLL_THROTTLE_MS)
                });
            if throttle_ok {
                if let Some(sid) = session_id_for_scroll {
                    if let Some(mut term) = self.sessions.get(&sid).and_then(|s| s.term.lock().ok())
                    {
                        term.scroll_view_by(scroll_delta);
                    }
                }
                self.last_auto_scroll_at = Some(now);
            }
        } else {
            // viewport 안으로 돌아오면 throttle 리셋.
            self.last_auto_scroll_at = None;
        }
        let cell = self.renderer.cell_metrics();
        let new_head_viewport = clamp_pos_to_viewport_caret(pos, viewport, cell);
        // Phase 5: viewport caret → abs caret. scroll 후 새 view_offset 기준으로 변환.
        let new_head_abs = {
            session_id_for_scroll
                .and_then(|sid| self.sessions.get(&sid))
                .and_then(|s| s.term.lock().ok())
                .map(|t| caret_to_abs(&t, new_head_viewport))
                .unwrap_or(new_head_viewport)
        };
        let viewport_cols = viewport.cols;
        let Some(selection) = self.selection.as_mut() else {
            return false;
        };
        // line_drag 모드: head row가 anchor row와 다를 때만 line whole 발동. 같은 row 안
        // micro-drag(trackpad 떨림 등)에는 char-level 동작 유지 — 단순 click 후 미세 흔들림
        // 으로 line 전체가 잡히는 회귀 방지.
        let target_head = if selection.line_drag && new_head_abs.0 != selection.anchor.0 {
            let head_row = new_head_abs.0;
            let anchor_row = selection.anchor.0;
            if head_row > anchor_row {
                // 정방향 drag (아래로) — anchor=(anchor_row, 0), head=(head_row, cols).
                selection.anchor = (anchor_row, 0);
                (head_row, viewport_cols)
            } else {
                // 역방향 drag (위로) — anchor=(anchor_row, cols), head=(head_row, 0).
                selection.anchor = (anchor_row, viewport_cols);
                (head_row, 0)
            }
        } else {
            new_head_abs
        };
        if selection.head == target_head {
            return false;
        }
        selection.head = target_head;
        true
    }

    /// 슬라이스 6.3c: Cmd+click 시 클릭한 cell의 hyperlink URI를 추출해 macOS `open`으로 실행.
    /// 반환 true면 caller가 다른 click 처리 skip.
    fn try_open_hyperlink_at_mouse(&mut self) -> bool {
        let Some((pane_id, (row, col))) = self.pane_cell_at_mouse() else {
            return false;
        };
        // pane_id의 session에서 cell의 hyperlink_id 조회.
        let idx_opt = self.active_tab().panes.iter().position(|p| p.id == pane_id);
        let Some(idx) = idx_opt else {
            return false;
        };
        let session = self.session_for_pane_idx(idx);
        let Ok(term) = session.term.lock() else {
            return false;
        };
        let cell = term.cell(row, col);
        let id = cell.hyperlink_id;
        if id == 0 {
            return false;
        }
        let uri = match term.hyperlink_uri_by_id(id) {
            Some(u) => u.to_string(),
            None => return false,
        };
        drop(term);
        // macOS `open` 호출.
        log::info!("hyperlink open: {}", uri);
        if let Err(e) = std::process::Command::new("open").arg(&uri).spawn() {
            log::warn!("hyperlink open failed: {e}");
        }
        true
    }

    /// 슬라이스 6.6b (Codex B-1): 마우스 버튼 비트마스크 갱신. reporting이 press를
    /// 흡수해도 button_held 추적은 일관되도록 모든 MouseInput에서 호출.
    fn track_mouse_button(&mut self, pressed: bool, button: u8) {
        let bit: u8 = match button {
            0 => 0b001, // Left
            1 => 0b010, // Middle
            2 => 0b100, // Right
            _ => return,
        };
        if pressed {
            self.mouse_buttons_held |= bit;
        } else {
            self.mouse_buttons_held &= !bit;
        }
    }

    /// 슬라이스 6.6: 마우스 버튼 이벤트를 reporting mode일 때만 PTY로 송신.
    /// 반환 true면 caller가 다른 처리(selection 등)를 skip하라는 의미.
    fn try_report_mouse_button(&mut self, pressed: bool, button: u8) -> bool {
        let (proto, sgr) = self.active_mouse_state();
        if matches!(proto, MouseProtocol::Off) {
            return false;
        }
        let Some((pane_id, cell)) = self.pane_cell_at_mouse() else {
            return false;
        };
        // 좌표는 pane local (0-based) — 마우스 이벤트는 pane 기준.
        self.set_active(pane_id);
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();
        let ctrl = self.modifiers.control_key();
        let bytes = encode_mouse_report(
            button, pressed, false, shift, alt, ctrl, cell.1, cell.0, sgr,
        );
        let idx = self.active_index();
        let session = self.session_for_pane_idx_mut(idx);
        if let Err(e) = session.pty.write(&bytes) {
            log::warn!("mouse-report write failed: {e}");
        }
        true
    }

    /// 모션 이벤트(드래그)를 ButtonEvent/AnyEvent reporting에 따라 송신.
    fn try_report_mouse_motion(&mut self, button_held: bool) -> bool {
        let (proto, sgr) = self.active_mouse_state();
        let should = match proto {
            MouseProtocol::Off | MouseProtocol::Button => false,
            MouseProtocol::ButtonEvent => button_held,
            MouseProtocol::AnyEvent => true,
        };
        if !should {
            return false;
        }
        let Some((_, cell)) = self.pane_cell_at_mouse() else {
            return false;
        };
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();
        let ctrl = self.modifiers.control_key();
        // 버튼 정보 없음 — 0 (drag 시 SGR 인코딩에선 응용프로그램이 hold 상태 추적).
        // 1003 hover는 release(3) — xterm spec.
        let button = if button_held { 0 } else { 3 };
        let bytes = encode_mouse_report(button, false, true, shift, alt, ctrl, cell.1, cell.0, sgr);
        let idx = self.active_index();
        let session = self.session_for_pane_idx_mut(idx);
        if let Err(e) = session.pty.write(&bytes) {
            log::warn!("mouse-motion-report write failed: {e}");
        }
        true
    }

    /// active pane의 term에서 마우스 모드 + SGR encoding 스냅샷.
    fn active_mouse_state(&self) -> (MouseProtocol, bool) {
        let idx = self.active_index();
        self.session_for_pane_idx(idx)
            .term
            .lock()
            .map(|t| (t.mouse_protocol(), t.mouse_sgr_encoding()))
            .unwrap_or((MouseProtocol::Off, false))
    }

    /// Phase 2 caret 모델: word/line selection은 cell 기반 (double/triple click),
    /// single click drag selection은 caret 기반 (anchor=caret, head=caret).
    fn start_selection_at(&mut self, pane: PaneId, cell: (usize, usize), caret: (usize, usize)) {
        let click_count = self.next_click_count(pane, cell);
        // Phase 5: cell/caret은 viewport-local. abs row로 변환해 anchor/head 저장.
        // 또한 click cell이 빈 cell이면 line whole drag mode 활성.
        let session_id = self
            .active_tab()
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.session);
        let (abs_caret, is_empty_cell, viewport_cols) = if let Some(sid) = session_id {
            if let Some(term) = self.sessions.get(&sid).and_then(|s| s.term.lock().ok()) {
                let abs = caret_to_abs(&term, caret);
                let empty = cell.0 < term.rows()
                    && cell.1 < term.cols()
                    && term.cell(cell.0, cell.1).ch == ' ';
                (abs, empty, term.cols())
            } else {
                (caret, false, 0)
            }
        } else {
            (caret, false, 0)
        };
        let range = match click_count {
            2 => self.word_selection_at(pane, cell),
            3.. => Some(self.line_selection_at(pane, cell.0)),
            _ => None,
        };
        self.selection = if let Some(range) = range {
            Some(MouseSelection {
                pane,
                anchor: range.start,
                head: range.end,
                dragging: false,
                line_drag: false,
            })
        } else {
            // 빈 cell이면 line_drag=true 표시만. anchor=head=abs_caret으로 시작해서 단순 click
            // 후 release면 selection 없음 (anchor==head 자연 처리). drag로 head 움직이면
            // update_selection_to_mouse가 line whole로 확장.
            let _ = viewport_cols;
            Some(MouseSelection {
                pane,
                anchor: abs_caret,
                head: abs_caret,
                dragging: true,
                line_drag: is_empty_cell,
            })
        };
    }

    fn next_click_count(&mut self, pane: PaneId, cell: (usize, usize)) -> u8 {
        let now = Instant::now();
        let count = self
            .last_click
            .as_ref()
            .filter(|last| {
                last.pane == pane
                    && last.cell == cell
                    && now.duration_since(last.at) <= Duration::from_millis(MULTI_CLICK_MS)
            })
            .map(|last| last.count.saturating_add(1).min(3))
            .unwrap_or(1);
        self.last_click = Some(MouseClick {
            pane,
            cell,
            count,
            at: now,
        });
        count
    }

    fn word_selection_at(&self, pane: PaneId, cell: (usize, usize)) -> Option<SelectionRange> {
        let session_id = self
            .active_tab()
            .panes
            .iter()
            .find(|candidate| candidate.id == pane)
            .map(|pane| pane.session)?;
        let session = self.sessions.get(&session_id)?;
        let term = session.term.lock().ok()?;
        // Phase 5: viewport-local row/col → abs row로 변환해서 반환.
        let viewport_range = word_selection_range(&term, cell.0, cell.1)?;
        let top_abs = term.top_visible_abs() as usize;
        Some(SelectionRange::new(
            (top_abs + viewport_range.start.0, viewport_range.start.1),
            (top_abs + viewport_range.end.0, viewport_range.end.1),
        ))
    }

    fn line_selection_at(&self, pane: PaneId, row: usize) -> SelectionRange {
        let pane_info = self
            .active_tab()
            .panes
            .iter()
            .find(|candidate| candidate.id == pane);
        let cols = pane_info.map(|p| p.viewport.cols).unwrap_or(1);
        let session_id = pane_info.map(|p| p.session);
        let top_abs = session_id
            .and_then(|sid| self.sessions.get(&sid))
            .and_then(|s| s.term.lock().ok())
            .map(|t| t.top_visible_abs() as usize)
            .unwrap_or(0);
        // caret 모델: line 전체 = [0, cols). Phase 5: row를 abs로 변환.
        SelectionRange::new((top_abs + row, 0), (top_abs + row, cols))
    }

    fn finish_selection_drag(&mut self) {
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        let was_dragging = selection.dragging;
        selection.dragging = false;
        // caret 모델: head == anchor (caret 동일) 시 selection은 empty range.
        // 단순 click이든 한 글자 선택 후 origin 복귀든 동일하게 selection 비움.
        if was_dragging && selection.anchor == selection.head {
            self.selection = None;
        }
        self.window.request_redraw();
    }

    fn drag_divider_to_mouse(&mut self) {
        let Some(hit) = self.dragging_divider.clone() else {
            return;
        };
        let Some((col, row)) = self.mouse_cell() else {
            return;
        };
        let Some(row) = row.checked_sub(TAB_BAR_ROWS) else {
            return;
        };
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let cell = self.renderer.cell_metrics();
        if layout::set_split_ratio_at_cell(
            &mut self.active_tab_mut().root,
            &hit,
            tab_content_size(size, cell),
            cell,
            col,
            row,
            MIN_PANE_COLS,
            MIN_PANE_ROWS,
        ) {
            self.apply_layout_viewports();
            self.window.request_redraw();
        }
    }

    /// Phase 5: Cmd+A — 활성 pane의 scrollback + main 전체 selection.
    /// anchor=(oldest_kept_abs, 0), head=(last_abs_row+1, 0) caret 의미. Cmd+C로 전체 copy.
    fn handle_select_all(&mut self) {
        let idx = self.active_index();
        let pane = &self.active_tab().panes[idx];
        let pane_id = pane.id;
        let session_id = pane.session;
        let Some(session) = self.sessions.get(&session_id) else {
            return;
        };
        let Ok(term) = session.term.lock() else {
            return;
        };
        let cols = term.cols();
        let rows = term.rows();
        let scrollback_len = term.scrollback_len();
        let oldest_abs = term.oldest_kept_abs() as usize;
        let last_main_abs = oldest_abs + scrollback_len + rows.saturating_sub(1);
        drop(term);
        self.selection = Some(MouseSelection {
            pane: pane_id,
            anchor: (oldest_abs, 0),
            head: (last_main_abs, cols),
            dragging: false,
            line_drag: false,
        });
        self.window.request_redraw();
    }

    /// M12-6 design §5: 마우스 hit-test로 SessionId 반환. pane_at_mouse → SessionId 매핑.
    #[allow(dead_code)]
    fn session_at_mouse(&self, include_status_row: bool) -> Option<SessionId> {
        self.pane_index_at_mouse(include_status_row)
            .map(|idx| self.active_tab().panes[idx].session)
    }

    /// M12 selection slice: visible cell selection을 clipboard text로 직렬화.
    fn handle_copy(&mut self) {
        let Some(selection) = self.selection.as_ref() else {
            log::debug!("copy ignored: no selection");
            return;
        };
        let Some(pane) = self
            .active_tab()
            .panes
            .iter()
            .find(|pane| pane.id == selection.pane)
        else {
            log::warn!("copy ignored: selected pane missing");
            return;
        };
        let Some(session) = self.sessions.get(&pane.session) else {
            log::warn!("copy ignored: selected session missing");
            return;
        };
        let text = match session.term.lock() {
            Ok(term) => {
                // Phase 5 후속: selection.anchor/head는 abs caret. selection_text_abs가
                // scrollback row + main row 양쪽 cell_at_abs로 접근 → viewport 밖 row도 추출.
                selection_text_abs(&term, SelectionRange::new(selection.anchor, selection.head))
            }
            Err(e) => {
                log::warn!("copy ignored: term lock failed: {e}");
                return;
            }
        };
        if text.is_empty() {
            log::debug!("copy ignored: empty selection");
            return;
        }
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.clone())) {
            Ok(()) => log::debug!("copy: {} bytes", text.len()),
            Err(e) => log::warn!("clipboard write failed: {e}"),
        }
    }

    /// M10-6: Cmd+V로 진입. arboard로 clipboard 읽고 bracketed paste mode면 \e[200~/\e[201~ 래핑.
    fn handle_paste(&mut self) {
        let text = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("clipboard read failed: {e}");
                return;
            }
        };
        if text.is_empty() {
            return;
        }
        // Phase 5: paste 시 selection 해제 (Derek 보고: 긴 paste 후 selection이 viewport 안
        // 잘못된 영역에 highlight). PTY echo로 화면 갱신되면 abs selection이 다른 row 가리킴.
        // handle_dropped_file도 동일 패턴.
        self.selection = None;
        self.write_text_to_active_pty(&text, "paste");
        self.window.request_redraw();
    }

    fn handle_dropped_file(&mut self, path: &Path) {
        let Some(text) = shell_quoted_path_for_drop(path) else {
            log::warn!("file drop ignored: path is not safe UTF-8");
            return;
        };
        self.selection = None;
        self.write_text_to_active_pty(&text, "file drop");
        self.window.request_redraw();
    }

    fn write_text_to_active_pty(&mut self, text: &str, label: &str) {
        let idx = self.active_index();
        // scrollback view 활성 시 paste는 bottom으로 snap.
        if let Ok(mut term) = self.session_for_pane_idx(idx).term.lock() {
            if term.view_offset() > 0 {
                term.snap_to_bottom();
                self.window.request_redraw();
            }
        }
        let bracketed = self
            .session_for_pane_idx(idx)
            .term
            .lock()
            .map(|t| t.bracketed_paste())
            .unwrap_or(false);
        log::debug!(
            "{label}: {} bytes, bracketed={}, lines={}",
            text.len(),
            bracketed,
            text.matches('\n').count() + 1
        );
        let Some(bytes) = paste_payload_bytes(text, bracketed) else {
            log::warn!("{label} ignored: bracketed paste terminator found in payload");
            return;
        };
        let session = self.session_for_pane_idx_mut(idx);
        if let Err(e) = session.pty.write(&bytes) {
            log::warn!("{label} write failed: {e}");
        }
    }

    fn clear_active_buffer(&mut self, scrollback_only: bool) {
        let idx = self.active_index();
        let send_redraw;
        if let Ok(mut term) = self.session_for_pane_idx(idx).term.lock() {
            let alt_screen = term.is_alt_screen();
            term.clear_scrollback();
            send_redraw = !scrollback_only && !alt_screen;
            log::info!(
                "clear active buffer: mode={}",
                if scrollback_only || alt_screen {
                    "scrollback"
                } else {
                    "redraw"
                }
            );
        } else {
            log::warn!("clear active buffer ignored: term lock failed");
            return;
        }
        self.selection = None;
        if send_redraw {
            let session = self.session_for_pane_idx_mut(idx);
            if let Err(e) = session.pty.write(b"\x0c") {
                log::warn!("clear active buffer redraw write failed: {e}");
            }
        }
        self.window.request_redraw();
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.log_window_frame("resize before");
        let font_changed = self
            .pending_logical_font_size
            .take()
            .map(|logical_font_size| {
                self.logical_font_size = logical_font_size;
                let physical_font_size =
                    physical_font_size(logical_font_size, self.window.scale_factor());
                self.renderer
                    .set_font_size(&self.device, &self.queue, physical_font_size)
            })
            .unwrap_or(false);
        let preserve_grid = std::mem::take(&mut self.preserve_grid_on_next_resize);
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);
        self.renderer
            .resize(&self.queue, [size.width as f32, size.height as f32]);
        let root = self.active_tab().root.clone();
        let gutter_px = compute_block_gutter_px(
            self.block_visual_active(),
            self.renderer.cell_metrics(),
            self.logical_font_size,
        );
        let layouts = compute_tab_viewports(&root, size, self.renderer.cell_metrics(), gutter_px);
        log::info!(
            "resize event size={}x{} cell={}x{} layouts={:?}",
            size.width,
            size.height,
            self.renderer.cell_metrics().width,
            self.renderer.cell_metrics().height,
            layouts
                .values()
                .map(|v| (v.cols, v.rows, v.col_offset, v.status_row))
                .collect::<Vec<_>>(),
        );
        let pane_count = self.active_tab().panes.len();
        for idx in 0..pane_count {
            let pane_id = self.active_tab().panes[idx].id;
            let mut viewport = layouts[&pane_id];
            let prev = self.active_tab().panes[idx].viewport;
            if preserve_grid {
                viewport.cols = prev.cols;
                viewport.rows = prev.rows;
            }
            // M12-5 회귀 fix (Codex threads 019e164b → 019e1653 가설 K):
            // PTY/Term resize는 실제 terminal cell size(rows/cols)가 바뀔 때만 필요하다.
            // col_offset/status_row/pixel 크기는 visual layout/chrome 값이라 이걸 trigger로
            // pty.resize를 보내면 rows/cols가 같아도 SIGWINCH가 발생할 수 있다.
            // split startup 직후 col_offset/status_row 차이가 zsh duplicate prompt 유발.
            let pty_cell_size_changed = prev.cols != viewport.cols || prev.rows != viewport.rows;
            // viewport visual/pixel 값은 surface size 변경 시 달라질 수 있으므로 항상 갱신.
            // M12-5 mutation 단일 진입점 — 전체 struct 교체만 허용.
            self.active_tab_mut().panes[idx].viewport = viewport;
            if pty_cell_size_changed {
                let session = self.session_for_pane_idx_mut(idx);
                if let Ok(mut term) = session.term.lock() {
                    term.resize(viewport.cols, viewport.rows);
                }
                let _ = session.pty.resize(PtySize {
                    rows: viewport.rows as u16,
                    cols: viewport.cols as u16,
                    pixel_width: viewport.width_px as u16,
                    pixel_height: viewport.height_px as u16,
                });
            }
        }
        if font_changed && !preserve_grid {
            self.update_min_inner_size();
        }
        self.window.request_redraw();
        self.log_window_frame("resize after");
    }

    fn prepare_scale_factor_change(&mut self, scale_factor: f64) {
        let physical_font_size = physical_font_size(self.logical_font_size, scale_factor);
        self.pending_logical_font_size = Some(self.logical_font_size);
        self.preserve_grid_on_next_resize = true;
        self.current_scale_factor = scale_factor_or_default(scale_factor);
        log::debug!(
            "scale factor changed: scale={scale_factor} physical_font_size={physical_font_size}"
        );
    }

    fn set_logical_font_size(&mut self, font_size: f32) {
        let font_size = clamp_font_size(font_size);
        if (self.logical_font_size - font_size).abs() < f32::EPSILON {
            return;
        }
        self.pending_logical_font_size = Some(font_size);
        self.preserve_grid_on_next_resize = false;
        self.pending_resize = Some(self.window.inner_size());
        log::info!("font zoom: logical_font_size={font_size}");
    }

    fn render(&mut self) {
        // M10-1: vt가 누적한 응답(DSR/DA 등)을 PTY로 송신. lock 잡고 drain → drop → write.
        let pane_count = self.active_tab().panes.len();
        for idx in 0..pane_count {
            let pane_id = self.active_tab().panes[idx].id;
            let session = self.session_for_pane_idx_mut(idx);
            let responses: Vec<Vec<u8>> = if let Ok(mut term) = session.term.lock() {
                term.drain_responses()
            } else {
                Vec::new()
            };
            for resp in responses {
                if let Err(e) = session.pty.write(&resp) {
                    log::warn!("pty response write (pane {}): {e}", pane_id.0);
                }
            }
        }

        use wgpu::CurrentSurfaceTexture as C;
        let frame = match self.surface.get_current_texture() {
            C::Success(t) | C::Suboptimal(t) => t,
            C::Outdated | C::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
                return;
            }
            C::Timeout => {
                self.window.request_redraw();
                return;
            }
            C::Occluded | C::Validation => return,
        };

        self.renderer.begin_terms();
        let active = self.active_tab().active;
        let pane_count = self.active_tab().panes.len();
        for idx in 0..pane_count {
            let (pane_id, pane_col_offset, pane_row_offset, pane_gutter_px, session_id) = {
                let p = &self.active_tab().panes[idx];
                (
                    p.id,
                    p.viewport.col_offset,
                    p.viewport.row_offset,
                    p.viewport.gutter_px,
                    p.session,
                )
            };
            let term_arc = self.sessions[&session_id].term.clone();
            if let Ok(mut term) = term_arc.lock() {
                let cur = term.cursor();
                let is_active = pane_id == active;
                if is_active {
                    self.cursor_blinking_cache = cur.blinking;
                    if let Some(t) = term.take_title_if_changed() {
                        if let Some(session) = self.sessions.get_mut(&session_id) {
                            session.title = t.clone();
                        }
                        self.sync_active_tab_title(t.clone());
                        self.window.set_title(&format!("{} — pj001", t));
                    }
                }
                let in_scrollback = term.view_offset() > 0;
                let preedit_arg = if is_active {
                    self.preedit.as_deref().map(|s| (s, cur.col, cur.row))
                } else {
                    None
                };
                let cursor_render =
                    if is_active && cur.visible && self.cursor_visible && !in_scrollback {
                        let (row, col) = if let Some((preedit_str, col, row)) = preedit_arg {
                            let mut c = col;
                            for ch in preedit_str.chars() {
                                c += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                            }
                            (row, c.min(term.cols().saturating_sub(1)))
                        } else {
                            (cur.row, cur.col)
                        };
                        Some(CursorRender {
                            row,
                            col,
                            shape: cur.shape,
                            focused: self.focused,
                        })
                    } else {
                        None
                    };
                let preedit_for_render = if in_scrollback { None } else { preedit_arg };
                let selection = self.selection.as_ref().and_then(|selection| {
                    if selection.pane != pane_id {
                        return None;
                    }
                    // Phase 5: anchor/head는 abs caret. viewport와 겹치는 경우만 highlight.
                    // 둘 다 viewport 위 또는 둘 다 viewport 아래면 None — 스크롤로 selection이
                    // viewport 밖으로 빠지면 잘못된 row 0/rows-1에 highlight되는 회귀 방지.
                    let top_abs = term.top_visible_abs() as usize;
                    let bottom_abs = top_abs.saturating_add(term.rows().saturating_sub(1));
                    let lo = selection.anchor.0.min(selection.head.0);
                    let hi = selection.anchor.0.max(selection.head.0);
                    if hi < top_abs || lo > bottom_abs {
                        return None;
                    }
                    let anchor_vp = abs_to_viewport_caret(&term, selection.anchor);
                    let head_vp = abs_to_viewport_caret(&term, selection.head);
                    Some(SelectionRange::new(anchor_vp, head_vp))
                });
                // Phase 4b-2b: block visual 활성 시 visible_blocks → BlockOverlay 리스트.
                // Phase 4b-2c-1: theme token 도입. palette.block_bg/block_border 사용.
                // obsidian만 bg와 다른 값(시각 발동), 나머지 5 테마는 bg와 동일(raw fallback).
                // Phase 4c: status badge용 visible_blocks 정보도 같이 보존.
                let visible_blocks = if self.block_mode == BlockMode::Auto {
                    term.visible_blocks(term.rows())
                } else {
                    Vec::new()
                };
                let block_overlays: Vec<crate::render::BlockOverlay> = {
                    let palette = self.renderer.palette();
                    visible_blocks
                        .iter()
                        .map(|vb| crate::render::BlockOverlay {
                            visible_row_start: vb.visible_row_start,
                            visible_row_end: vb.visible_row_end,
                            bg: palette.block_bg,
                            border_color: palette.block_border,
                        })
                        .collect()
                };
                // Phase 4b-2c-3: gutter_cells = gutter_px / cell_w. compute_tab_viewports에서
                // 이미 cell.width 단위로 align됐으므로 정확한 정수 분할.
                let cell_w = self.renderer.cell_metrics().width.max(1);
                let pane_gutter_cells = (pane_gutter_px as u32 / cell_w) as usize;
                self.renderer.append_term(
                    &self.queue,
                    &term,
                    preedit_for_render,
                    cursor_render,
                    selection,
                    pane_col_offset,
                    pane_row_offset,
                    &block_overlays,
                    pane_gutter_cells,
                );
                // Phase 5: scrollbar — scrollback이 있을 때 우측 마지막 col에 thumb 표시.
                // view_offset 0면 thumb 바닥, scrollback_len이면 천장.
                let scrollback_len = term.scrollback_len();
                let view_offset = term.view_offset();
                let viewport_rows = self.active_tab().panes[idx].viewport.rows;
                if let Some((thumb_top, thumb_size)) =
                    scrollbar_thumb(viewport_rows, scrollback_len, view_offset)
                {
                    let palette = self.renderer.palette();
                    // scrollback view 활성 시 더 강조 — view_offset>0면 block_border(밝음),
                    // 0이면 dim(fg를 bg에 0.3 mix).
                    let thumb_bg = if view_offset > 0 {
                        palette.block_border
                    } else {
                        mix_palette(palette.bg, palette.fg, 0.3)
                    };
                    let scrollbar_col = pane_col_offset + self.active_tab().panes[idx].viewport.cols
                        - 1;
                    let scrollbar_row = pane_row_offset + thumb_top;
                    self.renderer.append_scrollbar_thumb(
                        scrollbar_col,
                        scrollbar_row,
                        thumb_size,
                        thumb_bg,
                    );
                }
                // Phase 4c: status badge — 각 카드의 visible_row_start 우측 끝에 chip text.
                let viewport_cols = {
                    self.active_tab().panes[idx].viewport.cols
                };
                let palette = self.renderer.palette();
                for vb in &visible_blocks {
                    let Some(badge) = status_badge_text(&vb.state, vb.duration_ms) else {
                        continue;
                    };
                    let badge_width = badge.chars().count();
                    if badge_width == 0 || badge_width > viewport_cols {
                        continue;
                    }
                    let badge_col = pane_col_offset + viewport_cols - badge_width;
                    let badge_row = pane_row_offset + vb.visible_row_start;
                    self.renderer.append_text_line(
                        &self.queue,
                        &badge,
                        badge_col,
                        badge_row,
                        badge_width,
                        palette.bg,
                        palette.block_border,
                    );
                }
                // M6-3b: active pane 기준으로 IME composition window 위치 갱신.
                if is_active && !in_scrollback && self.last_ime_cursor != Some((cur.row, cur.col)) {
                    let cell = self.renderer.cell_metrics();
                    let pos = winit::dpi::PhysicalPosition::<f64>::new(
                        ((cur.col + pane_col_offset) as u32 * cell.width) as f64,
                        ((cur.row + pane_row_offset) as u32 * cell.height) as f64,
                    );
                    let size = winit::dpi::PhysicalSize::<u32>::new(cell.width, cell.height);
                    self.window.set_ime_cursor_area(pos, size);
                    self.last_ime_cursor = Some((cur.row, cur.col));
                }
            }
        }
        self.append_tab_bar();
        self.append_split_chrome();
        self.append_find_overlay();
        self.renderer.finish_terms(&self.device, &self.queue);

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("pj001-encoder"),
            });
        self.renderer.draw(&mut encoder, &view);
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    fn append_tab_bar(&mut self) {
        let cell = self.renderer.cell_metrics();
        if cell.width == 0 {
            return;
        }
        let total_cols = (self.surface_config.width / cell.width).max(1) as usize;
        self.renderer.append_fill_row(0, 0, total_cols, TAB_BAR_BG);
        if self.tabs.is_empty() {
            return;
        }

        for idx in 0..self.tabs.len() {
            let (segment_col, segment_cols) = status_segment(0, total_cols, self.tabs.len(), idx);
            let tab = &self.tabs[idx];
            let bg = if tab.id == self.active_tab {
                TAB_ACTIVE_BG
            } else {
                TAB_INACTIVE_BG
            };
            let label = tab_label(idx, &tab.title);
            let text = tab_text(&label, segment_cols);
            self.renderer.append_text_line(
                &self.queue,
                &text,
                segment_col,
                0,
                segment_cols,
                STATUS_FG,
                bg,
            );
        }
    }

    /// 슬라이스 6.5c: find 모드 활성 시 화면 최하단에 `find: <query>_` overlay.
    /// 활성 pane content 마지막 row를 덮는 시각적 트레이드오프(데이터는 보존, 시각만 덮임).
    fn append_find_overlay(&mut self) {
        let Some(find) = self.pending_find.as_ref() else {
            return;
        };
        let cell = self.renderer.cell_metrics();
        if cell.height == 0 || cell.width == 0 {
            return;
        }
        let total_rows = (self.surface_config.height / cell.height).max(1) as usize;
        let total_cols = (self.surface_config.width / cell.width).max(1) as usize;
        let row = total_rows.saturating_sub(1);
        let bg = [0.10, 0.10, 0.16, 1.0];
        let fg = [1.0, 0.78, 0.40, 1.0];
        self.renderer.append_fill_row(0, row, total_cols, bg);
        let text = format!("find: {}_", find.query);
        let width = text.chars().count().min(total_cols);
        self.renderer
            .append_text_line(&self.queue, &text, 0, row, width, fg, bg);
    }

    fn append_split_chrome(&mut self) {
        if self.active_tab().panes.len() < 2 {
            return;
        }

        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let cell = self.renderer.cell_metrics();
        let content_size = tab_content_size(size, cell);
        let root = self.active_tab().root.clone();
        for mut divider in layout::vertical_dividers(&root, content_size, cell) {
            divider.row += TAB_BAR_ROWS;
            self.renderer
                .append_fill_column(divider.col, divider.row, divider.height, DIVIDER_BG);
        }
        for mut divider in layout::horizontal_dividers(&root, content_size, cell) {
            divider.row += TAB_BAR_ROWS;
            self.renderer
                .append_fill_row(divider.col, divider.row, divider.width, DIVIDER_BG);
        }

        let mut status_items = Vec::new();
        let active = self.active_tab().active;
        for pane in &self.active_tab().panes {
            let Some(status_row) = pane.viewport.status_row else {
                continue;
            };
            let session = &self.sessions[&pane.session];
            let alive = session.alive;
            let state = if !alive {
                "DEAD"
            } else if pane.id == active && self.pending_quick_spawn.is_some() {
                "SPAWN"
            } else if pane.id == active {
                "ACTIVE"
            } else {
                "READY"
            };
            let bg = if !alive {
                STATUS_DEAD_BG
            } else if pane.id == active {
                STATUS_ACTIVE_BG
            } else {
                STATUS_INACTIVE_BG
            };
            let text = status_text(state, &session.title, pane.viewport.cols);
            status_items.push((
                status_row,
                pane.viewport.col_offset,
                pane.viewport.cols,
                text,
                bg,
            ));
        }

        let mut rendered = vec![false; status_items.len()];
        for idx in 0..status_items.len() {
            if rendered[idx] {
                continue;
            }
            let (row, col, cols, _, _) = &status_items[idx];
            let group: Vec<usize> = status_items
                .iter()
                .enumerate()
                .filter_map(
                    |(candidate_idx, (candidate_row, candidate_col, candidate_cols, _, _))| {
                        (!rendered[candidate_idx]
                            && candidate_row == row
                            && candidate_col == col
                            && candidate_cols == cols)
                            .then_some(candidate_idx)
                    },
                )
                .collect();
            let group_len = group.len();
            for (group_idx, item_idx) in group.into_iter().enumerate() {
                rendered[item_idx] = true;
                let (status_row, group_col, group_cols, text, bg) = &status_items[item_idx];
                let (segment_col, segment_cols) =
                    status_segment(*group_col, *group_cols, group_len, group_idx);
                self.renderer.append_text_line(
                    &self.queue,
                    text,
                    segment_col,
                    *status_row,
                    segment_cols,
                    STATUS_FG,
                    *bg,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::CellMetrics;

    fn cell() -> CellMetrics {
        CellMetrics {
            width: 10,
            height: 20,
            baseline: 15.0,
        }
    }

    #[test]
    fn physical_font_size_scales_logical_default() {
        assert_eq!(physical_font_size(14.0, 1.0), 14.0);
        assert_eq!(physical_font_size(14.0, 2.0), 28.0);
    }

    #[test]
    fn physical_font_size_ignores_invalid_scale() {
        assert_eq!(physical_font_size(14.0, 0.0), 14.0);
        assert_eq!(physical_font_size(14.0, f64::NAN), 14.0);
    }

    #[test]
    fn font_size_is_clamped_before_physical_scaling() {
        assert_eq!(clamp_font_size(5.0), MIN_FONT_SIZE);
        assert_eq!(clamp_font_size(80.0), MAX_FONT_SIZE);
        assert_eq!(clamp_font_size(f32::NAN), DEFAULT_FONT_SIZE);
        assert_eq!(
            physical_font_size(MAX_FONT_SIZE + 1.0, 2.0),
            MAX_FONT_SIZE * 2.0
        );
    }

    #[test]
    fn compute_viewports_single_uses_full_window() {
        let layouts = compute_viewports(PhysicalSize::new(100, 80), cell(), 1);

        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].cols, 10);
        assert_eq!(layouts[0].rows, 4);
        assert_eq!(layouts[0].col_offset, 0);
        assert_eq!(layouts[0].status_row, None);
        assert_eq!(layouts[0].x_px, 0);
        assert_eq!(layouts[0].y_px, 0);
        assert_eq!(layouts[0].width_px, 100);
        assert_eq!(layouts[0].height_px, 80);
    }

    #[test]
    fn compute_viewports_split_reserves_divider_column() {
        let layouts = compute_viewports(PhysicalSize::new(100, 80), cell(), 2);

        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].cols, 5);
        assert_eq!(layouts[0].col_offset, 0);
        assert_eq!(layouts[0].x_px, 0);
        assert_eq!(layouts[1].cols, 4);
        assert_eq!(layouts[1].col_offset, 6);
        assert_eq!(layouts[1].x_px, 60);
        assert_eq!(layouts[0].rows, 3);
        assert_eq!(layouts[1].rows, 3);
        assert_eq!(layouts[0].status_row, Some(3));
        assert_eq!(layouts[1].status_row, Some(3));
    }

    #[test]
    fn compute_viewports_split_keeps_minimum_cells() {
        let layouts = compute_viewports(PhysicalSize::new(1, 1), cell(), 2);

        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].cols, 1);
        assert_eq!(layouts[1].cols, 1);
        assert_eq!(layouts[0].col_offset, 0);
        assert_eq!(layouts[1].col_offset, 0);
        assert_eq!(layouts[0].x_px, 0);
        assert_eq!(layouts[1].x_px, 0);
        assert_eq!(layouts[0].width_px, 1);
        assert_eq!(layouts[1].width_px, 1);
        assert_eq!(layouts[0].rows, 1);
        assert_eq!(layouts[1].rows, 1);
        assert_eq!(layouts[0].status_row, None);
        assert_eq!(layouts[1].status_row, None);
    }

    #[test]
    fn compute_viewports_split_uses_no_divider_when_only_two_columns_fit() {
        let layouts = compute_viewports(PhysicalSize::new(20, 80), cell(), 2);

        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].cols, 1);
        assert_eq!(layouts[0].col_offset, 0);
        assert_eq!(layouts[1].cols, 1);
        assert_eq!(layouts[1].col_offset, 1);
        assert_eq!(layouts[1].x_px, 10);
        assert_eq!(layouts[0].col_offset + layouts[0].cols, 1);
        assert_eq!(layouts[1].col_offset + layouts[1].cols, 2);
    }

    #[test]
    fn compute_tab_viewports_reserves_top_tab_row() {
        let root = Layout::Pane(PaneId(0));
        let layouts = compute_tab_viewports(&root, PhysicalSize::new(100, 80), cell(), 0);
        let viewport = layouts[&PaneId(0)];

        assert_eq!(viewport.cols, 10);
        assert_eq!(viewport.rows, 3);
        assert_eq!(viewport.col_offset, 0);
        assert_eq!(viewport.row_offset, 1);
        assert_eq!(viewport.status_row, None);
        assert_eq!(viewport.x_px, 0);
        assert_eq!(viewport.y_px, 20);
        assert_eq!(viewport.width_px, 100);
        assert_eq!(viewport.height_px, 60);
    }

    #[test]
    fn compute_tab_viewports_shifts_split_status_rows_below_tab_bar() {
        let root = Layout::from_initial_panes(&[PaneId(0), PaneId(1)]);
        let layouts = compute_tab_viewports(&root, PhysicalSize::new(100, 80), cell(), 0);
        let left = layouts[&PaneId(0)];
        let right = layouts[&PaneId(1)];

        assert_eq!(left.rows, 2);
        assert_eq!(right.rows, 2);
        assert_eq!(left.row_offset, 1);
        assert_eq!(right.row_offset, 1);
        assert_eq!(left.status_row, Some(3));
        assert_eq!(right.status_row, Some(3));
        assert_eq!(left.y_px, 20);
        assert_eq!(right.y_px, 20);
    }

    #[test]
    fn compute_tab_viewports_keeps_one_content_row_when_tab_bar_consumes_height() {
        let root = Layout::Pane(PaneId(0));
        let layouts = compute_tab_viewports(&root, PhysicalSize::new(100, 10), cell(), 0);
        let viewport = layouts[&PaneId(0)];

        assert_eq!(viewport.cols, 10);
        assert_eq!(viewport.rows, 1);
        assert_eq!(viewport.row_offset, 1);
        assert_eq!(viewport.status_row, None);
        assert_eq!(viewport.y_px, 20);
    }

    #[test]
    fn is_modifier_only_recognizes_modifier_keys() {
        use winit::keyboard::{Key, NamedKey};
        assert!(is_modifier_only_key(&Key::Named(NamedKey::Control)));
        assert!(is_modifier_only_key(&Key::Named(NamedKey::Shift)));
        assert!(is_modifier_only_key(&Key::Named(NamedKey::Alt)));
        assert!(is_modifier_only_key(&Key::Named(NamedKey::Super)));
        assert!(is_modifier_only_key(&Key::Named(NamedKey::Meta)));
        assert!(is_modifier_only_key(&Key::Named(NamedKey::Fn)));
        assert!(is_modifier_only_key(&Key::Named(NamedKey::CapsLock)));
    }

    #[test]
    fn is_modifier_only_rejects_normal_keys() {
        use winit::keyboard::{Key, NamedKey, SmolStr};
        assert!(!is_modifier_only_key(&Key::Character(SmolStr::new("a"))));
        assert!(!is_modifier_only_key(&Key::Named(NamedKey::Enter)));
        assert!(!is_modifier_only_key(&Key::Named(NamedKey::ArrowUp)));
        assert!(!is_modifier_only_key(&Key::Named(NamedKey::PageDown)));
        assert!(!is_modifier_only_key(&Key::Named(NamedKey::Escape)));
    }

    #[test]
    fn scrollbar_thumb_none_when_no_scrollback() {
        assert!(scrollbar_thumb(10, 0, 0).is_none());
    }

    #[test]
    fn scrollbar_thumb_bottom_when_view_offset_zero() {
        // scrollback 100, viewport 10, view_offset 0 (bottom).
        // total = 110, thumb_size = 10*10/110 = 0 → max(1) = 1.
        // bottom_anchored = 100, top_offset = 100*10/110 = 9. clamp to (10-1)=9.
        let (top, size) = scrollbar_thumb(10, 100, 0).unwrap();
        assert_eq!(size, 1);
        assert_eq!(top, 9);
    }

    #[test]
    fn scrollbar_thumb_top_when_view_offset_max() {
        // view_offset = scrollback_len → bottom_anchored = 0, top_offset = 0.
        let (top, size) = scrollbar_thumb(10, 100, 100).unwrap();
        assert_eq!(top, 0);
        assert!(size >= 1);
    }

    #[test]
    fn scrollbar_thumb_middle_position() {
        // scrollback 10, view_offset 5 (middle), viewport 10. total 20.
        // bottom_anchored = 5, top_offset = 5*10/20 = 2.
        // thumb_size = 10*10/20 = 5. clamp (10-5)=5. top=2.
        let (top, size) = scrollbar_thumb(10, 10, 5).unwrap();
        assert_eq!(top, 2);
        assert_eq!(size, 5);
    }

    #[test]
    fn scrollbar_thumb_zero_viewport() {
        assert!(scrollbar_thumb(0, 100, 50).is_none());
    }

    #[test]
    fn status_badge_text_prompt_none() {
        assert!(status_badge_text(&BlockState::Prompt, None).is_none());
        assert!(status_badge_text(&BlockState::Prompt, Some(100)).is_none());
    }

    #[test]
    fn status_badge_text_running_run_chip() {
        assert_eq!(
            status_badge_text(&BlockState::Command, None),
            Some(" RUN ".to_string())
        );
        assert_eq!(
            status_badge_text(&BlockState::Running, Some(500)),
            Some(" RUN ".to_string())
        );
    }

    #[test]
    fn status_badge_text_completed_success_with_duration() {
        let state = BlockState::Completed {
            exit_code: Some(0),
        };
        assert_eq!(
            status_badge_text(&state, Some(123)),
            Some(" 123ms ".to_string())
        );
        // duration None → None badge (값 없으면 표시 X)
        assert!(status_badge_text(&state, None).is_none());
    }

    #[test]
    fn status_badge_text_completed_failure_shows_exit_code() {
        let state = BlockState::Completed {
            exit_code: Some(1),
        };
        assert_eq!(
            status_badge_text(&state, Some(50)),
            Some(" x 1 ".to_string())
        );
    }

    #[test]
    fn status_badge_text_completed_unknown_exit_question() {
        let state = BlockState::Completed { exit_code: None };
        assert_eq!(
            status_badge_text(&state, Some(50)),
            Some(" ? ".to_string())
        );
    }

    #[test]
    fn status_badge_text_abandoned_dash_dash() {
        let state = BlockState::Abandoned {
            reason: crate::block::AbandonReason::Reset,
        };
        assert_eq!(status_badge_text(&state, None), Some(" -- ".to_string()));
    }

    #[test]
    fn compute_tab_viewports_gutter_shifts_col_offset_left() {
        // Phase 4b-1c-fix: gutter는 좌측에 위치. content cell이 우측으로 shift되어야 함.
        // cell.width=10, gutter_px=20 → gutter_cells=2. cols 10→8, col_offset 0→2.
        let root = Layout::Pane(PaneId(0));
        let layouts = compute_tab_viewports(&root, PhysicalSize::new(100, 80), cell(), 20);
        let viewport = layouts[&PaneId(0)];

        assert_eq!(viewport.cols, 8);
        assert_eq!(viewport.col_offset, 2);
        assert_eq!(viewport.gutter_px, 20);
        assert_eq!(viewport.content_x_px, 20);
        assert_eq!(viewport.width_px, 80);
    }

    // === clamp_pos_to_viewport_cell: 드래그가 창 밖으로 나가도 selection.pane 가장자리로 clamp ===

    fn vp(cols: usize, rows: usize, col_offset: usize, row_offset: usize) -> PaneViewport {
        PaneViewport {
            cols,
            rows,
            col_offset,
            row_offset,
            status_row: None,
            x_px: 0,
            y_px: 0,
            width_px: 0,
            height_px: 0,
            gutter_px: 0,
            content_x_px: 0,
        }
    }

    fn cm(w: u32, h: u32) -> CellMetrics {
        CellMetrics {
            width: w,
            height: h,
            baseline: 0.0,
        }
    }

    #[test]
    fn clamp_pos_negative_pins_to_origin() {
        // pos가 (-50, -10)이면 row=0, col=0.
        let p = PhysicalPosition::new(-50.0, -10.0);
        let v = vp(10, 5, 0, 0);
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(8, 16)), (0, 0));
    }

    #[test]
    fn clamp_pos_beyond_right_edge_pins_to_last_col() {
        // 8px cells, 10 cols → 80px width. pos.x = 200 → 가장자리 col = 9.
        let p = PhysicalPosition::new(200.0, 16.0);
        let v = vp(10, 5, 0, 0);
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(8, 16)), (1, 9));
    }

    #[test]
    fn clamp_pos_beyond_bottom_pins_to_last_row() {
        let p = PhysicalPosition::new(8.0, 1000.0);
        let v = vp(10, 5, 0, 0);
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(8, 16)), (4, 1));
    }

    #[test]
    fn clamp_pos_inside_viewport_returns_local_cell() {
        // pos = (24, 32). col=24/8=3, row=32/16=2.
        let p = PhysicalPosition::new(24.0, 32.0);
        let v = vp(10, 5, 0, 0);
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(8, 16)), (2, 3));
    }

    #[test]
    fn clamp_pos_respects_viewport_offset() {
        // viewport col_offset=5, row_offset=2.
        // pos = (16, 32). abs col=2, abs row=2. local col=2-5 saturating→0, local row=2-2=0.
        let p = PhysicalPosition::new(16.0, 32.0);
        let v = vp(10, 5, 5, 2);
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(8, 16)), (0, 0));
        // pos가 viewport 안: abs col=10, abs row=4 → local (2, 5).
        let p2 = PhysicalPosition::new(80.0, 64.0);
        assert_eq!(clamp_pos_to_viewport_cell(p2, v, cm(8, 16)), (2, 5));
    }

    #[test]
    fn clamp_pos_handles_zero_cell_metrics_safely() {
        let p = PhysicalPosition::new(40.0, 40.0);
        let v = vp(10, 5, 0, 0);
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(0, 16)), (0, 0));
        assert_eq!(clamp_pos_to_viewport_cell(p, v, cm(8, 0)), (0, 0));
    }

    #[test]
    fn status_segment_divides_shared_status_region() {
        assert_eq!(status_segment(0, 10, 2, 0), (0, 5));
        assert_eq!(status_segment(0, 10, 2, 1), (5, 5));
        assert_eq!(status_segment(6, 5, 3, 0), (6, 1));
        assert_eq!(status_segment(6, 5, 3, 1), (7, 2));
        assert_eq!(status_segment(6, 5, 3, 2), (9, 2));
    }

    #[test]
    fn status_text_fits_available_columns() {
        assert_eq!(status_text("READY", "shell", 13), " READY shell ");
        assert_eq!(status_text("READY", "shell", 12), " READY ");
        assert_eq!(status_text("READY", "shell", 6), " R ");
        assert_eq!(status_text("READY", "shell", 2), "");
    }

    #[test]
    fn status_text_uses_display_width_for_wide_titles() {
        assert_eq!(status_text("READY", "셸", 10), " READY 셸 ");
        assert_eq!(status_text("READY", "셸", 9), " READY ");
    }

    #[test]
    fn quick_spawn_hint_lists_sorted_unique_keys() {
        let presets = vec![
            QuickSpawnPreset {
                key: 'x',
                spec: SessionSpec {
                    title: "bash".to_string(),
                    command: CommandSpec::Custom("/bin/bash".to_string()),
                },
            },
            QuickSpawnPreset {
                key: 's',
                spec: SessionSpec {
                    title: "shell".to_string(),
                    command: CommandSpec::Shell,
                },
            },
            QuickSpawnPreset {
                key: 'C',
                spec: SessionSpec {
                    title: "fish".to_string(),
                    command: CommandSpec::Custom("/usr/local/bin/fish".to_string()),
                },
            },
            QuickSpawnPreset {
                key: 'c',
                spec: SessionSpec {
                    title: "fish duplicate".to_string(),
                    command: CommandSpec::Custom("/usr/local/bin/fish".to_string()),
                },
            },
        ];

        assert_eq!(quick_spawn_hint(&presets), "SPAWN csx");
        assert_eq!(quick_spawn_hint(&[]), "SPAWN");
    }

    #[test]
    fn quick_spawn_timeout_triggers_after_deadline() {
        let started_at = Instant::now();

        assert!(!quick_spawn_elapsed_timed_out(
            started_at,
            started_at + Duration::from_millis(QUICK_SPAWN_TIMEOUT_MS),
        ));
        assert!(quick_spawn_elapsed_timed_out(
            started_at,
            started_at + Duration::from_millis(QUICK_SPAWN_TIMEOUT_MS + 1),
        ));
    }

    #[test]
    fn selection_text_abs_extracts_scrollback_row() {
        // Phase 5 후속: scrollback으로 밀린 row의 text도 abs row로 추출 가능.
        let mut term = Term::new(10, 2);
        for ch in "abc".chars() {
            term.print(ch);
        }
        term.set_cursor(1, 0);
        for ch in "def".chars() {
            term.print(ch);
        }
        term.set_cursor(1, 3);
        // newline → main row 0 "abc"가 scrollback으로 push, cursor row 1로 유지.
        term.newline();

        // abs row 0 = scrollback "abc", abs row 1 = main row 0 "def".
        let range = SelectionRange::new((0, 0), (1, 3));
        let text = selection_text_abs(&term, range);
        assert_eq!(text, "abc\ndef");
    }

    #[test]
    fn selection_text_abs_returns_empty_for_evicted_rows() {
        // evict된 abs (oldest_kept_abs 미만)는 cell_at_abs가 None → 빈 line.
        let mut term = Term::new(5, 1);
        for ch in "x".chars() {
            term.print(ch);
        }
        // abs row 100은 존재 안 함 (way past main).
        let range = SelectionRange::new((100, 0), (100, 3));
        let text = selection_text_abs(&term, range);
        assert_eq!(text, "");
    }

    #[test]
    fn cell_at_abs_scrollback_and_main() {
        let mut term = Term::new(5, 2);
        for ch in "ab".chars() {
            term.print(ch);
        }
        term.set_cursor(1, 0);
        for ch in "cd".chars() {
            term.print(ch);
        }
        term.set_cursor(1, 2);
        term.newline(); // "ab" → scrollback. main row 0 = "cd", row 1 = "".

        // abs 0 = scrollback "ab".
        assert_eq!(term.cell_at_abs(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(term.cell_at_abs(0, 1).map(|c| c.ch), Some('b'));
        // abs 1 = main row 0 "cd".
        assert_eq!(term.cell_at_abs(1, 0).map(|c| c.ch), Some('c'));
        assert_eq!(term.cell_at_abs(1, 1).map(|c| c.ch), Some('d'));
        // abs 2 = main row 1 (empty).
        assert_eq!(term.cell_at_abs(2, 0).map(|c| c.ch), Some(' '));
        // abs 100 = 없음.
        assert!(term.cell_at_abs(100, 0).is_none());
    }

    #[test]
    fn selection_text_serializes_visible_cells() {
        // caret 모델: end caret은 마지막 cell의 다음 boundary. row 0 col 1~7 + row 1 col 0~2.
        // 즉 "ello" (row 0 col 1..8, cell 1~7) + "wor" (row 1 col 0..3, cell 0~2).
        let mut term = Term::new(8, 3);
        for ch in "hello".chars() {
            term.print(ch);
        }
        term.set_cursor(1, 0);
        for ch in "world".chars() {
            term.print(ch);
        }

        assert_eq!(
            selection_text(&term, SelectionRange::new((0, 1), (1, 3))),
            "ello\nwor"
        );
    }

    #[test]
    fn selection_text_skips_wide_continuation_cells() {
        // caret 모델: "한"(WIDE, cell 0-1) + "a"(cell 2) → caret [0, 3).
        let mut term = Term::new(6, 2);
        term.print('한');
        term.print('a');

        assert_eq!(
            selection_text(&term, SelectionRange::new((0, 0), (0, 3))),
            "한a"
        );
    }

    #[test]
    fn word_selection_range_selects_alnum_and_underscore() {
        // caret 모델: "foo_bar"는 cell 4~10. caret [4, 11).
        let mut term = Term::new(16, 1);
        for ch in "run foo_bar!".chars() {
            term.print(ch);
        }

        let range = word_selection_range(&term, 0, 6).expect("word range");
        assert_eq!(range, SelectionRange::new((0, 4), (0, 11)));
        assert_eq!(selection_text(&term, range), "foo_bar");
        assert!(word_selection_range(&term, 0, 11).is_none());
    }

    #[test]
    fn shell_quoted_path_for_drop_uses_posix_single_quotes() {
        assert_eq!(
            shell_quoted_path_for_drop(Path::new("/tmp/a b/$x(1)")),
            Some("'/tmp/a b/$x(1)' ".to_string())
        );
        assert_eq!(
            shell_quoted_path_for_drop(Path::new("/tmp/it's/한글")),
            Some("'/tmp/it'\\''s/한글' ".to_string())
        );
    }

    #[test]
    fn shell_quoted_path_for_drop_rejects_control_chars() {
        assert_eq!(shell_quoted_path_for_drop(Path::new("/tmp/a\nb")), None);
        assert_eq!(shell_quoted_path_for_drop(Path::new("/tmp/a\u{1b}b")), None);
    }

    #[cfg(unix)]
    #[test]
    fn shell_quoted_path_for_drop_rejects_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::path::PathBuf;

        let path = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]));

        assert_eq!(shell_quoted_path_for_drop(&path), None);
    }

    #[test]
    fn paste_payload_wraps_bracketed_paste() {
        assert_eq!(paste_payload_bytes("abc", false), Some(b"abc".to_vec()));
        assert_eq!(
            paste_payload_bytes("abc", true),
            Some(b"\x1b[200~abc\x1b[201~".to_vec())
        );
        // 입력에 \x1b[201~ 있어도 sanitize가 ESC를 제거하므로 안전. 결과는 "[201~"가 됨.
        assert_eq!(
            paste_payload_bytes("a\x1b[201~b", true),
            Some(b"\x1b[200~a[201~b\x1b[201~".to_vec())
        );
    }

    #[test]
    fn sanitize_paste_strips_esc_and_c0_controls() {
        // ESC + BEL + NUL strip, tab/LF 보존.
        assert_eq!(
            super::sanitize_paste_text("a\x1bb\x07c\x00d\te\nf"),
            "abcd\te\nf"
        );
    }

    #[test]
    fn sanitize_paste_converts_cr_to_lf() {
        // paste-execution 방어: CR → LF.
        assert_eq!(super::sanitize_paste_text("line1\rline2"), "line1\nline2");
        assert_eq!(
            super::sanitize_paste_text("line1\r\nline2"),
            "line1\n\nline2"
        );
    }

    #[test]
    fn sanitize_paste_strips_c1_controls() {
        // U+0080..U+009F (C1) 제거.
        let s = format!("a{}b{}c", '\u{0085}', '\u{009F}');
        assert_eq!(super::sanitize_paste_text(&s), "abc");
    }

    #[test]
    fn sanitize_paste_preserves_unicode_text() {
        // 일반 텍스트 + 한글/이모지 보존.
        let s = "안녕 hello 🚀";
        assert_eq!(super::sanitize_paste_text(s), s);
    }

    #[test]
    fn sanitize_paste_strips_del() {
        assert_eq!(super::sanitize_paste_text("ab\x7fc"), "abc");
    }

    #[test]
    fn mouse_report_sgr_press() {
        // col=10 row=3 (0-based) → 11/4 1-based. Left button=0. SGR press.
        let bytes = super::encode_mouse_report(0, true, false, false, false, false, 10, 3, true);
        assert_eq!(bytes, b"\x1b[<0;11;4M".to_vec());
    }

    #[test]
    fn mouse_report_sgr_release() {
        let bytes = super::encode_mouse_report(0, false, false, false, false, false, 10, 3, true);
        assert_eq!(bytes, b"\x1b[<0;11;4m".to_vec());
    }

    #[test]
    fn mouse_report_sgr_modifiers_combine() {
        // shift+ctrl+alt = 4+8+16 = 28 → button 0+28 = 28.
        let bytes = super::encode_mouse_report(0, true, false, true, true, true, 0, 0, true);
        assert_eq!(bytes, b"\x1b[<28;1;1M".to_vec());
    }

    #[test]
    fn mouse_report_sgr_motion_flag() {
        let bytes = super::encode_mouse_report(0, false, true, false, false, false, 5, 5, true);
        // motion +32, button 0 → 32. press/motion = 'M'.
        assert_eq!(bytes, b"\x1b[<32;6;6M".to_vec());
    }

    #[test]
    fn mouse_report_legacy_press() {
        // col=10 row=3 → 11/4 1-based. legacy bytes = 32+button=32, 32+11=43='+', 32+4=36='$'
        let bytes = super::encode_mouse_report(0, true, false, false, false, false, 10, 3, false);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 43, 36]);
    }

    #[test]
    fn mouse_report_legacy_release_uses_button_3() {
        let bytes = super::encode_mouse_report(0, false, false, false, false, false, 10, 3, false);
        // legacy: release encodes button=3 (32+3=35='#').
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 43, 36]);
    }

    #[test]
    fn cmd_arrow_left_right_maps_to_home_end_bytes() {
        use winit::keyboard::NamedKey;

        assert_eq!(
            cmd_named_key_bytes(&NamedKey::ArrowLeft),
            Some(&b"\x1bOH"[..])
        );
        assert_eq!(
            cmd_named_key_bytes(&NamedKey::ArrowRight),
            Some(&b"\x1bOF"[..])
        );
        assert_eq!(cmd_named_key_bytes(&NamedKey::ArrowUp), None);
        assert_eq!(cmd_named_key_bytes(&NamedKey::ArrowDown), None);
    }

    #[test]
    fn tab_text_fits_available_columns() {
        assert_eq!(tab_text("1", 3), " 1 ");
        assert_eq!(tab_text("10", 3), " 1");
        assert_eq!(tab_text("10", 1), "");
    }

    #[test]
    fn tab_label_prefixes_one_based_index() {
        assert_eq!(tab_label(0, "shell"), "1 shell");
        assert_eq!(tab_label(2, "bash"), "3 bash");
    }

    #[test]
    fn tab_index_at_col_matches_status_segment_boundaries() {
        assert_eq!(tab_index_at_col(5, 3, 0), Some(0));
        assert_eq!(tab_index_at_col(5, 3, 1), Some(1));
        assert_eq!(tab_index_at_col(5, 3, 2), Some(1));
        assert_eq!(tab_index_at_col(5, 3, 3), Some(2));
        assert_eq!(tab_index_at_col(5, 3, 4), Some(2));
        assert_eq!(tab_index_at_col(5, 3, 5), None);
    }

    #[test]
    fn tab_index_at_col_handles_tight_tab_bars() {
        assert_eq!(tab_index_at_col(4, 3, 0), Some(0));
        assert_eq!(tab_index_at_col(4, 3, 1), Some(1));
        assert_eq!(tab_index_at_col(4, 3, 2), Some(2));
        assert_eq!(tab_index_at_col(4, 3, 3), Some(2));
        assert_eq!(tab_index_at_col(0, 3, 0), None);
        assert_eq!(tab_index_at_col(4, 0, 0), None);
    }

    #[test]
    fn ordinal_from_digit_key_prefers_physical_digit_for_modified_symbols() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            ordinal_from_digit_key(Some("!"), Some(KeyCode::Digit1)),
            Some(1)
        );
        assert_eq!(
            ordinal_from_digit_key(Some("@"), Some(KeyCode::Digit2)),
            Some(2)
        );
        assert_eq!(ordinal_from_digit_key(Some("2"), None), Some(2));
        assert_eq!(
            ordinal_from_digit_key(Some("0"), Some(KeyCode::Digit0)),
            None
        );
    }

    #[test]
    fn cmd_shortcut_routes_tab_and_pane_ordinals() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            cmd_shortcut(Some("1"), Some(KeyCode::Digit1), false, false),
            Some(CmdShortcut::TabOrdinal(1))
        );
        assert_eq!(
            cmd_shortcut(Some("!"), Some(KeyCode::Digit1), false, true),
            Some(CmdShortcut::PaneOrdinal(1))
        );
        assert_eq!(
            cmd_shortcut(Some("!"), Some(KeyCode::Digit1), true, false),
            None
        );
    }

    #[test]
    fn cmd_shortcut_routes_panes_tabs_and_closers() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            cmd_shortcut(Some("["), Some(KeyCode::BracketLeft), false, false),
            Some(CmdShortcut::PrevPane)
        );
        assert_eq!(
            cmd_shortcut(Some("]"), Some(KeyCode::BracketRight), false, false),
            Some(CmdShortcut::NextPane)
        );
        assert_eq!(
            cmd_shortcut(Some("{"), Some(KeyCode::BracketLeft), true, false),
            Some(CmdShortcut::PrevTab)
        );
        assert_eq!(
            cmd_shortcut(Some("}"), Some(KeyCode::BracketRight), true, false),
            Some(CmdShortcut::NextTab)
        );
        assert_eq!(
            cmd_shortcut(Some("w"), Some(KeyCode::KeyW), false, false),
            Some(CmdShortcut::ClosePaneOrTab)
        );
        assert_eq!(
            cmd_shortcut(Some("w"), Some(KeyCode::KeyW), true, false),
            Some(CmdShortcut::CloseTab)
        );
        assert_eq!(
            cmd_shortcut(Some("c"), Some(KeyCode::KeyC), false, false),
            Some(CmdShortcut::Copy)
        );
        assert_eq!(
            cmd_shortcut(Some("v"), Some(KeyCode::KeyV), false, false),
            Some(CmdShortcut::Paste)
        );
    }

    #[test]
    fn cmd_shortcut_routes_new_tab_and_new_pane() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            cmd_shortcut(Some("t"), Some(KeyCode::KeyT), false, false),
            Some(CmdShortcut::NewTab)
        );
        assert_eq!(
            cmd_shortcut(Some("n"), Some(KeyCode::KeyN), false, false),
            Some(CmdShortcut::NewPane)
        );
        assert_eq!(
            cmd_shortcut(Some("n"), Some(KeyCode::KeyN), true, false),
            Some(CmdShortcut::QuickSpawnStart)
        );
        assert_eq!(
            cmd_shortcut(Some("r"), Some(KeyCode::KeyR), false, false),
            Some(CmdShortcut::RespawnSession)
        );
        assert_eq!(
            cmd_shortcut(Some("k"), Some(KeyCode::KeyK), false, false),
            Some(CmdShortcut::ClearBuffer)
        );
        assert_eq!(
            cmd_shortcut(Some("k"), Some(KeyCode::KeyK), true, false),
            Some(CmdShortcut::ClearScrollback)
        );
        assert_eq!(
            cmd_shortcut(Some("K"), Some(KeyCode::KeyK), true, false),
            Some(CmdShortcut::ClearScrollback)
        );
        assert_eq!(
            cmd_shortcut(Some("k"), Some(KeyCode::KeyK), false, true),
            Some(CmdShortcut::ClearScrollback)
        );
        assert_eq!(
            cmd_shortcut(Some("˚"), Some(KeyCode::KeyK), false, true),
            Some(CmdShortcut::ClearScrollback)
        );
    }

    #[test]
    fn cmd_shortcut_routes_find() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            cmd_shortcut(Some("f"), Some(KeyCode::KeyF), false, false),
            Some(CmdShortcut::Find)
        );
    }

    #[test]
    fn cmd_shortcut_routes_font_zoom() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            cmd_shortcut(Some("="), Some(KeyCode::Equal), false, false),
            Some(CmdShortcut::FontZoomIn)
        );
        assert_eq!(
            cmd_shortcut(Some("+"), Some(KeyCode::Equal), true, false),
            Some(CmdShortcut::FontZoomIn)
        );
        assert_eq!(
            cmd_shortcut(Some("-"), Some(KeyCode::Minus), false, false),
            Some(CmdShortcut::FontZoomOut)
        );
        assert_eq!(
            cmd_shortcut(Some("0"), Some(KeyCode::Digit0), false, false),
            Some(CmdShortcut::FontZoomReset)
        );
        assert_eq!(
            cmd_shortcut(Some("="), Some(KeyCode::Equal), false, true),
            None
        );
    }

    #[test]
    fn close_decision_escalates_cmd_w_from_pane_to_tab_to_exit() {
        assert_eq!(
            close_decision(CmdShortcut::ClosePaneOrTab, 2, 1),
            CloseDecision::ClosePane
        );
        assert_eq!(
            close_decision(CmdShortcut::ClosePaneOrTab, 1, 2),
            CloseDecision::CloseTab
        );
        assert_eq!(
            close_decision(CmdShortcut::ClosePaneOrTab, 1, 1),
            CloseDecision::Exit
        );
    }

    #[test]
    fn close_decision_cmd_shift_w_targets_tab_or_exit() {
        assert_eq!(
            close_decision(CmdShortcut::CloseTab, 3, 2),
            CloseDecision::CloseTab
        );
        assert_eq!(
            close_decision(CmdShortcut::CloseTab, 3, 1),
            CloseDecision::Exit
        );
    }

    #[test]
    fn config_single_shell_builds_one_session() {
        let config = Config::single_shell(Some("/bin/zsh".to_string()));
        let sessions = config.pane_specs().unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "shell");
        assert_eq!(
            sessions[0].command,
            CommandSpec::Custom("/bin/zsh".to_string())
        );
    }

    #[test]
    fn config_vertical_split_builds_pair() {
        let config = Config::vertical_split(
            SessionSpec {
                title: "left".to_string(),
                command: CommandSpec::Custom("/bin/zsh".to_string()),
            },
            SessionSpec {
                title: "right".to_string(),
                command: CommandSpec::Custom("/bin/bash".to_string()),
            },
        );
        let sessions = config.pane_specs().unwrap();

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].title, "left");
        assert_eq!(sessions[1].title, "right");
        assert_eq!(
            config.initial_layout,
            InitialLayout::Split {
                direction: SplitDirection::Vertical,
                first: 0,
                second: 1,
            }
        );
    }

    #[test]
    fn config_rejects_missing_layout_session() {
        let config = Config {
            sessions: vec![SessionSpec {
                title: "one".to_string(),
                command: CommandSpec::Shell,
            }],
            initial_layout: InitialLayout::Split {
                direction: SplitDirection::Vertical,
                first: 0,
                second: 1,
            },
            quick_spawn_presets: default_quick_spawn_presets(),
            hooks: Hooks::default(),
            theme: None,
            block_mode: BlockMode::default(),
            backdrop_enabled: None,
        };

        assert!(config.pane_specs().is_err());
    }

    #[test]
    fn config_rejects_duplicate_split_session() {
        let config = Config {
            sessions: vec![SessionSpec {
                title: "one".to_string(),
                command: CommandSpec::Shell,
            }],
            initial_layout: InitialLayout::Split {
                direction: SplitDirection::Vertical,
                first: 0,
                second: 0,
            },
            quick_spawn_presets: default_quick_spawn_presets(),
            hooks: Hooks::default(),
            theme: None,
            block_mode: BlockMode::default(),
            backdrop_enabled: None,
        };

        assert!(config.pane_specs().is_err());
    }
}
