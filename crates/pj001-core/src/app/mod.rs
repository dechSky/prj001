pub mod event;
mod input;
mod layout;
mod session;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use unicode_width::UnicodeWidthStr;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ImePurpose, Window, WindowId};

use crate::error::{Error, Result};
use crate::grid::Term;
use crate::pty::PtyHandle;
use crate::render::{CursorRender, Renderer};
use event::{IdAllocator, PaneId, SessionId, TabId, UserEvent};
#[cfg(test)]
use layout::SplitRatio;
use layout::{Layout, RatioDirection, SplitAxis};
use session::Session;

const FONT_SIZE: f32 = 14.0;
const MIN_WINDOW_WIDTH: u32 = 720;
const MIN_WINDOW_HEIGHT: u32 = 420;
const MIN_PANE_COLS: usize = 30;
const MIN_PANE_ROWS: usize = 5;
const TAB_BAR_ROWS: usize = 1;
const CURSOR_BLINK_MS: u64 = 500;
const TAB_BAR_BG: [f32; 4] = [0.10, 0.11, 0.13, 1.0];
const TAB_ACTIVE_BG: [f32; 4] = [0.18, 0.32, 0.42, 1.0];
const TAB_INACTIVE_BG: [f32; 4] = [0.15, 0.16, 0.18, 1.0];
const STATUS_FG: [f32; 4] = [0.92, 0.94, 0.96, 1.0];
const STATUS_ACTIVE_BG: [f32; 4] = [0.14, 0.30, 0.42, 1.0];
const STATUS_INACTIVE_BG: [f32; 4] = [0.12, 0.13, 0.15, 1.0];
const STATUS_DEAD_BG: [f32; 4] = [0.40, 0.12, 0.12, 1.0];
const DIVIDER_BG: [f32; 4] = [0.22, 0.23, 0.26, 1.0];

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
    pub hooks: Hooks,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSpec {
    pub title: String,
    pub command: CommandSpec,
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
/// `lifecycle_sink` is invoked for session start and child-exit events. PTY
/// errors currently mark panes dead but are not reported as lifecycle events.
/// `route_sink` is reserved for the future generic routing primitive and is not
/// invoked yet.
#[derive(Clone, Default)]
pub struct Hooks {
    pub route_sink: Option<Arc<dyn RouteSink>>,
    pub lifecycle_sink: Option<Arc<dyn LifecycleSink>>,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks")
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
        self.sessions == other.sessions && self.initial_layout == other.initial_layout
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
            hooks: Hooks::default(),
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
            hooks: Hooks::default(),
        }
    }

    pub fn with_hooks(mut self, hooks: Hooks) -> Self {
        self.hooks = hooks;
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
    NewTab,
    ClosePaneOrTab,
    CloseTab,
    Quit,
    Paste,
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
    if lower == Some("v") || physical_code == Some(KeyCode::KeyV) {
        return Some(CmdShortcut::Paste);
    }
    None
}

