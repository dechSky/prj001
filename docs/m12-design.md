# M12 — Session 추출 (behavior-preserving refactor)

> **⚠ STALE — 본 파일은 superseded되었습니다 (2026-05-11)**.
> 정본: archive `docs/architecture/m12-pj001-session-extraction-design.md` v2 통합본.
> 본 파일은 §0 line 62 sketch에 `kind: AgentKind`가 §2.2 결정(비도입)과 모순, §7.1 test #5/#14/#15도 모순. 모두 정본에서 정리됨.
> **코드 진입 전 반드시 archive 정본을 참조하세요.** 본 파일 sync는 후속 pj001 세션 책임.

**상태**: 설계 v2 (2026-05-11) — **archive 정본으로 대체됨**. 코드 미진입. archive `docs/architecture/m12-m16-pj001-sessions-tabs-bridge-plan.md` §M12를 ancestor로 두고 Codex 2nd opinion(2026-05-11, thread `019e15ef`) blocking 2건 + 권고 5건 반영.
**v2 변경 (vs v1, Codex 검토 반영)**: AgentKind는 core에서 제거(plan §2.2 보정 — Claude/Codex variant는 core/archive 분리 원칙 위반), JSON schema v1 유지 명시(내부 Rust 타입만 SessionId), 단계 분해 재정렬(API boundary 별도 step 분리), Ord trait + checked_add + spec_index 변수명, RouteEvent M12 정리 정책 명시, 회귀 시나리오 +9.
**목적**: "PTY 프로세스"와 "보이는 슬롯"을 분리한다. 외부 동작은 M11과 완전히 동일하다. M13(Layout tree) / M14(Tabs) / M15(AgentKind 활용) / M16(Bridge)의 토대가 되는 자료구조 분리만 수행.
**범위**: refactor only. 새 UX 없음, 새 단축키 없음, 새 CLI flag 없음.
**선행**: M11 workspace 분리 완료(commit `396a536`), cargo test 75 통과 (M11 시점 기준).
**스타일**: M7/M8/M10/M17 design.md 패턴. 단계 분해 5개(M12-1~M12-5).

---

## 0. 배경

M11 종료 시점의 `pj001-core` 구조:

```rust
// crates/pj001-core/src/app/mod.rs

struct AppState {
    // ...
    panes: Vec<Pane>,   // size 0..2, RunMode에 따라 결정
    active: PaneId,
    // ...
}

struct Pane {
    id: PaneId,              // pub struct PaneId(pub usize)
    title: String,
    pty: PtyHandle,
    term: Arc<Mutex<Term>>,
    cols: usize,
    rows: usize,
    col_offset: usize,
    status_row: Option<usize>,
    alive: bool,
}

pub enum UserEvent {
    Repaint(PaneId),
    ChildExited { pane: PaneId, code: i32 },
    PtyError { pane: PaneId, message: String },
}
```

문제:
- `Pane`이 **PTY 프로세스(논리) + viewport(시각)** 두 역할을 겸한다.
- M13(BSP layout tree) 진입 시 layout이 PaneId를 참조해야 하는데, Pane이 viewport까지 보유하면 tree mutation과 PTY lifecycle이 결합된다 → 안티패턴 #1(layout tree와 pane lifecycle 결합).
- `UserEvent.Repaint(PaneId)`는 본질적으로 "Term이 바뀌었다"는 신호인데 PaneId가 Term을 직접 식별하지 않는다. 비활성 tab(M14)의 Term mutation은 redraw가 필요 없지만, 현 시그니처로는 구별 불가.
- `PaneId(pub usize)` — 외부 hook(`RouteSink`/`LifecycleSink`)에 노출되는데 `usize`는 의미 불명(index? 핸들?). Codex 권고대로 `u64` newtype + 재사용 금지 정책 필요.

M12 종료 시점 목표:

```rust
struct AppState {
    sessions: HashMap<SessionId, Session>,   // PTY 프로세스
    panes: Vec<Pane>,                         // viewport (M11과 동일 size 0..2)
    active: PaneId,
}

struct Session {
    id: SessionId,
    kind: AgentKind,         // enum 정의만, M15에서 활용
    title: String,
    pty: PtyHandle,
    term: Arc<Mutex<Term>>,
    alive: bool,
    exit_code: Option<i32>,
}

struct Pane {
    id: PaneId,
    session: SessionId,      // 어떤 session을 보여주는가
    viewport: PaneViewport,  // pixel + cell 메트릭
}

pub enum UserEvent {
    SessionRepaint(SessionId),
    SessionExited { id: SessionId, code: i32 },
    SessionPtyError { id: SessionId, message: String },
}
```

