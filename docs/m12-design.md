# M12 — Session 추출 (stale stub)

> **stale stub** — M12 구현은 commits `e517d42`~`6af3659`로 완료됨 (2026-05-11). 원본 v2 설계 본문은 git history 참조: `git log -- docs/m12-design.md`.

## 결과 요약

- PTY 프로세스(`Session`)와 보이는 슬롯(`Pane`)을 분리. 외부 동작은 M11과 동일.
- `SessionId(u64)` / `PaneId(u64)` monotonic ID 도입 (no reuse, Ord derive).
- `Pane` 슬림화: id/session/cols/rows/col_offset/status_row.
- `AppState.sessions: HashMap<SessionId, Session>` + `panes: Vec<Pane>`.
- `PaneViewport` 분리 (cols/rows/col_offset/status_row + x_px/y_px/width_px/height_px).
- `IdAllocator` struct로 ID 발급 통합.
- 회귀 0 (M11 75 tests → M12 86 tests).

## 후속 milestone

- M13 BSP layout tree → 완료 (commits `d716b70`~`ed52803`)
- M14 tabs → 완료 (commits `d3f8dcc` etc.)
- M15+ AgentKind/Bridge → 후속 milestone

본 문서는 단순 stub. 상세 trajectory는 `docs/roadmap.md` 참조.
