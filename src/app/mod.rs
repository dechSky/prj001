pub mod event;
mod input;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{ImePurpose, Window, WindowId};

use crate::error::{Error, Result};
use crate::grid::Term;
use crate::pty::PtyHandle;
use crate::render::Renderer;
use event::UserEvent;

const FONT_SIZE: f32 = 14.0;
const CURSOR_BLINK_MS: u64 = 500;

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
            WindowEvent::Resized(size) => state.resize(size),
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::Ime(ime) => {
                log::info!("ime: {:?}", ime);
                use winit::event::Ime;
                match ime {
                    Ime::Preedit(s, _range) => {
                        state.preedit = if s.is_empty() { None } else { Some(s) };
                        state.window.request_redraw();
                    }
                    Ime::Commit(s) => {
                        state.preedit = None;
                        if let Err(e) = state.pty.write(s.as_bytes()) {
                            log::warn!("pty write (ime commit): {e}");
                        }
                        state.window.request_redraw();
                    }
                    Ime::Disabled => {
                        if state.preedit.is_some() {
                            state.preedit = None;
                            state.window.request_redraw();
                        }
                    }
                    Ime::Enabled => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                log::info!(
                    "key: state={:?} logical={:?} text={:?}",
                    event.state,
                    event.logical_key,
                    event.text
                );
                if let Some(bytes) = input::encode_key(&event) {
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
        })
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
        use wgpu::CurrentSurfaceTexture as C;
        let frame = match self.surface.get_current_texture() {
            C::Success(t) | C::Suboptimal(t) => t,
            C::Outdated | C::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            C::Timeout | C::Occluded | C::Validation => return,
        };

        if let Ok(term) = self.term.lock() {
            let cur = term.cursor();
            let preedit_arg = self
                .preedit
                .as_deref()
                .map(|s| (s, cur.col, cur.row));
            // cursor 위치 = preedit 있으면 preedit 끝, 없으면 term.cursor().
            // 깜빡임 OFF 단계면 None.
            let cursor_xy = if self.cursor_visible {
                let (row, col) = if let Some((preedit_str, col, row)) = preedit_arg {
                    let mut c = col;
                    for ch in preedit_str.chars() {
                        c += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    }
                    (row, c.min(term.cols().saturating_sub(1)))
                } else {
                    (cur.row, cur.col)
                };
                Some((row, col))
            } else {
                None
            };
            self.renderer.update_term(
                &self.device,
                &self.queue,
                &term,
                preedit_arg,
                cursor_xy,
            );
            // M6-3b: cursor 위치가 바뀌면 IME composition window 위치 갱신.
            if self.last_ime_cursor != Some((cur.row, cur.col)) {
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