외부 동작 변경 없음. 컴파일 에러로 일괄 변경 잡힘.

---

## 1. 핵심 결정

| 항목 | 결정 | 출처 |
|---|---|---|
| Session/Pane 분리 | 별도 struct로 분리, AppState가 두 컬렉션 보유 | m12-m16 plan §M12 |
| Session 키 자료구조 | `HashMap<SessionId, Session>` (Vec 아님) | plan §2.7 — M14+ tab 분산 대비 |
| Pane 키 자료구조 | M12는 `Vec<Pane>` 유지 (M11과 동일 size 0..2) | plan §M12 |
| **PaneId/SessionId 크기** | **`u64`** (plan §2.1의 `u32` 보정) | **Codex refinement #1** — 외부 hook 노출 시 재사용 금지·breaking change 비용 회피 |
| ID 재사용 정책 | **monotonic counter, 재사용 금지** + `checked_add(1)` overflow panic | Codex |
| ID derive | `Copy + Eq + PartialOrd + Ord + Hash + Debug` (Ord는 M16 BTreeSet<SessionId> 대비 선제) | Codex v2 권고 |
| **AgentKind 도입 시점** | **M12 비도입 (Claude/Codex variant는 core/archive 분리 원칙 위반)**. Session에 kind 필드 없음. archive-bridge가 필요하면 외부 map 관리. M15에서 PaneMeta/SessionMeta와 함께 재검토 | **Codex v2 blocking** — plan §2.2 보정 |
| SessionSpec index vs SessionId 명명 | spec index 변수는 `spec_index`로, runtime ID는 `SessionId`로 엄격 구분 | Codex v2 권고 |
| UserEvent 시그니처 | PaneId → SessionId 기반 | plan §2.8 |
| reader thread 인자 | `pane: PaneId` → `session: SessionId` | plan §M12 |
| viewport 분리 | `PaneViewport { x_px, y_px, width_px, height_px, cols, rows, col_offset, status_row }` | plan §2.3 |
| 마우스 routing | `pane viewport hit-test → pane.id → pane.session` 명시 단계 분리 | plan §M12 |
| AgentKind 배치 (M15에서 재검토) | M12는 plan대로 Session.kind. M15 도입 시 `PaneMeta` 분리 여부 결정 | Codex refinement #4 — 노트만 남김 |

### Codex refinement / blocking 처리 매트릭스

| # | refinement | M12 영향 | 처리 |
|---|---|---|---|
| 1 | ID `u32 → u64` | ✅ 직접 적용 | SessionId/PaneId/TabId/BridgeId 모두 `u64` + Ord derive |
| 2 | ratio `f32 → Ratio(basis_points u16)` | M13 영역 | M13 design 진입 시 결정 노트 |
| 3 | `SplitDir → Axis::LeftRight/TopBottom` | M13 영역 | M13 design 진입 시 결정 노트 |
| 4 | AgentKind 배치 | ✅ **v2 blocking — M12에서 AgentKind 도입 자체 제거**. core/archive 분리 원칙 우선 | M15에서 PaneMeta + archive-bridge 외부 map 함께 설계 |
| 5 (v2) | archive-bridge JSON schema 호환 | ✅ **v2 blocking — schema v1 유지** | 내부 Rust 타입만 SessionId. `"session": 0` JSON field 유지 |
| 6 (v2) | RouteEvent 정리 시점 | ✅ M12에서 함께 정리 (route_sink 미호출 상태라 비용 낮음) | RouteEvent.from/to → from_session/to_sessions: SessionId |
| 7 (v2) | API boundary 별도 commit step | ✅ M12-2를 "API boundary" 단계로 신설 | 5단계 → 6단계 재정렬 |

---

## 2. 자료구조 변경

### 2.1 ID newtype (Codex refinement #1 + Ord derive 적용)

```rust
// crates/pj001-core/src/app/mod.rs (또는 별도 ids.rs)

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

// PaneId는 M11에서 PaneId(pub usize). u64로 변경.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(pub u64);

// M14에서 도입 예정. M12 단계에선 미사용.
// #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
// pub struct TabId(pub u64);
```

**Ord 선제 derive 사유**: M16 `Bridge { members: BTreeSet<SessionId> }` (plan §2.6) 대비. M12에서 미리 두면 후속 마찰 0.

