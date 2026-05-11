#[derive(Debug)]
pub enum UserEvent {
    Repaint(PaneId),
    ChildExited { pane: PaneId, code: i32 },
    PtyError { pane: PaneId, message: String },
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
