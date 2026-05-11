#[derive(Debug)]
pub enum UserEvent {
    Repaint(PaneId),
    ChildExited { pane: PaneId, code: i32 },
    PtyError { pane: PaneId, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub usize);

impl PaneId {
    pub const fn first() -> Self {
        Self(0)
    }
}