**`pub u64` 정책**: archive-bridge가 `.0`으로 JSON serialize하는 현재 구조 유지. 장기적으로 `pub fn as_u64(self) -> u64` 헬퍼 + private field로 좁힐 수 있으나 M12는 refactor-only라 보류.

**재사용 금지 정책**: AppState에 `next_session_id: u64`, `next_pane_id: u64` monotonic counter. 매 spawn 시 `checked_add(1).expect("...")`로 overflow 명시.

```rust
struct AppState {
    next_session_id: u64,
    next_pane_id: u64,
    // ...
}

impl AppState {
    fn next_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_session_id);
        self.next_session_id = self.next_session_id
            .checked_add(1)
            .expect("session id overflow (u64 exhausted)");
        id
    }
    fn next_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id = self.next_pane_id
            .checked_add(1)
            .expect("pane id overflow (u64 exhausted)");
        id
    }
}
```

PaneId의 u32 → u64 변경은 외부 API(`RouteSink`/`LifecycleSink`의 PaneId 참조 모두)에 영향. **M12 commit chain의 첫 번째 commit이 ID 시그니처 변경**, 나머지 변경의 토대.

**SessionSpec index vs SessionId 변수명 분리**: `InitialLayout::Single { session: usize }`의 `session`은 `Config.sessions: Vec<SessionSpec>`의 index이고, runtime `SessionId`와 다름. 내부 코드에서 둘이 같은 이름이면 bug source. M12에서 spec index 변수는 모두 `spec_index`로 rename, runtime은 `SessionId` 그대로.

### 2.2 AgentKind — M12 비도입 (Codex v2 blocking)

**plan §2.2 보정**: `AgentKind::{Claude, Codex, Shell, Custom}`을 pj001-core `Session.kind`에 넣지 **않는다**. 이유:

- commit `396a536` "M11: pj001 core workspace 분리" 메시지: "archive 전용 의미가 core에 남지 않도록 Config 기반 public API 정리"
- modularization 원칙: pj001-core에는 Claude/Codex/archive 의미가 남으면 안 됨
- M12는 refactor-only — 새 의미 도입 자체가 scope 외

**M12 처리**:
- `Session`에 `kind` 필드 없음
- `RunMode::Bridge → (Claude, Codex)` 매핑 로직은 thin binary(`crates/pj001/src/main.rs`)에 둠. core는 `CommandSpec::Custom("claude")`와 `CommandSpec::Custom("codex")`로만 받음
- archive-bridge가 Claude/Codex 구분이 필요하면 자체 `HashMap<SessionId, AgentMeta>`로 외부 관리
- M15 design 진입 시 `PaneMeta` 또는 `SessionMeta` 도입 여부 + 위치(core vs archive-bridge) 결정

**core에 남기는 generic 대안 (M15 선택지)**:
```rust
// M15에서 결정. M12에는 도입 안 함.
pub enum SessionKind {
    Shell,
    Custom(String),
}
```
M15 진입 전까지 core는 `command: String` 하나로 충분.

### 2.3 Session struct (PTY 프로세스 분리)

```rust
pub struct Session {
    pub id: SessionId,
    pub title: String,
    pub command: String,         // spawn 시 사용한 원본 명령 (CommandSpec 해석 후)
    pub pty: PtyHandle,
    pub term: Arc<Mutex<Term>>,
    pub alive: bool,
    pub exit_code: Option<i32>,
    pub created_at: Instant,
}
```

M11의 `Pane.{pty, term, title}`이 Session으로 이관. M11의 `Pane.alive`는 Session.alive로 이관.

**`kind` 필드 없음** (Codex v2 blocking — §2.2 참조). M15에서 `PaneMeta`/`SessionMeta` 도입 시 별도 결정.

M12에서는 plan §2.2의 `status_hint: SessionStatusHint` 필드 도입을 **보류**. M15에서 활용 시점에 함께 도입. M12는 `alive: bool + exit_code: Option<i32>`로 충분.

### 2.4 Pane 슬림화 (viewport만)

```rust
pub struct Pane {
    pub id: PaneId,
    pub session: SessionId,       // M14까지 1 Session = 0..1 Pane
    pub viewport: PaneViewport,
}

pub struct PaneViewport {
    pub x_px: u32,
    pub y_px: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub cols: usize,
    pub rows: usize,
    pub col_offset: usize,        // 현재 split의 좌측 col offset
    pub status_row: Option<usize>, // status bar row index (M11과 동일)
}
```

