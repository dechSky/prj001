mod reader;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use winit::event_loop::EventLoopProxy;

use crate::app::event::UserEvent;
use crate::error::Result;
use crate::grid::Term;

pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader_thread: Option<JoinHandle<()>>,
}

impl PtyHandle {
    pub fn spawn(
        shell: &str,
        size: PtySize,
        term: Arc<Mutex<Term>>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size)?;
        let cmd = CommandBuilder::new(shell);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;
        let reader_thread = reader::spawn(reader, term, proxy);

        Ok(Self {
            master: pair.master,
            writer,
            child,
            reader_thread: Some(reader_thread),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    #[allow(dead_code)] // M5에서 WindowEvent::Resized 와이어업
    pub fn resize(&self, size: PtySize) -> Result<()> {
        self.master.resize(size)?;
        Ok(())
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        // 운영 정책 G 보정: stdlib Child::drop은 detach라 명시적 kill+wait 필요
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = self.reader_thread.take() {
            let _ = t.join();
        }
    }
}
