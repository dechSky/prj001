#[derive(Debug)]
pub enum UserEvent {
    /// M12-4: PTY 출력으로 Term이 갱신되었음을 알림. visible session인 경우만 redraw.
    SessionRepaint(SessionId),
    /// M12-4: PTY child process 종료. emit 측은 alive=false + exit_code 기록.
    SessionExited { id: SessionId, code: i32 },
    /// M12-4: PTY read 에러. fatal 처리(active 이동 + 다른 session도 다 죽으면 종료).
    SessionPtyError { id: SessionId, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(pub u64);

impl PaneId {
    pub const fn first() -> Self {
        Self(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);
