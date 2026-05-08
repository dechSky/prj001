use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("event loop error: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),

    #[error("os error: {0}")]
    Os(#[from] winit::error::OsError),

    #[error("create surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),

    #[error("no compatible GPU adapter")]
    NoAdapter,

    #[error("request device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),

    #[error("pty: {0}")]
    Pty(#[from] anyhow::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
