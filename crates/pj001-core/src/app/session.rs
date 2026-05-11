use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::app::event::SessionId;
use crate::grid::Term;
use crate::pty::PtyHandle;

/// M12-3: PTY 프로세스 단위. M11 Pane이 보유하던 pty/term/title/alive를 이관.
/// Pane은 viewport(시각 슬롯)만 보유. 1 Session = 0..1 Pane (M14 tab 전까지).
pub struct Session {
    pub id: SessionId,
    pub title: String,
    /// spawn 시 사용한 원본 명령 (CommandSpec 해석 후 — title fallback, 디버깅, 추후 respawn에 사용).
    #[allow(dead_code)]
    pub command: String,
    pub pty: PtyHandle,
    pub term: Arc<Mutex<Term>>,
    pub alive: bool,
    pub exit_code: Option<i32>,
    #[allow(dead_code)]
    pub created_at: Instant,
}