M11의 `Pane.{cols, rows, col_offset, status_row}`이 viewport로 묶임. pixel_width/pixel_height는 PTY resize에 사용.

### 2.5 AppState 변경

```rust
struct AppState {
    // window/surface/device/queue/renderer 기존 그대로
    sessions: HashMap<SessionId, Session>,
    panes: Vec<Pane>,                       // M11과 동일 size 0..2
    active: PaneId,
    next_session_id: u64,
    next_pane_id: u64,
    hooks: Hooks,
    // M11 잔존: last_ime_cursor, preedit, cursor_visible, last_blink, focused,
    //          cursor_blinking_cache, modifiers, last_mouse_pos, pending_resize
}
```

`active`는 M14까지 단일 (M14에서 tab별 active_pane으로 이동). M12는 plan §I3 invariant 유지 — "1 Session = 0..1 Pane".

---

## 3. UserEvent + LifecycleEvent + RouteEvent 시그니처 변경

### 3.1 신/구 매핑

```rust
// M11
pub enum UserEvent {
    Repaint(PaneId),
    ChildExited { pane: PaneId, code: i32 },
    PtyError { pane: PaneId, message: String },
}

// M12
pub enum UserEvent {
    SessionRepaint(SessionId),
    SessionExited { id: SessionId, code: i32 },
    SessionPtyError { id: SessionId, message: String },
}
```

### 3.2 변경 사유

- reader thread는 PTY 출력 → Term mutation만 담당. PaneId는 시각 슬롯이라 reader가 알 필요 없음.
- M14 비활성 tab의 Term mutation은 redraw 비용 0이어야 한다. SessionId 기반이면 `app::handle_user_event`가 "이 session이 현재 visible한가" 확인 후 redraw 결정 가능.

### 3.3 app::handle_user_event 흐름 (방어적 lookup)

```rust
match event {
    UserEvent::SessionRepaint(session_id) => {
        // M12는 1 Session = 1 Pane. lookup 실패 시 panic 아닌 debug log + ignore.
        // (M13~M15에서 close/respawn 도입 시 race condition 방어용 — 미리 안전 정책 수립)
        if !self.sessions.contains_key(&session_id) {
            log::debug!("SessionRepaint for unknown session_id={:?}, ignore", session_id);
            return;
        }
        if self.panes.iter().any(|p| p.session == session_id) {
            self.window.request_redraw();
        }
    }
    UserEvent::SessionExited { id, code } => {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.alive = false;
            session.exit_code = Some(code);
        } else {
            log::debug!("SessionExited for unknown session_id={:?}, ignore", id);
            return;
        }
        self.window.request_redraw();
        if let Some(sink) = &self.hooks.lifecycle_sink {
            sink.on_lifecycle(LifecycleEvent::SessionExited {
                session_id: id,
                code,
            });
        }
    }
    // ...
}
```

Codex v2 권고: `sessions.get(&id)` 실패 시 panic하지 말고 debug log + ignore. M12에서는 session 제거가 없지만, M13~M15에서 close/respawn 도입 시 동일 정책 그대로 활용.

### 3.4 LifecycleEvent 시그니처

M11:

```rust
pub enum LifecycleEvent {
    SessionStarted { session: usize, title: String },
    SessionExited { session: usize, code: i32 },
}
```

M12 (내부 Rust 타입만 변경 — **archive-bridge JSON schema_version v1 유지**):

```rust
pub enum LifecycleEvent {
    SessionStarted { session_id: SessionId, title: String },
    SessionExited { session_id: SessionId, code: i32 },
}
```

**archive-bridge JSON schema 호환 정책 (Codex v2 blocking)**:

- 내부 Rust event field: `session_id` (SessionId 타입)
- archive-bridge가 calls.jsonl에 쓰는 JSON field: **`"session": <u64>` 그대로 유지** (schema v1)
- transcript.md markdown field: **`**session**: <u64>` 그대로 유지**
- archive-bridge 측 serde rename: `#[serde(rename = "session")]` 또는 manual field write

이유: schema_version v1 호환이 "Rust 타입만 바뀌고 외부 wire format은 동일"을 의미해야 명확. dual-write(`"session"` + `"session_id"`)는 장기적 cruft. M16 bridge graph schema 변경 시 `schema_version: 2`로 함께 정리.

archive 측 `tail-*` script가 lifecycle JSON을 파싱하지 않는다면 영향 없음. 파싱한다면 schema v1 그대로라 변경 없음.

