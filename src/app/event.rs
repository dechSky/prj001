#[derive(Debug)]
pub enum UserEvent {
    Repaint,
    ChildExited(i32),
    PtyError(String),
}
