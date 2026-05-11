pub mod event;
mod input;
mod layout;
mod session;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{ImePurpose, Window, WindowId};

use crate::error::{Error, Result};
use crate::grid::Term;
use crate::pty::PtyHandle;
use crate::render::{CursorRender, Renderer};
use event::{IdAllocator, PaneId, SessionId, UserEvent};
#[cfg(test)]
use layout::SplitRatio;
use layout::{Layout, RatioDirection, SplitAxis};
use session::Session;

const FONT_SIZE: f32 = 14.0;
const CURSOR_BLINK_MS: u64 = 500;
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

struct AppState {
    window: Arc<Window>,
    proxy: EventLoopProxy<UserEvent>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// M12-3: PTY 프로세스 보관. Pane은 SessionId로 참조.
    sessions: HashMap<SessionId, Session>,
    panes: Vec<Pane>,
    /// M13-2: active tab 도입 전 단일 BSP root. M13 dynamic split/close가 이 tree를 mutate.
    layout_root: Layout,
    active: PaneId,
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
        self.state = Some(state);
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
            .with_inner_size(PhysicalSize::new(960u32, 600u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("create_window"));
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
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::CursorMoved { position, .. } => {
                state.last_mouse_pos = Some(position);
            }
            WindowEvent::CursorLeft { .. } => {
                state.last_mouse_pos = None;
            }
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Left,
                ..
            } => {
                if button_state == ElementState::Pressed {
                    if let Some(pane_id) = state.pane_at_mouse(true) {
                        state.set_active(pane_id);
                    }
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
                use winit::keyboard::{Key, NamedKey};
                // M8-6: macOS Cmd 단축키 처리 — Cmd+Q/W = 종료, Cmd+V = paste, 그 외 swallow.
                if event.state == ElementState::Pressed && state.modifiers.super_key() {
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
                    if let Key::Character(s) = &event.logical_key {
                        let lower = s.to_lowercase();
                        if let Some(ordinal) = lower
                            .chars()
                            .next()
                            .and_then(|ch| ch.to_digit(10))
                            .filter(|digit| (1..=9).contains(digit))
                        {
                            state.focus_pane_by_ordinal(ordinal as usize);
                            return;
                        }
                        if lower == "[" {
                            state.focus_adjacent_pane(false);
                            return;
                        }
                        if lower == "]" {
                            state.focus_adjacent_pane(true);
                            return;
                        }
                        if lower == "d" {
                            let axis = if state.modifiers.shift_key() {
                                SplitAxis::Horizontal
                            } else {
                                SplitAxis::Vertical
                            };
                            if let Err(e) = state.split_active(axis) {
                                log::warn!("cmd+d split failed: {e}");
                            }
                            return;
                        }
                        if lower == "w" {
                            if state.panes.len() <= 1 {
                                log::info!("cmd+w: exit last pane");
                                event_loop.exit();
                            } else {
                                state.close_active_pane();
                            }
                            return;
                        }
                        if lower == "q" {
                            log::info!("cmd+{lower}: exit");
                            event_loop.exit();
                            return;
                        }
                        if lower == "v" {
                            // M10-6: clipboard paste. bracketed_paste mode면 \e[200~/\e[201~ 래핑.
                            state.handle_paste();
                            return;
                        }
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
                    if state.panes.iter().any(|p| p.session == session_id) {
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
                    if state.panes.len() <= 1 {
                        event_loop.exit();
                    } else {
                        let active_session = state
                            .panes
                            .iter()
                            .find(|p| p.id == state.active)
                            .map(|p| p.session);
                        if state.all_sessions_dead() {
                            event_loop.exit();
                        } else if active_session == Some(id) {
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
                    if state.panes.len() <= 1 {
                        event_loop.exit();
                    } else {
                        let active_session = state
                            .panes
                            .iter()
                            .find(|p| p.id == state.active)
                            .map(|p| p.session);
                        if state.all_sessions_dead() {
                            event_loop.exit();
                        } else if active_session == Some(id) {
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
    fn active_index(&self) -> usize {
        self.panes
            .iter()
            .position(|p| p.id == self.active)
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
        self.panes.iter().position(|pane| {
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
        self.panes
            .iter()
            .all(|p| !sessions.get(&p.session).map(|s| s.alive).unwrap_or(false))
    }

    /// M12-3: 다음 alive pane을 찾음.
    fn next_alive_pane_id(&self) -> Option<PaneId> {
        let sessions = &self.sessions;
        self.panes
            .iter()
            .find(|p| sessions.get(&p.session).map(|s| s.alive).unwrap_or(false))
            .map(|p| p.id)
    }

    /// M12-3: PaneId → SessionId 매핑. M12-4 재작성에서 inline으로 옮겨갔지만 M13 BSP layout
    /// traversal에서 PaneId 기반 lookup에 다시 활용 예정.
    #[allow(dead_code)]
    fn session_id_for_pane(&self, pane: PaneId) -> Option<SessionId> {
        self.panes.iter().find(|p| p.id == pane).map(|p| p.session)
    }

    fn set_active(&mut self, id: PaneId) {
        let sessions = &self.sessions;
        let Some(new_idx) = self
            .panes
            .iter()
            .position(|p| p.id == id && sessions.get(&p.session).map(|s| s.alive).unwrap_or(false))
        else {
            return;
        };
        if self.active == id {
            return;
        }

        let old_idx = self.active_index();
        if self.focused {
            self.send_focus_report(old_idx, false);
            self.send_focus_report(new_idx, true);
        }

        let title = self.session_for_pane_idx(new_idx).title.clone();
        self.active = id;
        self.window.set_title(&format!("{} — pj001", title));
        self.cursor_visible = true;
        self.last_ime_cursor = None;
        self.preedit = None;
        self.window.request_redraw();
    }

    fn send_focus_report(&mut self, idx: usize, focused: bool) {
        if idx >= self.panes.len() {
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
        self.panes.push(Pane {
            id: pane_id,
            session: session_id,
            viewport,
        });
        self.emit_lifecycle(LifecycleEvent::SessionStarted { session_id, title });
        Ok(())
    }

    fn apply_layout_viewports(&mut self) {
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let layouts =
            layout::compute_viewports(&self.layout_root, size, self.renderer.cell_metrics());
        for idx in 0..self.panes.len() {
            let pane_id = self.panes[idx].id;
            let Some(viewport) = layouts.get(&pane_id).copied() else {
                log::warn!("layout missing viewport for pane {}", pane_id.0);
                continue;
            };
            let prev = self.panes[idx].viewport;
            let pty_cell_size_changed = prev.cols != viewport.cols || prev.rows != viewport.rows;
            self.panes[idx].viewport = viewport;
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
        if !self.layout_root.split_pane(self.active, axis, new_pane) {
            log::warn!("split requested for missing active pane {}", self.active.0);
            return Ok(());
        }
        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        let layouts =
            layout::compute_viewports(&self.layout_root, size, self.renderer.cell_metrics());
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
        self.apply_layout_viewports();
        self.set_active(new_pane);
        Ok(())
    }

    fn close_active_pane(&mut self) {
        if self.panes.len() <= 1 {
            return;
        }
        let closing = self.active;
        if !self.layout_root.close_pane(closing) {
            log::warn!("close requested for missing active pane {}", closing.0);
            return;
        }
        let Some(idx) = self.panes.iter().position(|p| p.id == closing) else {
            log::warn!("close requested for missing pane {}", closing.0);
            return;
        };
        let closing_session = self.panes[idx].session;
        self.panes.remove(idx);
        self.sessions.remove(&closing_session);
        self.apply_layout_viewports();
        if let Some(next) = self.next_alive_pane_id() {
            self.active = next;
            self.cursor_visible = true;
            self.last_ime_cursor = None;
            self.preedit = None;
            if let Some(next_idx) = self.panes.iter().position(|p| p.id == next) {
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
        let order = self.layout_root.pane_order();
        if order.len() <= 1 {
            return;
        }
        let Some(current_idx) = order.iter().position(|id| *id == self.active) else {
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
        let order = self.layout_root.pane_order();
        let Some(candidate) = order.get(ordinal - 1).copied() else {
            return;
        };
        self.set_active(candidate);
    }

    fn adjust_active_split(&mut self, axis: SplitAxis, direction: RatioDirection) -> bool {
        if self
            .layout_root
            .adjust_split_for_pane(self.active, axis, direction)
        {
            self.apply_layout_viewports();
            self.window.request_redraw();
            true
        } else {
            false
        }
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
        let layouts = layout::compute_viewports(&layout_root, size, renderer.cell_metrics());
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

        Ok(Self {
            window,
            proxy,
            surface,
            surface_config,
            device,
            queue,
            sessions,
            panes,
            layout_root,
            active: PaneId::first(),
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
            pending_resize: None,
            ids,
        })
    }

    /// M12-3: pane index → Session lookup. borrow scope 짧게 유지하기 위해 SessionId만 복사.
    fn session_id_for_pane_idx(&self, idx: usize) -> SessionId {
        self.panes[idx].session
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
            .map(|idx| self.panes[idx].id)
    }

    /// M12-6 design §5: 마우스 hit-test로 SessionId 반환. pane_at_mouse → SessionId 매핑.
    #[allow(dead_code)]
    fn session_at_mouse(&self, include_status_row: bool) -> Option<SessionId> {
        self.pane_index_at_mouse(include_status_row)
            .map(|idx| self.panes[idx].session)
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
        let layouts =
            layout::compute_viewports(&self.layout_root, size, self.renderer.cell_metrics());
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
        for idx in 0..self.panes.len() {
            let pane_id = self.panes[idx].id;
            let viewport = layouts[&pane_id];
            let prev = self.panes[idx].viewport;
            // M12-5 회귀 fix (Codex threads 019e164b → 019e1653 가설 K):
            // PTY/Term resize는 실제 terminal cell size(rows/cols)가 바뀔 때만 필요하다.
            // col_offset/status_row/pixel 크기는 visual layout/chrome 값이라 이걸 trigger로
            // pty.resize를 보내면 rows/cols가 같아도 SIGWINCH가 발생할 수 있다.
            // split startup 직후 col_offset/status_row 차이가 zsh duplicate prompt 유발.
            let pty_cell_size_changed = prev.cols != viewport.cols || prev.rows != viewport.rows;
            // viewport visual/pixel 값은 surface size 변경 시 달라질 수 있으므로 항상 갱신.
            // M12-5 mutation 단일 진입점 — 전체 struct 교체만 허용.
            self.panes[idx].viewport = viewport;
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
        for idx in 0..self.panes.len() {
            let pane_id = self.panes[idx].id;
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
                return;
            }
            C::Timeout | C::Occluded | C::Validation => return,
        };

        self.renderer.begin_terms();
        let active = self.active;
        let pane_count = self.panes.len();
        for idx in 0..pane_count {
            let (pane_id, pane_col_offset, pane_row_offset, session_id) = {
                let p = &self.panes[idx];
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

    fn append_split_chrome(&mut self) {
        if self.panes.len() < 2 {
            return;
        }

        let size = PhysicalSize::new(self.surface_config.width, self.surface_config.height);
        for divider in
            layout::vertical_dividers(&self.layout_root, size, self.renderer.cell_metrics())
        {
            self.renderer
                .append_fill_column(divider.col, divider.row, divider.height, DIVIDER_BG);
        }
        for divider in
            layout::horizontal_dividers(&self.layout_root, size, self.renderer.cell_metrics())
        {
            self.renderer
                .append_fill_row(divider.col, divider.row, divider.width, DIVIDER_BG);
        }

        let mut status_items = Vec::new();
        for pane in &self.panes {
            let Some(status_row) = pane.viewport.status_row else {
                continue;
            };
            let session = &self.sessions[&pane.session];
            let alive = session.alive;
            let state = if !alive {
                "DEAD"
            } else if pane.id == self.active {
                "ACTIVE"
            } else {
                "READY"
            };
            let bg = if !alive {
                STATUS_DEAD_BG
            } else if pane.id == self.active {
                STATUS_ACTIVE_BG
            } else {
                STATUS_INACTIVE_BG
            };
            let text = format!(" {state} {} ", session.title);
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