### 3.5 RouteEvent 시그니처 (M12에서 함께 정리)

M11:

```rust
pub struct RouteEvent {
    pub from: usize,
    pub to: Vec<usize>,
    pub bytes: Vec<u8>,
}
```

M12 (Codex v2 권고 — route_sink 미호출 상태에서 함께 정리, public API break 비용 최저):

```rust
pub struct RouteEvent {
    pub from_session: SessionId,
    pub to_sessions: Vec<SessionId>,
    pub bytes: Vec<u8>,
}
```

M11 design hooks.rs 주석 "route_sink is reserved for the future generic routing primitive and is not invoked yet" 명시. M12에서 invoke 추가는 없음. 타입만 정리.

archive-bridge consumer가 route_sink를 구현했다면 trait 시그니처 변경으로 컴파일 에러 일괄. 현재 archive `tools/archive-bridge`는 lifecycle_sink만 구현하므로 route_sink 변경은 영향 없음 (M16에서 실제 사용 시점에 archive 동시 패치).

---

## 4. reader thread 시그니처 변경

M11 `crates/pj001-core/src/pty/reader.rs`:

```rust
pub fn spawn(pane: PaneId, pty: PtyHandle, term: Arc<Mutex<Term>>, proxy: EventLoopProxy<UserEvent>) {
    // ... vte parser loop ...
    // proxy.send_event(UserEvent::Repaint(pane));
}
```

M12:

```rust
pub fn spawn(session: SessionId, pty: PtyHandle, term: Arc<Mutex<Term>>, proxy: EventLoopProxy<UserEvent>) {
    // ... vte parser loop ...
    // proxy.send_event(UserEvent::SessionRepaint(session));
}
```

호출 측(`AppState::new` 또는 spawn 헬퍼)에서 Session 생성 시점에 SessionId 발급 + reader spawn.

---

## 5. mouse routing 명시화

M11에서 마우스 좌표 → pane 매핑은 fixed 2-split 기반(`pane_layouts(count<=2)`). M12는 layout이 그대로(M13에서 BSP 도입)이지만, 라우팅 경로를 명시 단계로 분리해 M13 진입 시 변경점이 명확해야 한다.

```rust
impl AppState {
    fn pane_at(&self, pos: PhysicalPosition<f64>) -> Option<PaneId> {
        let (x, y) = (pos.x as u32, pos.y as u32);
        for pane in &self.panes {
            let vp = &pane.viewport;
            if (vp.x_px..vp.x_px + vp.width_px).contains(&x)
                && (vp.y_px..vp.y_px + vp.height_px).contains(&y)
            {
                return Some(pane.id);
            }
        }
        None
    }

    fn session_at(&self, pos: PhysicalPosition<f64>) -> Option<SessionId> {
        self.pane_at(pos)
            .and_then(|id| self.panes.iter().find(|p| p.id == id))
            .map(|p| p.session)
    }
}
```

M11의 인라인 hit-test 로직을 `pane_at`/`session_at`로 추출. wheel scroll, click(focus 전환), drag 시작점 모두 같은 헬퍼 사용.

---

## 6. 단계 분해 (M12-1 ~ M12-6, Codex v2 재정렬)

**v2 재정렬 사유**: Codex 권고 — "API boundary" 별도 step 분리. UserEvent + LifecycleEvent + RouteEvent의 public boundary 정리를 한 번에 묶고, 이후 내부 자료구조(Session 추출) 진행. Viewport는 마지막.

각 단계 종료 시 cargo test 통과 + 회귀 0. commit 권장.

### M12-1 — ID newtype + monotonic counter

**범위**:
- `SessionId(pub u64)`, `PaneId(pub u64)` 정의 (Ord + checked_add 포함).
- M11 `PaneId(pub usize)` 교체.
- `AppState.next_session_id`, `next_pane_id: u64` 추가 + `next_session_id()`/`next_pane_id()` 메서드.
- 모든 PaneId 사용처 컴파일 에러 일괄 수정.
- `RouteEvent.from/to`, `LifecycleEvent.session`은 **M12-2에서 처리** (시그니처 안정성 위해 한 step에 묶음).

**검증**: cargo test 회귀 0. 시각 변화 없음.

**commit message**:
```
M12-1: PaneId u32→u64 + SessionId 도입 + monotonic counter

외부 hook(RouteSink/LifecycleSink)에 노출되는 ID 재사용 금지 정책
도입. Ord trait 선제 derive로 M16 BTreeSet<SessionId> 대비.
checked_add(1)로 overflow 명시. M13/M14/M15/M16에서 ID stability 의존.
```

