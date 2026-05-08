pub mod event;
mod input;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{MouseScrollDelta, WindowEvent};
use winit::keyboard::ModifiersState;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{ImePurpose, Window, WindowId};

use crate::error::{Error, Result};
use crate::grid::Term;
use crate::pty::PtyHandle;
use crate::render::{CursorRender, Renderer};
use event::UserEvent;

const FONT_SIZE: f32 = 14.0;
const CURSOR_BLINK_MS: u64 = 500;

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

pub fn run(shell_override: Option<String>) -> Result<()> {
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
    let mut app = App::new(proxy, shell_override);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    state: Option<AppState>,
    proxy: EventLoopProxy<UserEvent>,
    shell_override: Option<String>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pty: PtyHandle,
    term: Arc<Mutex<Term>>,
    renderer: Renderer,
    last_ime_cursor: Option<(usize, usize)>,
    preedit: Option<String>,
    cursor_visible: bool,
    last_blink: Instant,
    focused: bool,
    cursor_blinking_cache: bool,
    modifiers: ModifiersState,
    /// M17-5: resize coalesce. winit Resized burst를 about_to_wait에서 마지막 size로만 처리.
    /// 매번 reflow + PTY size 갱신하면 zsh가 따라잡지 못해 redraw 시퀀스가 잘못된 size에 적용 → tearing.
    pending_resize: Option<PhysicalSize<u32>>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, shell_override: Option<String>) -> Self {
        Self {
            state: None,
            proxy,
            shell_override,
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("pj001")
            .with_inner_size(PhysicalSize::new(960u32, 600u32));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("create_window"),
        );
        let state = pollster::block_on(AppState::new(
            window,
            self.proxy.clone(),
            self.shell_override.clone(),
        ))
        .expect("AppState::new");
        state.window.focus_window();
        // M6-3 Phase 0: IME 이벤트 활성화. Terminal purpose로 IME에 컨텍스트 hint.
        state.window.set_ime_allowed(true);
        state.window.set_ime_purpose(ImePurpose::Terminal);
        self.state = Some(state);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else { return };
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
                let send: Option<&[u8]> = {
                    let term = state.term.lock().unwrap();
                    if term.focus_reporting() {
                        Some(if focused { b"\x1b[I" } else { b"\x1b[O" })
                    } else {
                        None
                    }
                };
                if let Some(bytes) = send {
                    if let Err(e) = state.pty.write(bytes) {
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
                    if let Ok(mut term) = state.term.lock() {
                        term.scroll_view_by(lines);
                    }
                    state.window.request_redraw();
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
                        if let Err(e) = state.pty.write(s.as_bytes()) {
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
                use winit::event::ElementState;
                use winit::keyboard::{Key, NamedKey};
                // M8-6: macOS Cmd 단축키 처리 — Cmd+Q/W = 종료, Cmd+V = paste, 그 외 swallow.
                if event.state == ElementState::Pressed && state.modifiers.super_key() {
                    if let Key::Character(s) = &event.logical_key {
                        let lower = s.to_lowercase();
                        if lower == "q" || lower == "w" {
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
                        let alt = state
                            .term
                            .lock()
                            .map(|t| t.is_alt_screen())
                            .unwrap_or(false);
                        if alt {
                            // alt screen: encode_named_key가 byte 반환 → 일반 PTY 송신 흐름.
                            log_page_dispatch_once("PTY (alt screen)");
                        } else {
                            // main screen: scrollback view 스크롤. PTY 안 보냄.
                            if let Ok(mut term) = state.term.lock() {
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
                    if let Ok(mut term) = state.term.lock() {
                        if term.view_offset() > 0 {
                            term.snap_to_bottom();
                            state.window.request_redraw();
                        }
                    }
                }
                // single lock snapshot (advisor 가이드).
                let mode = {
                    let term = state.term.lock().unwrap();
                    input::InputMode {
                        cursor_keys_application: term.cursor_keys_application(),
                        alt_screen: term.is_alt_screen(),
                        modifiers: state.modifiers,
                    }
                };
                if let Some(bytes) = input::encode_key(&event, mode) {
                    if let Err(e) = state.pty.write(&bytes) {
                        log::warn!("pty write: {e}");
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else { return };
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
            UserEvent::Repaint => {
                if let Some(state) = &self.state {
                    state.window.request_redraw();
                }
            }
            UserEvent::ChildExited(code) => {
                log::info!("child exited (code={code})");
                event_loop.exit();
            }
            UserEvent::PtyError(msg) => {
                log::error!("pty error: {msg}");
                event_loop.exit();
            }
        }
    }
}

impl AppState {
    async fn new(
        window: Arc<Window>,
        proxy: EventLoopProxy<UserEvent>,
        shell_override: Option<String>,
    ) -> Result<Self> {
        let size = window.inner_size();
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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

        let cell = renderer.cell_metrics();
        let cols = (size.width / cell.width).max(1) as usize;
        let rows = (size.height / cell.height).max(1) as usize;

        let term = Arc::new(Mutex::new(Term::new(cols, rows)));
        let shell = shell_override
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/zsh".to_string());
        log::info!("shell: {shell}");
        let pty = PtyHandle::spawn(
            &shell,
            PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: size.width as u16,
                pixel_height: size.height as u16,
            },
            term.clone(),
            proxy,
        )?;

        Ok(Self {
            window,
            surface,
            surface_config,
            device,
            queue,
            pty,
            term,
            renderer,
            last_ime_cursor: None,
            preedit: None,
            cursor_visible: true,
            last_blink: Instant::now(),
            focused: true,
            cursor_blinking_cache: true,
            modifiers: ModifiersState::empty(),
            pending_resize: None,
        })
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
        // scrollback view 활성 시 paste는 bottom으로 snap.
        if let Ok(mut term) = self.term.lock() {
            if term.view_offset() > 0 {
                term.snap_to_bottom();
                self.window.request_redraw();
            }
        }
        let bracketed = self.term.lock().map(|t| t.bracketed_paste()).unwrap_or(false);
        log::debug!(
            "paste: {} bytes, bracketed={}, lines={}",
            text.len(),
            bracketed,
            text.matches('\n').count() + 1
        );
        if bracketed {
            let _ = self.pty.write(b"\x1b[200~");
            let _ = self.pty.write(text.as_bytes());
            let _ = self.pty.write(b"\x1b[201~");
        } else {
            let _ = self.pty.write(text.as_bytes());
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
        let cell = self.renderer.cell_metrics();
        let cols = (size.width / cell.width).max(1) as usize;
        let rows = (size.height / cell.height).max(1) as usize;
        if let Ok(mut term) = self.term.lock() {
            term.resize(cols, rows);
        }
        let _ = self.pty.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: size.width as u16,
            pixel_height: size.height as u16,
        });
        self.window.request_redraw();
    }

    fn render(&mut self) {
        // M10-1: vt가 누적한 응답(DSR/DA 등)을 PTY로 송신. lock 잡고 drain → drop → write.
        let responses: Vec<Vec<u8>> = if let Ok(mut term) = self.term.lock() {
            term.drain_responses()
        } else {
            Vec::new()
        };
        for resp in responses {
            if let Err(e) = self.pty.write(&resp) {
                log::warn!("pty response write: {e}");
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

        if let Ok(mut term) = self.term.lock() {
            let cur = term.cursor();
            // M7-3: cursor.blinking 캐시 갱신. about_to_wait이 매 tick lock 안 잡도록.
            self.cursor_blinking_cache = cur.blinking;
            // M8-7: title 변경 있으면 winit window에 반영.
            if let Some(t) = term.take_title_if_changed() {
                self.window.set_title(&t);
            }
            let in_scrollback = term.view_offset() > 0;
            let preedit_arg = self
                .preedit
                .as_deref()
                .map(|s| (s, cur.col, cur.row));
            // cursor 위치 = preedit 있으면 preedit 끝, 없으면 term.cursor().
            // 가시성 판정: DECTCEM(cur.visible) + scrollback view + blink phase(cursor_visible).
            let cursor_render = if cur.visible && self.cursor_visible && !in_scrollback {
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
            // scrollback view 중이면 preedit overlay도 stale 위치라 안 그림.
            let preedit_for_render = if in_scrollback { None } else { preedit_arg };
            self.renderer.update_term(
                &self.device,
                &self.queue,
                &term,
                preedit_for_render,
                cursor_render,
            );
            // M6-3b: cursor 위치가 바뀌면 IME composition window 위치 갱신.
            // scrollback view 중이면 stale 위치라 skip.
            if !in_scrollback && self.last_ime_cursor != Some((cur.row, cur.col)) {
                let cell = self.renderer.cell_metrics();
                let pos = winit::dpi::PhysicalPosition::<f64>::new(
                    (cur.col as u32 * cell.width) as f64,
                    (cur.row as u32 * cell.height) as f64,
                );
                let size = winit::dpi::PhysicalSize::<u32>::new(cell.width, cell.height);
                self.window.set_ime_cursor_area(pos, size);
                self.last_ime_cursor = Some((cur.row, cur.col));
            }
        }

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
}