fn compute_tab_viewports(
    root: &Layout,
    size: PhysicalSize<u32>,
    cell: crate::render::CellMetrics,
) -> HashMap<PaneId, PaneViewport> {
    let content_size = tab_content_size(size, cell);
    let mut layouts = layout::compute_viewports(root, content_size, cell);
    for viewport in layouts.values_mut() {
        viewport.row_offset += TAB_BAR_ROWS;
        viewport.status_row = viewport.status_row.map(|row| row + TAB_BAR_ROWS);
        viewport.y_px = viewport
            .y_px
            .saturating_add(cell.height * TAB_BAR_ROWS as u32);
    }
    layouts
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
    /// M17-5: resize coalesce. winit Resized burst를 about_to_wait에서 마지막 size로만 처리.
    /// 매번 reflow + PTY size 갱신하면 zsh가 따라잡지 못해 redraw 시퀀스가 잘못된 size에 적용 → tearing.
    pending_resize: Option<PhysicalSize<u32>>,
    /// M12-6: design §2.1의 monotonic ID 정책을 IdAllocator로 캡슐화. M15 dynamic spawn에서 활용.
    #[allow(dead_code)]
    ids: IdAllocator,
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
        // M6-3 Phase 0: IME 이벤트 활성화.
        state.window.set_ime_allowed(true);
        state.window.set_ime_purpose(ImePurpose::Terminal);
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
        let attrs = Window::default_attributes()
            .with_title("pj001")
            .with_inner_size(PhysicalSize::new(960u32, 600u32))
            .with_min_inner_size(PhysicalSize::new(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
        let window = Arc::new(event_loop.create_window(attrs).expect("create_window"));
        window.set_min_inner_size(Some(PhysicalSize::new(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)));
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
            WindowEvent::Occluded(false) => {
                state.window.request_redraw();
            }
            WindowEvent::Occluded(true) => {}
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::CursorMoved { position, .. } => {
                state.last_mouse_pos = Some(position);
                if state.dragging_divider.is_some() {
                    state.drag_divider_to_mouse();
                }
                state.update_mouse_cursor();
            }
            WindowEvent::CursorLeft { .. } => {
                state.last_mouse_pos = None;
                state.dragging_divider = None;
                state.window.set_cursor(CursorIcon::Default);
            }
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Left,
                ..
            } => match button_state {
                ElementState::Pressed => {
                    if let Some(tab_id) = state.tab_at_mouse() {
                        state.set_active_tab(tab_id);
                        return;
                    }
                    if let Some(hit) = state.divider_hit_at_mouse() {
                        state.dragging_divider = Some(hit);
                        state.update_mouse_cursor();
                        return;
                    }
                    if let Some(pane_id) = state.pane_at_mouse(true) {
                        state.set_active(pane_id);
                    }
                }
                ElementState::Released => {
                    state.dragging_divider = None;
                    state.update_mouse_cursor();
                }
            },
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
                        Some(CmdShortcut::Paste) => {
                            state.handle_paste();
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
                if event.state == ElementState::Pressed {
                    let idx = state.active_index();
                    if let Ok(mut term) = state.session_for_pane_idx(idx).term.lock() {
                        if term.view_offset() > 0 {
                            term.snap_to_bottom();
                            state.window.request_redraw();
                        }
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
        // 깜빡임 정지 조건:
        // - 창 비활성 (focused=false)
        // - cursor.blinking=false (DECSCUSR steady)
        // 정지 시 cursor_visible=true 유지 (계속 보임), Wait로 idle.
        if !state.focused || !state.cursor_blinking_cache {
            if !state.cursor_visible {
                state.cursor_visible = true;
                state.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let blink = Duration::from_millis(CURSOR_BLINK_MS);
        let now = Instant::now();
        let next = if now.duration_since(state.last_blink) >= blink {
            state.cursor_visible = !state.cursor_visible;
            state.last_blink = now;
            state.window.request_redraw();
            now + blink
        } else {
            state.last_blink + blink
        };
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
        let layouts = compute_tab_viewports(&root, size, self.renderer.cell_metrics());
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
        let new_pane = self.ids.new_pane();
        let active = self.active_tab().active;
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
        let layouts =
            compute_tab_viewports(&self.active_tab().root, size, self.renderer.cell_metrics());
        let Some(viewport) = layouts.get(&new_pane).copied() else {
            log::warn!("split produced no viewport for new pane {}", new_pane.0);
            return Ok(());
        };
        self.spawn_session_for_pane(
            new_pane,
            SessionSpec {
                title: "shell".to_string(),
                command: CommandSpec::Shell,
            },
            viewport,
        )?;
        self.apply_layout_viewports_for_size(size);
        self.set_active(new_pane);
        Ok(())
    }

    fn create_tab(&mut self) -> Result<()> {
        let tab_id = self.ids.new_tab();
        let pane_id = self.ids.new_pane();
        let root = Layout::Pane(pane_id);
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let layouts = compute_tab_viewports(&root, size, self.renderer.cell_metrics());
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

    fn target_inner_size_for_layout(&self) -> PhysicalSize<u32> {
        let min = self.minimum_inner_size();
        PhysicalSize::new(
            self.surface_config.width.max(min.width),
            self.surface_config.height.max(min.height),
        )
    }

    fn update_min_inner_size(&self) {
        self.window
            .set_min_inner_size(Some(self.minimum_inner_size()));
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
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let renderer = Renderer::new(
            &device,
            &queue,
            format,
            [size.width as f32, size.height as f32],
            FONT_SIZE,
        );

        let hooks = config.hooks.clone();
        let pane_specs = config.pane_specs()?;
        let mut ids = IdAllocator::default();
        let pane_ids = (0..pane_specs.len())
            .map(|_| ids.new_pane())
            .collect::<Vec<_>>();
        let layout_root = Layout::from_initial_panes(&pane_ids);
        let layouts = compute_tab_viewports(&layout_root, size, renderer.cell_metrics());
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
            pending_resize: None,
            ids,
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
        let tab_width = (total_cols / self.tabs.len()).max(1);
        let idx = (col / tab_width).min(self.tabs.len() - 1);
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
        let hit = self
            .dragging_divider
            .as_ref()
            .cloned()
            .or_else(|| self.divider_hit_at_mouse());
        let icon = match hit.map(|hit| hit.axis()) {
            Some(SplitAxis::Vertical) => CursorIcon::ColResize,
            Some(SplitAxis::Horizontal) => CursorIcon::RowResize,
            None => CursorIcon::Default,
        };
        self.window.set_cursor(icon);
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

    /// M12-6 design §5: 마우스 hit-test로 SessionId 반환. pane_at_mouse → SessionId 매핑.
    #[allow(dead_code)]
    fn session_at_mouse(&self, include_status_row: bool) -> Option<SessionId> {
        self.pane_index_at_mouse(include_status_row)
            .map(|idx| self.active_tab().panes[idx].session)
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
            "paste: {} bytes, bracketed={}, lines={}",
            text.len(),
            bracketed,
            text.matches('\n').count() + 1
        );
        let session = self.session_for_pane_idx_mut(idx);
        if bracketed {
            let _ = session.pty.write(b"\x1b[200~");
            let _ = session.pty.write(text.as_bytes());
            let _ = session.pty.write(b"\x1b[201~");
        } else {
            let _ = session.pty.write(text.as_bytes());
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);
        self.renderer
            .resize(&self.queue, [size.width as f32, size.height as f32]);
        let root = self.active_tab().root.clone();
        let layouts = compute_tab_viewports(&root, size, self.renderer.cell_metrics());
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
            let viewport = layouts[&pane_id];
            let prev = self.active_tab().panes[idx].viewport;
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
        self.window.request_redraw();
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
            let (pane_id, pane_col_offset, pane_row_offset, session_id) = {
                let p = &self.active_tab().panes[idx];
                (
                    p.id,
                    p.viewport.col_offset,
                    p.viewport.row_offset,
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
                self.renderer.append_term(
                    &self.queue,
                    &term,
                    preedit_for_render,
                    cursor_render,
                    pane_col_offset,
                    pane_row_offset,
                );
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
    fn tab_text_fits_available_columns() {
        assert_eq!(tab_text("1", 3), " 1 ");
        assert_eq!(tab_text("10", 3), " 1");
        assert_eq!(tab_text("10", 1), "");
    }

    #[test]
    fn tab_label_prefixes_one_based_index() {
        assert_eq!(tab_label(0, "shell"), "1 shell");
        assert_eq!(tab_label(2, "Codex"), "3 Codex");
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
            hooks: Hooks::default(),
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
            hooks: Hooks::default(),
        };

        assert!(config.pane_specs().is_err());
    }
}