### M12-2 — public API boundary 정리 (LifecycleEvent + RouteEvent + archive-bridge)

**범위 (Codex v2 권고 — 단일 commit에 묶음)**:
- `LifecycleEvent { session_id: SessionId, ... }` — Rust 타입만 SessionId
- `RouteEvent { from_session: SessionId, to_sessions: Vec<SessionId>, bytes }` — route_sink 미호출 상태에서 함께 정리
- archive `tools/archive-bridge` 동시 패치:
  - lifecycle_sink 구현체 SessionId 인자 처리
  - calls.jsonl JSON serialize는 **`"session": <u64>` 그대로 유지** (schema v1 호환)
  - transcript markdown field `**session**: <u64>` 그대로 유지
  - serde rename 또는 manual write로 처리
- `LifecycleSink` trait — local 프로젝트라 외부 사용자 없음, 단 design에 public API break 명시

**검증**: cargo test 회귀 0. archive-bridge `cargo test` 통과. `cargo run -- --bridge --left /bin/zsh --right /bin/zsh` 후 한쪽 exit 시 calls.jsonl에 `"session": <u64>` 정상 기록 + transcript markdown 정상.

**commit 순서**: pj001 commit → archive 동시 commit. 두 repo 별 branch면 archive PR를 pj001 commit 직후 머지.

### M12-3 — Session struct 추출

**범위**:
- `Session` struct 도입 (`pj001-core/src/app/session.rs` 신설 또는 mod.rs 안 module). **`kind` 필드 없음 (Codex v2 blocking)**
- M11 `Pane.{pty, term, title, alive}` → `Session`으로 이관
- `AppState.sessions: HashMap<SessionId, Session>` 도입
- Session spawn 헬퍼: `spawn_session(spec: SessionSpec) -> Result<Session>`
- `Pane.session: SessionId` 필드 추가 (M12-3 시점엔 Pane은 여전히 cols/rows 등 viewport 필드 보유 — M12-5에서 정리)
- `SessionSpec` index 사용처 변수명을 `spec_index`로 rename (SessionId와 명명 충돌 방지)

**검증**: cargo test 회귀 0. 시각 변화 없음. `cargo run -- --bridge --left /bin/zsh --right /bin/zsh` 정상.

### M12-4 — UserEvent + reader thread SessionId 전환

**범위**:
- `UserEvent` enum SessionId 기반으로 변경
- `pty::reader::spawn` 시그니처 변경 (`pane: PaneId` → `session: SessionId`)
- `app::handle_user_event` SessionId → Pane lookup 패턴 도입 (방어적 — `sessions.get(&id)` 실패 시 debug log + ignore)
- reader는 PaneId를 모름

**검증**: cargo test 회귀 0. `--bridge` 두 PTY 시작/exit 모두 정상 lifecycle hook 호출. 양 pane 동시 burst output 시 SessionId 뒤섞임 없음.

### M12-5 — PaneViewport 분리

**범위**:
- `PaneViewport { x_px, y_px, width_px, height_px, cols, rows, col_offset, status_row }` struct 신설
- M11 `Pane.{cols, rows, col_offset, status_row}` → `PaneViewport`로 이관
- pixel_width/pixel_height 명시 필드 추가 (M13 BSP 진입 대비)
- 기존 `pane_layouts(size, cell, count) -> Vec<PaneLayout>`를 `compute_viewports`로 rename (M13에서 BSP 기반으로 재작성 예정 — 이름 미리 정합)

**검증**: cargo test 회귀 0. 리사이즈 동작 동일. `stty size`를 양 pane에서 확인 — pty.resize lookup 정확성.

### M12-6 — mouse routing 명시화

**범위**:
- `AppState::pane_at(pos) -> Option<PaneId>`, `session_at(pos) -> Option<SessionId>` 헬퍼 추출
- M11 인라인 hit-test 로직 모두 헬퍼로 교체
- 마우스 wheel/click/drag 경로 통일 (active pane 기준 scrollback 유지)
- M12-6에서 너무 일반화하지 말 것 — M13 BSP에서 layout-aware로 재작성 예정

**검증**: cargo test 회귀 0. trackpad scroll, focus 전환 클릭 모두 정상. 비active pane 위 wheel scroll이 active pane 기준 유지(M11 동일).

---

## 7. 테스트 목록

### 7.1 신규 unit test

