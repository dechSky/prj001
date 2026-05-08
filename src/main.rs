mod app;
mod error;
mod grid;
mod pty;
mod render;
mod vt;

fn main() -> error::Result<()> {
    env_logger::init();
    app::run()
}
