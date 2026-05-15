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

    /// M-W-6.1: surface capabilities가 일부 비어 있어 configure 불가.
    /// 첫 윈도우(request_adapter 직후)도, 두 번째 이후(공유 adapter)도 사용 — 양쪽 path 공통.
    /// `Surface::configure` panic 회피 — caller가 caps empty check 후 이 variant로 변환.
    /// Codex M-W-6.1 1차 개선: alpha_modes_empty 필드 추가 (alpha caps empty 진단성).
    #[error(
        "surface capabilities incompatible: formats_empty={formats_empty} alpha_modes_empty={alpha_modes_empty} present_modes_empty={present_modes_empty}"
    )]
    SurfaceIncompatible {
        formats_empty: bool,
        alpha_modes_empty: bool,
        present_modes_empty: bool,
    },

    #[error("request device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),

    #[error("pty: {0}")]
    Pty(#[from] anyhow::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("argument error: {0}")]
    Args(String),
}

pub type Result<T> = std::result::Result<T, Error>;