| # | 테스트 | 위치 |
|---|---|---|
| 1 | `SessionId`/`PaneId` monotonic 발급 (재사용 안 됨) | app::mod tests |
| 2 | `next_session_id`/`next_pane_id` 1000회 호출 후 unique | app::mod tests |
| 3 | `AppState.sessions.insert` + `get(SessionId)` 정상 lookup | app::mod tests |
| 4 | `Pane.session` 참조 정합성 (등록된 SessionId만 가능) | app::mod tests |
| 5 | `spawn_session(SessionSpec::Shell)` → Session.kind == Shell | app::mod tests |
| 6 | `spawn_session(SessionSpec::Custom("claude"))` → command 정확 | app::mod tests |
| 7 | `pane_at(pos)` hit-test — 좌측 pane 영역 클릭 시 Pane 0 반환 | app::mod tests |
| 8 | `pane_at(pos)` — 우측 pane 영역 | app::mod tests |
| 9 | `pane_at(pos)` — divider 영역 (None) | app::mod tests |
| 10 | `pane_at(pos)` — status row 영역 (Pane 매핑 정책 결정) | app::mod tests |
| 11 | `session_at(pos)` — pane → session 두 단계 lookup | app::mod tests |
| 12 | `UserEvent::SessionRepaint` dispatch 시 redraw 호출 | app::mod tests (proxy mock) |
| 13 | `UserEvent::SessionExited` 시 Session.alive=false, exit_code 설정 | app::mod tests |
| 14 | `AgentKind` enum 정의 (Claude/Codex/Shell 3종) | app::mod tests |
| 15 | `RunMode::Bridge` → Claude/Codex AgentKind 자동 매핑 | app::mod tests |

### 7.2 회귀 (M11 시각 검증 시나리오) — Codex v2 +9 추가

기본 10개 (M11 시점 검증):

1. `pj001` (single shell) — zsh prompt 시작
2. `pj001 --shell /bin/bash` — bash prompt 시작
3. `pj001 --bridge --left /bin/zsh --right /bin/zsh` — 좌우 split
4. Cmd+1 / Cmd+2 active 전환
5. Cmd+V (clipboard paste)
6. Trackpad scroll (scrollback view)
7. 한글 IME composition (ㄴ→나→난)
8. vim 시작 (alt screen 진입)
9. vim 종료 시 main screen 복원
10. focus 잃었다가 복원 (focus reporting)

Codex v2 추가 시나리오 (M12 자료구조 변경에 특화된 회귀):

11. **한쪽 child만 종료**: `--bridge --left /bin/zsh --right /bin/zsh`에서 왼쪽만 `exit`. 오른쪽 alive 유지, dead status, active 전환 정상.
12. **종료된 pane에 Cmd+Enter/Cmd+V 시도**: dead session write가 warning/no-op, panic 없음.
13. **빠른 output 중 resize**: `yes | head -10000` 또는 `find /usr -maxdepth 3` 중 창 resize. reader repaint가 session lookup 실패나 stale viewport를 만들지 않음.
14. **양 pane 동시 burst output**: 양쪽에서 `for i in {1..500}; do echo $i; done`. SessionId 뒤섞임 없음.
15. **resize 후 PTY size 정확성**: `stty size`를 양 pane에서 확인. PaneViewport 분리 후 session.pty.resize lookup 정확.
16. **pending VT responses (DSR/DA)**: vim startup, DSR/DA 응답이 여전히 해당 session PTY로 돌아감. `render()`가 pane.term 직접 순회에서 session.term lookup으로 바뀐 경로 검증.
17. **OSC title/cwd**: session title이 `Session.title`로 이동 후 title update가 pane status에 정상 반영.
18. **mouse wheel over inactive pane**: scrollback routing이 active pane 기준 유지. M12-6 직접 목표.
19. **IME cursor area after active switch**: Cmd+1/2로 active 변경 직후 한글 preedit 위치 정확.

각 시나리오는 M11과 시각/동작 동일해야 함.

### 7.3 archive consumer 회귀

`tools/archive-bridge` lifecycle 로그가 새 SessionId 기반 LifecycleEvent를 정상 소비:

- **JSON wire format**: `"session": <u64>` 그대로 유지 (schema v1) — serde rename 또는 manual write로 처리
- **transcript markdown field**: `**session**: <u64>` 그대로 유지
- **calls.jsonl schema_version**: v1 유지

