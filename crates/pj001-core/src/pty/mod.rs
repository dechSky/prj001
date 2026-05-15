mod reader;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use winit::{event_loop::EventLoopProxy, window::WindowId};

use crate::app::event::{SessionId, UserEvent};
use crate::error::Result;
use crate::grid::Term;

fn shell_command(shell: &str) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(shell);
    // M8-7 보강: TERM_PROGRAM=Apple_Terminal 위장으로 macOS zsh의
    // /etc/zshrc_Apple_Terminal 이 활성화되어 OSC 7(cwd) 자동 송신.
    // SHELL_SESSIONS_DISABLE은 Apple session restore 배너와 ~/.zsh_sessions
    // 기록만 끈다. OSC 7 precmd hook은 이 플래그와 별도로 유지된다.
    // TERM도 명시 (xterm-256color — 표준 ANSI/256-color 인식).
    cmd.env("TERM", "xterm-256color");
    cmd.env("TERM_PROGRAM", "Apple_Terminal");
    cmd.env("SHELL_SESSIONS_DISABLE", "1");
    cmd
}

fn shell_command_with_cwd(shell: &str, cwd: Option<&str>) -> CommandBuilder {
    let mut cmd = shell_command(shell);
    if let Some(cwd) = cwd.filter(|cwd| std::path::Path::new(cwd).is_dir()) {
        cmd.cwd(cwd);
    }
    cmd
}

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
        window_id: WindowId,
        session: SessionId,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size)?;
        let cmd = shell_command_with_cwd(shell, cwd);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;
        let reader_thread = reader::spawn(reader, term, proxy, window_id, session);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_command_sets_terminal_identity_and_disables_apple_restore() {
        let cmd = shell_command("/bin/zsh");

        assert_eq!(cmd.get_argv()[0], "/bin/zsh");
        assert_eq!(cmd.get_env("TERM").unwrap(), "xterm-256color");
        assert_eq!(cmd.get_env("TERM_PROGRAM").unwrap(), "Apple_Terminal");
        assert_eq!(cmd.get_env("SHELL_SESSIONS_DISABLE").unwrap(), "1");
    }

    #[test]
    fn shell_command_with_cwd_sets_existing_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let cmd = shell_command_with_cwd("/bin/zsh", cwd.to_str());

        assert_eq!(cmd.get_cwd().map(|s| s.as_os_str()), Some(cwd.as_os_str()));
    }

    #[test]
    fn shell_command_with_cwd_ignores_missing_cwd() {
        let cmd = shell_command_with_cwd("/bin/zsh", Some("/definitely/missing/pj001/cwd"));

        assert!(cmd.get_cwd().is_none());
    }
}
