pub mod event;
mod input;

use std::sync::{Arc, Mutex};

use portable_pty::PtySize;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::error::{Error, Result};
use crate::grid::Term;
use crate::pty::PtyHandle;
use crate::render::Renderer;
use event::UserEvent;

const FONT_SIZE: f32 = 14.0;

pub fn run() -> Result<()> {
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
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    state: Option<AppState>,
    proxy: EventLoopProxy<UserEvent>,
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
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self { state: None, proxy }
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
        let state = pollster::block_on(AppState::new(window, self.proxy.clone()))
            .expect("AppState::new");
        state.window.focus_window();
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
    async fn new(window: Arc<Window>, proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
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
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
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
            self.renderer.update_term(&self.device, &self.queue, &term);
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