archive 측 `archive-bridge` crate는 lifecycle_sink 구현체가 새 LifecycleEvent enum의 `session_id: SessionId` 필드를 받아 JSON `"session"` 필드로 write — 내부 Rust 변경에 따른 동시 commit 필요(M12-2 commit과 페어).

---

## 8. 알려진 한계 / 비목표

### M12 비목표 (m12-m16 plan §M12 그대로)

- 새 UX 없음.
- 새 단축키 없음.
- 새 CLI flag 없음.
- AgentKind는 enum 정의만, 사용은 M15.
- SessionStatusHint(Spawning/Active/Idle/Dead) 도입은 M15.
- layout tree는 M13.
- tabs는 M14.

### Codex refinement 보류 (M15에서 재평가)

- `Session.kind` 직접 보유 vs `PaneMeta { kind, title, cwd, labels }` 분리 — M15 진입 시 사용처 보고 결정.
- `PaneEventSink` hook 신설 — M15 또는 M16에서 필요 시.

---

## 9. 위험 / 결정점

### 9.1 위험

| 영역 | risk | 완화 |
|---|---|---|
| **UserEvent breaking change** | reader thread + app::handle_user_event + archive-bridge 모두 영향 | 컴파일 에러로 일괄 잡힘. archive 쪽 commit 순서 조율 |
| **PaneId u32 → u64 cast** | archive consumer JSON serialize 영향 (u32 → u64 number 표현 변화 없음) | cast 1곳 추가, 위험 낮음 |
| **mouse routing layout 의존** | M11 fixed 2-split 기반. M13에서 BSP로 재작성 시 다시 손댐 | M12-5에서 너무 일반화하지 말 것. 단순 viewport hit-test만 |
| **archive-bridge LifecycleEvent schema 변경** | `session: usize` → `session_id: u64` 동시 패치 필요 | M12-4 commit 직후 archive 측 동시 commit. transcript schema는 M12에서 변화 없음 |
| **HashMap<SessionId, Session> overhead** | 0..2 size에 HashMap 과한가? | 가독성·M14 분산 대비 일관성 우선. perf 영향 0 |

### 9.2 결정점 (archive m12-m16 plan §6 중 M12 관련)

m12-m16 plan §6의 7개 결정점은 모두 M13~M16 대상. M12는 결정점 없음. **Derek 확인 필요 항목 없음**.

단, **Codex refinement 4건 중 M15에서 재평가할 1건**: AgentKind 배치(Session.kind 직접 vs PaneMeta 분리). M12는 plan 그대로 Session.kind 진행, M15 design 진입 시 사용처 패턴 보고 PaneMeta 도입 여부 최종 결정.

---

## 10. 완료 기준

1. cargo test 전부 통과 (회귀 0). 신규 unit test 15개 추가.
2. M11 시각 검증 시나리오 10개 모두 통과.
3. archive `tools/archive-bridge` lifecycle 로그가 새 SessionId 기반 LifecycleEvent를 정상 소비 (calls.jsonl schema_version=v0/v1 호환).
4. `git diff`가 자료구조 분리 + 매핑 adapter 외 동작 변경 없음을 보여줌.
5. `memory/projects/pj001/overview.md`에 M12 완료 항목 append (M1~M11 같은 형식).

---

## 11. 참조

- [m12-m16-pj001-sessions-tabs-bridge-plan.md](../../archive/docs/architecture/m12-m16-pj001-sessions-tabs-bridge-plan.md) — 5 milestone trajectory 정본 (archive)
- [m11-pj001-archive-bridge-design.md](../../archive/docs/architecture/m11-pj001-archive-bridge-design.md) — M11 design v1 (archive)
- [pj001-core-archive-bridge-modularization-plan.md](../../archive/docs/architecture/pj001-core-archive-bridge-modularization-plan.md) — M11 A-D 완료 기록 (archive)
- Codex 2nd opinion (2026-05-11) — `~/.cc-bridge/transcript.md` thread `019e15ef-1dfa-7d70-964b-84dda9bf8cdd`
- `pj001/docs/architecture.md` — 단일 바이너리 / 6 모듈 구조 (M13 BSP 도입 시 §5.5/§8 갱신)
- `pj001/docs/keyboard-design.md` — M13~M16 새 단축키 명세 추가 위치

---

**Author**: Claude Opus 4.7 (1M context) — 2026-05-11 design v1. Codex refinement 4건 중 M12 영역(#1 ID u64)만 직접 반영. 나머지 #2/#3/#4는 M13/M15 design 진입 시 결정 노트로 남김.
