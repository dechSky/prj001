#[derive(Debug)]
pub enum UserEvent {
    /// M12-4: PTY 출력으로 Term이 갱신되었음을 알림. visible session인 경우만 redraw.
    SessionRepaint(SessionId),
    /// M12-4: PTY child process 종료. emit 측은 alive=false + exit_code 기록.
    SessionExited { id: SessionId, code: i32 },
    /// M12-4: PTY read 에러. fatal 처리(active 이동 + 다른 session도 다 죽으면 종료).
    SessionPtyError { id: SessionId, message: String },
    /// macOS NSMenu click → AppCommand. design: docs/menu-dispatch-design.md.
    MenuCommand(AppMenuCommand),
}

/// macOS NSMenu에서 click 가능한 명령. NSMenuItem.tag = repr i64 값.
/// MenuTarget의 menuAction: selector가 tag를 읽어 UserEvent::MenuCommand로 dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum AppMenuCommand {
    // Shell
    NewTab = 1,
    CloseActive = 2,
    SplitVertical = 3,
    SplitHorizontal = 4,
    CloseTab = 5,
    /// Cmd+N — 기존 keyboard chain이 split_active(Vertical)로 매핑돼 있음(NewPane).
    /// NSMenu 표시는 "New Pane"로. multi-window milestone 전엔 NewWindow 별도 미구현.
    NewPane = 6,
    // Edit
    Copy = 10,
    Paste = 11,
    SelectAll = 12,
    Find = 13,
    ClearBuffer = 14,
    ClearScrollback = 15,
    // View
    ZoomIn = 20,
    ZoomOut = 21,
    ZoomReset = 22,
    // Window
    PrevTab = 30,
    NextTab = 31,
}

impl AppMenuCommand {
    /// tag(i64)로부터 enum 변환. unknown tag면 None.
    pub fn from_tag(tag: i64) -> Option<Self> {
        Some(match tag {
            1 => Self::NewTab,
            2 => Self::CloseActive,
            3 => Self::SplitVertical,
            4 => Self::SplitHorizontal,
            5 => Self::CloseTab,
            6 => Self::NewPane,
            10 => Self::Copy,
            11 => Self::Paste,
            12 => Self::SelectAll,
            13 => Self::Find,
            14 => Self::ClearBuffer,
            15 => Self::ClearScrollback,
            20 => Self::ZoomIn,
            21 => Self::ZoomOut,
            22 => Self::ZoomReset,
            30 => Self::PrevTab,
            31 => Self::NextTab,
            _ => return None,
        })
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(pub u64);

/// M12-6: design §2.1 정합. ID 재사용 금지 monotonic counter.
/// AppState가 보유하던 raw next_*_id 필드 + allocate_* 메서드를 한 struct로 묶음.
#[derive(Debug, Default)]
pub struct IdAllocator {
    next_pane: u64,
    next_session: u64,
    next_tab: u64,
}

impl IdAllocator {
    pub fn new_pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane = self
            .next_pane
            .checked_add(1)
            .expect("pane id overflow (u64 exhausted)");
        id
    }

    pub fn new_session(&mut self) -> SessionId {
        let id = SessionId(self.next_session);
        self.next_session = self
            .next_session
            .checked_add(1)
            .expect("session id overflow (u64 exhausted)");
        id
    }

    pub fn new_tab(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab = self
            .next_tab
            .checked_add(1)
            .expect("tab id overflow (u64 exhausted)");
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codex 6차 개선: AppMenuCommand 모든 variant가 from_tag(repr)로 roundtrip.
    /// variant 추가 시 from_tag도 같이 갱신하도록 자동 검증.
    #[test]
    fn app_menu_command_tag_roundtrip() {
        let all = [
            AppMenuCommand::NewTab,
            AppMenuCommand::CloseActive,
            AppMenuCommand::SplitVertical,
            AppMenuCommand::SplitHorizontal,
            AppMenuCommand::CloseTab,
            AppMenuCommand::NewPane,
            AppMenuCommand::Copy,
            AppMenuCommand::Paste,
            AppMenuCommand::SelectAll,
            AppMenuCommand::Find,
            AppMenuCommand::ClearBuffer,
            AppMenuCommand::ClearScrollback,
            AppMenuCommand::ZoomIn,
            AppMenuCommand::ZoomOut,
            AppMenuCommand::ZoomReset,
            AppMenuCommand::PrevTab,
            AppMenuCommand::NextTab,
        ];
        for cmd in all {
            let tag = cmd as i64;
            let back = AppMenuCommand::from_tag(tag);
            assert_eq!(back, Some(cmd), "tag {tag} roundtrip 실패");
        }
        assert!(AppMenuCommand::from_tag(0).is_none());
        assert!(AppMenuCommand::from_tag(999).is_none());
    }

    #[test]
    fn id_allocator_starts_at_zero() {
        let mut ids = IdAllocator::default();
        assert_eq!(ids.new_pane(), PaneId(0));
        assert_eq!(ids.new_session(), SessionId(0));
        assert_eq!(ids.new_tab(), TabId(0));
    }

    #[test]
    fn id_allocator_is_monotonic() {
        let mut ids = IdAllocator::default();
        let a = ids.new_pane();
        let b = ids.new_pane();
        let c = ids.new_pane();
        assert_eq!(a, PaneId(0));
        assert_eq!(b, PaneId(1));
        assert_eq!(c, PaneId(2));
        // session counter는 pane counter와 독립.
        assert_eq!(ids.new_session(), SessionId(0));
        assert_eq!(ids.new_session(), SessionId(1));
        // tab counter도 독립.
        assert_eq!(ids.new_tab(), TabId(0));
        assert_eq!(ids.new_tab(), TabId(1));
    }

    #[test]
    fn id_allocator_does_not_reuse() {
        let mut ids = IdAllocator::default();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1024 {
            assert!(seen.insert(ids.new_pane()));
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1024 {
            assert!(seen.insert(ids.new_session()));
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1024 {
            assert!(seen.insert(ids.new_tab()));
        }
    }

    #[test]
    fn pane_and_session_id_are_orderable() {
        // M16 BTreeSet<SessionId> 대비 — PartialOrd/Ord derive 검증.
        let mut v = vec![SessionId(3), SessionId(1), SessionId(2)];
        v.sort();
        assert_eq!(v, vec![SessionId(1), SessionId(2), SessionId(3)]);

        let mut v = vec![TabId(3), TabId(1), TabId(2)];
        v.sort();
        assert_eq!(v, vec![TabId(1), TabId(2), TabId(3)]);
    }
}
