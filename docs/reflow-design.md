# M17 — Resize Reflow 설계

**상태**: 설계 v1 (2026-05-08). 코드 미진입.
**목적**: 윈도우 리사이즈 시 logical line 단위로 다시 wrap하여 내용을 보존. 정책 변경: 미지원(truncate) → 지원.
**범위**: main grid + scrollback 통합 reflow. alt screen은 truncate 유지.
**선행/병행**: 독립. M9까지 완료 코드 위에서 진행. selection/clipboard(M12)는 미존재라 selection 좌표 안정성 고려 불필요.
**스타일**: M7/M8 패턴. 단계 분해 5개 + 각 단계 검증.

---

## 0. 배경

architecture.md §8에서 reflow는 **자리 비움**으로 명시됨. wrap line metadata는 `Cell`이 아닌 `Vec<RowMeta>` per Grid에 두기로 결정되어 있음. 본 문서가 그 결정을 구현으로 넘기는 단계.

현재 `Term::resize`는 truncate-only. `Grid::resize`가 overlap 영역만 복사 + 나머지는 default cell. scrollback row는 push 시점의 col 수를 그대로 보유, `Term::cell` read 시점에 truncate-on-read.

긴 명령 출력 (`cat large.txt`, `ls -l` 등)이 wrap된 상태에서 창을 키워도 wrap이 그대로라 "한 줄로 합쳐 다시 보고 싶다"가 안 됨. **본 작업의 사용자 가치 = 이거**.

## 1. 핵심 결정

| 항목 | 결정 | 출처 |
|---|---|---|
| scope | main + scrollback 통합 reflow (Alacritty식) | advisor |
| alt screen | truncate 유지 (vim/htop SIGWINCH 재그림이 표준) | advisor / xterm 표준 |
| wrap flag 위치 | 행 단위 `RowFlags` (Cell 외부, Grid + scrollback row 양쪽) | architecture.md §8 |
| trim 정책 | 비-WRAPPED는 trailing default cell trim 후 join. WRAPPED row와 cursor 있는 row는 trim 금지 | advisor |
| cursor 매핑 | reflow 전 `(logical_line_idx, col_offset_in_line)` 저장 → 후 새 grid에서 같은 위치 재계산 | advisor |
| WIDE 경계 | 새 cols 마지막 1칸이고 다음 글자 width=2면 그 셀은 빈 default + WIDE를 다음 row로 (분할 금지) | xterm/iTerm 표준 |
| resize 폭주 | 매번 즉시 reflow. 추후 느리면 debounce 추가 | 사용자 결정 |
| evict | reflow 후 cap 적용. 오래된 것부터 drop. 스크롤백 끝(최신)은 안전 | 사용자 결정 |

### 정책 D 변경 사항 없음
정책 D(cursor)는 그대로. 정책 변경은 **"Reflow: MVP 미지원 → 지원"** 한 줄.

## 2. 자료구조 변경

### 2.1 `RowFlags` 신설

```rust
// grid/mod.rs
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct RowFlags: u8 {
        /// 이 row는 다음 row로 이어진다 (autowrap에 의한 wrap line continuation).
        const WRAPPED = 1 << 0;
    }
}
```

### 2.2 `Grid`에 `row_flags` 추가

```rust
struct Grid {
    cells: Vec<Cell>,
    row_flags: Vec<RowFlags>,  // len = rows
    cols: usize,
    rows: usize,
}
```

`resize` / `scroll_up` / `scroll_down` / `clear` 모두 row_flags 같이 다룸.

**`scroll_up` / `scroll_down` 패턴 (한 줄)**:

```rust
// scroll_up
for r in top..(bottom - n) {
    // cells shift는 기존 그대로
    self.row_flags[r] = self.row_flags[r + n];
}
for r in (bottom - n)..bottom {
    self.row_flags[r] = RowFlags::empty();
}
```

`scroll_down`도 대칭. row_flags 시프트 누락이 reflow 정확성 가장 흔한 회귀.

**alt grid의 row_flags 정책**: alt도 print 호출이 있으므로 마킹 자체는 일어남(자료구조 통일). 그러나 alt는 reflow에 안 들어감 → flag는 사실상 dead. resize 시 alt grid는 truncate만. row_flags도 truncate/extend. 정책 일관성: **자료구조는 같지만 alt의 flag는 소비처 없음을 인지**.

### 2.3 scrollback row 자료형 변경

```rust
// grid/mod.rs
#[derive(Debug, Clone)]
pub(crate) struct ScrollbackRow {
    pub cells: Vec<Cell>,
    pub flags: RowFlags,
}

// Term
scrollback: VecDeque<ScrollbackRow>,
```

`Term::cell`의 외부 시그니처는 그대로 유지 (read 측에서 `.cells` 접근).

### 2.4 `SCROLLBACK_CAP`

10,000 그대로. reflow 후 적용.

## 3. WRAPPED 마킹 / 클리어 규칙

### 3.1 마킹 시점

`Term::print` (grid/mod.rs L408~) 의 `if self.cursor.col + w > self.cols() { self.newline(); ... }` 분기. **`newline()` 호출 직전, 현재 row를 WRAPPED로 마크.**

```rust
if self.cursor.col + w > self.cols() {
    let row = self.cursor.row;
    self.grid_mut().row_flags[row].insert(RowFlags::WRAPPED);
    self.newline();
    self.cursor.col = 0;
}
```

순서가 중요:

1. 마킹 → 2. newline → 3. (newline 안에서 필요 시 scrollback push) → 4. (scroll_up 시 cells & row_flags shift).

**경우 1**: cursor가 마지막 row 아닌 일반 row. newline은 `cursor.row += 1`만. WRAPPED은 마킹된 row(원래 cursor row)에 그대로. ✅

**경우 2**: cursor가 마지막 row(scroll_bottom-1). newline이 scroll_up 트리거. scroll_up 내부에서 row_flags 시프트 → 마킹된 row가 위로 한 칸. push되는 row[0]이 main grid의 row 0이고, push 코드는 `ScrollbackRow { cells: row[0].clone(), flags: row_flags[0] }`로 작성 → row 0의 flag가 scrollback으로 함께 이동. ✅

`Term::newline`의 scrollback push 코드 패치:
```rust
let cols = self.main.cols;
let row_cells: Vec<Cell> = self.main.cells[..cols].to_vec();
let row_flag = self.main.row_flags[0];
self.scrollback.push_back(ScrollbackRow { cells: row_cells, flags: row_flag });
```

### 3.2 클리어 시점 (reflow 정확성의 핵심)

다음 동작이 row를 (부분이든 전체든) **덮어쓰면** WRAPPED 클리어:

| 동작 | 영향 row | 클리어 조건 |
|---|---|---|
| `Term::carriage_return` 후 `print` | cursor row | print가 col 0부터 다시 쓰는 상황. **다만 print 자체로는 WRAPPED을 안 건드림** — overflow로 다시 wrap되면 print의 마킹 분기가 다시 set. CR만으로는 클리어 X. |
| `Term::erase_line(0/1/2)` | cursor row | 항상 클리어 (line 일부라도 지워졌으면 wrap continuation 의미 잃음) |
| `Term::erase_display(0)` | cursor row 부분 + 그 아래 행 | cursor row와 아래 모든 행 클리어 |
| `Term::erase_display(1)` | cursor row 위 + cursor row 부분 | cursor row 위 모든 행 + cursor row 클리어 |
| `Term::erase_display(2/3)` | 전체 grid | 모든 행 클리어 |
| `Grid::scroll_up(top, bottom, n)` | top..bottom 영역 | shift된 row의 flag 같이 shift, **bottom-n..bottom 새 빈 행 클리어** |
| `Grid::scroll_down(top, bottom, n)` | top..bottom 영역 | shift된 row의 flag 같이 shift, **top..top+n 새 빈 행 클리어** |
| (M11) IL / DL | 영역 | 같은 패턴 |
| `Term::switch_alt_screen` | alt 진입 시 alt grid | `for c ... = default; for f ... = empty` |

이게 가장 흔한 reflow 버그라고 advisor가 지적. 케이스 누락 시 reflow가 과거 wrap을 살리거나 끊긴 줄을 합치는 hallucination 발생.

### 3.3 명시적 비클리어 사례

- 일반 `print`로 WRAPPED row 위에 새 글자 쓰기: 클리어 X. 그 글자가 wrap continuation 일부일 수 있음. 다만 보통 cursor 위치가 그 row 바깥에 있어 일어나지 않음. 실제로는 EL/ED가 항상 선행함. 안전한 default = 클리어 X.
- `cursor_up/down/left/right` 같은 cursor 이동: 클리어 X.
- SGR 변경: 클리어 X.

### 3.4 Edge case: `\r` only 후 짧게 덮어쓰기

shell readline 일부 환경에서 `\r` (CR) 후 EL 없이 새 글자를 짧게 덮어쓰는 패턴이 있을 수 있음. 이 경우 row의 WRAPPED은 stale 상태로 남음.

- **표준 환경(zsh/bash + 색 prompt)**: `\r\x1b[K` 또는 `\x1b[2K\r` 패턴이 일반적 → erase_line이 클리어 처리.
- **edge case**: 사용자 직접 `printf '\rhi'` 같이 보내면 stale WRAPPED 유지 가능.

본 마일스톤에서는 **표준 환경 정확성 보장**까지만. edge case는 시각상 문제 발생 시 별 패치(다음 print가 row 0..cursor.col을 덮어쓸 때 cursor가 라인 끝까지 안 갔으면 클리어)로 한정 추가. 명시: 본 단계 미반영.

## 4. Reflow 알고리즘

### 4.1 logical line 단위 처리

```
입력: scrollback rows (각 ScrollbackRow) + main grid rows (cells + row_flags)
       + (cursor.row, cursor.col)
       + 현재 cols, 새 cols, 새 rows

단계 A — 평탄화: scrollback + main → flat sequence of (cells: Vec<Cell>, flags)
       cursor의 absolute row (scrollback.len() + cursor.row) 기록

단계 B — logical line 분할:
       row 시퀀스를 WRAPPED 끝나는 지점으로 logical line 단위로 묶음
       LogicalLine { cells: Vec<Cell> (concat), cursor_offset: Option<usize> }
       cells는 trim 정책으로 trailing default 제거 (단 cursor 있는 line 또는 line 자체가 빈 line이 아닌 마지막 line이면 보존 정책 결정 — 아래 §4.4)

단계 C — re-wrap:
       각 LogicalLine을 새 cols로 wrap. WIDE 경계 규칙 적용 (§4.3).
       wrap 결과: Vec<NewRow { cells, flags }>
       cursor가 있는 logical line이면 cursor의 새 (row, col) 산출.

단계 D — partition:
       전체 NewRow 시퀀스 길이 = total. 새 main grid는 마지막 new_rows 행.
       나머지는 scrollback (앞부분). cursor가 main 영역에 있어야 함 — 없으면 §4.5.
       SCROLLBACK_CAP 초과분은 front drop.

단계 E — write back:
       new main = NewRow[scrollback_len_new..scrollback_len_new + new_rows]
       new scrollback = NewRow[..scrollback_len_new] (각각 ScrollbackRow로)
       빈 행 부족 시 default + RowFlags::empty 패딩.
       cursor.row/col 갱신, view_offset clamp.
```

### 4.2 trim 정책

logical line의 cells를 만들 때 (§B):

- 라인 마지막 row가 **WRAPPED** 면: 절대 trim 금지. WRAPPED row는 끝까지 유효 cell이 채워져 있음 (또는 default cell이 의미 있는 위치 holder).
- 라인 마지막 row가 **non-WRAPPED** 이고 cursor가 그 row에 없으면: trailing default cell trim. (default cell = `Cell::default()` PartialEq)
- 라인 마지막 row가 **non-WRAPPED** 이고 cursor가 그 row에 있으면: cursor.col 까지의 default는 보존, cursor.col 이후는 trim. (cursor가 빈 칸 위에 있을 수 있음 — readline edit 도중)

WIDE_CONT은 default가 아니므로 자동 보존.

### 4.3 WIDE 경계 규칙

re-wrap 시 한 글자씩 cursor 진행:

```
현재 출력 col, 글자 width w (WIDE면 2):
  if cursor_col + w > new_cols:
      if w == 2 and cursor_col == new_cols - 1:
          # 마지막 1칸 비우고 다음 row로
          현재 row의 cursor_col에 default cell padding (이미 default일 가능성 높음)
          현재 row의 flags.WRAPPED set
          새 row 시작
      else:
          현재 row의 flags.WRAPPED set
          새 row 시작
  현재 row에 글자 쓰기 (w==2면 WIDE + WIDE_CONT)
  cursor_col += w
```

WIDE_CONT 셀이 단독으로 row 시작에 오면 안 됨 — 위 규칙으로 보장.

### 4.4 빈 라인 처리

scrollback 또는 main에 default cell만으로 채워진 row가 연속 있을 수 있음 (`echo` 빈 줄 등). 이 경우:
- 각 빈 row는 자체로 logical line (WRAPPED 아닌). re-wrap 시 빈 logical line = 빈 NewRow (default cells, no flag).
- 빈 logical line은 그대로 보존. zsh prompt 위 빈 줄들이 그대로 보이는 게 자연스러움.

### 4.5 cursor 매핑 산식

cursor를 안전하게 추적하려면 logical line 분할 단계(§B)에서 cursor의 **logical line index + offset**을 같이 기록한다.

```rust
// 평탄화 단계에서:
let cursor_global_row = scrollback.len() + cursor.row;
// scrollback row[i].cells.len()과 main row[r].cols는 다를 수 있음.
// 통일: 각 row를 평탄화 시 항상 source row의 cells 그대로 concat.
//       cursor.col은 main grid 기준이라 main row의 cols(현재 cols)에서 valid.

// logical line 분할:
//   cursor가 속한 logical_line_idx = L
//   cursor의 logical_offset = (L 시작 row부터 cursor row 직전 row들의 cells.len() 합)
//                            + cursor.col
//   ※ trim 정책으로 trailing default를 제거할 때, cursor가 그 row에 있으면 trim 안 함 (§4.2)

// re-wrap 후:
//   line L가 NewRow N개로 wrap됨. 각 NewRow 시작 시점의 누적 logical_offset를 alongside 기록.
//   cursor 위치 = NewRow 시퀀스 중 (logical_offset이 [start, end) 안에 들어가는) row 하나
//   new_cursor.row = (해당 NewRow의 partition 후 main_row index)
//   new_cursor.col = logical_offset - 그 NewRow start
//
// WIDE 보정: re-wrap 시 WIDE 글자가 다음 row로 밀리면 빈 default가 padding됨.
//            logical_offset에서 그 padding 셀 1개도 카운트해야 (단, padding cell은 logical line cells에 들어가야 함 → 평탄화에 이미 포함되어 있음. trim에서도 보존됨).
//            결국 cell 인덱스 단위 카운트로 정합 자동 보장.
```

핵심 invariant: **logical_offset = (해당 logical line의 cells slice에서 cursor 위치까지의 cell 인덱스)**. WIDE는 한 글자가 2 cells (WIDE + WIDE_CONT)이므로 인덱스 단위로 셈 → 자연 보정.

`cursor.col == cols` (eager wrap pending) 상태: print의 다음 글자가 들어올 때 wrap되는 미정 상태. 이 시점 cursor.col은 cols와 같음 → logical_offset도 그 row의 cells.len()과 같음 = logical line의 마지막 +1. 새 cols로 wrap하면 logical line 마지막 row의 끝 +1 위치, 즉 다음 row 0. 새 cursor.row = (logical line 끝 row의 다음), col = 0. 단 그 cursor 위치가 main 영역 바깥이면 partition을 cursor anchor로 강제(아래).

### 4.6 partition 결정

reflow 후 NewRow 시퀀스 길이 = total. 새 main grid는 마지막 new_rows.

- `let main_start = total.saturating_sub(new_rows);`
- `cursor_new_row = (cursor가 든 NewRow의 절대 인덱스) - main_start`
- 단, cursor가 main 영역 바깥(절대 인덱스 < main_start)이면 partition을 cursor-anchored로 보정:
  - `main_start = (cursor 절대 인덱스).saturating_sub(new_rows - 1)`
  - 즉 cursor를 main 마지막 row까지 끌어내림. 이게 더 일반적이지만 보통은 cursor가 logical line 끝에 있어 첫 식이 충족.
- `scrollback_new = NewRow[..main_start]`, `main_new = NewRow[main_start..main_start + new_rows]`.
- main_new가 부족하면 default + RowFlags::empty padding (위쪽 또는 아래쪽? **아래쪽 padding** — cursor가 위에 있는 게 자연스러움. 단 보통은 main_start 보정으로 부족하지 않음).
- SCROLLBACK_CAP 초과: `while scrollback_new.len() > CAP { pop_front }`.
- view_offset clamp: `view_offset.min(scrollback_new.len())`.

### 4.7 빈 trailing rows in main

prompt가 main 마지막 줄에 있고 그 위 N개 row가 빈 default면, 평탄화 시 N개 빈 logical line으로 들어감. re-wrap 시에도 N개 빈 NewRow. partition 후 위쪽이 main에 자리 잡으면 main 위쪽이 비어 보이는 게 자연스러움. **scrollback에 빈 줄이 push되지 않게** — partition 자체로 처리됨 (logical line이 빈 line이라도 NewRow 1개 차지하므로 위에서부터 채움).

대안: 빈 logical line을 생략하고 prompt만 main 채우는 것도 가능하나 줄 간격이 갑자기 바뀌어 어색. **현재 방식 유지**.

### 4.8 scrollback view 중 reflow

사용자가 `view_offset > 0` 상태에서 창을 리사이즈할 수 있음. 이 경우 보고 있던 logical line이 reflow 후 어디로 가는지 정확히 추적해 view_offset을 재계산하면 UX가 매끄럽지만 비용이 큼.

**본 마일스톤 결정 (M17-4 시각 검증 후 변경)**: resize 시 `view_offset = 0` (snap to bottom).
- 시각 검증에서 단순 clamp는 reflow 후 view 위치 매핑이 어긋나 콘텐츠가 어색하게 사라지는 사례 확인됨.
- snap to bottom은 resize라는 명시적 사용자 액션의 일부로 view reset이 자연스럽고 일관됨.
- 정밀 매핑(보고 있던 logical line 위치 보존)은 향후 별 패치 후보. §8 알려진 한계에 유지.

## 5. alt screen 처리

`switch_alt_screen` 후 use_alt = true 동안의 resize는 **alt grid만 truncate** (현재 동작 그대로). main grid + scrollback도 같이 reflow하되 — alt에 있는 동안에도 main이 안 보이지만 정합 유지를 위해 **둘 다 reflow** 권장. 정책 결정: **alt 진입 시점에 main의 reflow는 보류 → alt 종료 시점에 size mismatch면 reflow 한 번**. 이게 단순. 다만 vim 사용 중 사이즈 변경되고 vim 종료 → reflow 발생 = OK.

본 마일스톤은 더 단순하게: **alt 모드에선 resize 시 alt만 truncate. main은 사이즈 그대로 유지(이전 cols/rows에서 frozen)**. alt 종료 시 main을 새 cols/rows로 reflow.

```rust
fn resize(&mut self, cols, rows) {
    if self.use_alt {
        self.alt.resize_truncate(cols, rows);
        // main은 frozen. 단 cursor row/col clamp는 안 함 — alt 끝나면 saved cursor 복원.
    } else {
        self.reflow(cols, rows);  // main + scrollback 전체
    }
    self.scroll_top = 0;
    self.scroll_bottom = rows;
    self.view_offset = ...;
}

fn switch_alt_screen(&mut self, on) {
    ...
    if !on {
        // alt → main 복귀
        // 순서가 중요: cursor를 saved_main_cursor로 먼저 복원 → 그 후 main 사이즈 mismatch 검사 → reflow.
        // reflow가 cursor를 reflow된 main 좌표계로 매핑해야 정확.
        self.use_alt = false;
        self.cursor = self.saved_main_cursor;
        let (cur_c, cur_r) = (current cols, rows);
        if main grid size != (cur_c, cur_r) {
            // cursor.row/col이 frozen 사이즈 기준 valid → reflow 입력으로 valid.
            self.reflow_main_only(cur_c, cur_r);
        }
    }
}
```

리스크: alt 모드 중 main이 frozen이면 main의 PTY size는 이미 새 사이즈로 변경됨 — 단 alt 모드 중 main에 그려질 일 없으므로 OK.

대안 고려: 매 resize에서 main도 reflow. 더 단순하지만 alt 모드 중 불필요한 reflow 비용. **본 마일스톤은 위의 frozen 방식 채택**.

**scroll region**: 현재 `Term::resize`가 항상 `scroll_top = 0; scroll_bottom = rows`로 reset. alt 모드 frozen 정책이라도 새 rows에 맞게 reset(alt grid 기준). vim 등이 SIGWINCH 받고 자체 scroll region 재설정 → reset 손실 없음. **정책 유지: resize 시 항상 reset**.

**Renderer 영향 없음**: Renderer는 `Term::cell(row, col)`만 호출. `Term::cell`의 외부 시그니처(인자, 반환) 변경 X → render 코드 변경 X. 회귀 위험 차단.

## 5.A API 시그니처 / 모듈 위치

```rust
// grid/mod.rs (Term의 메서드)
impl Term {
    /// resize의 진입점. use_alt에 따라 분기.
    pub fn resize(&mut self, cols: usize, rows: usize);

    /// main 전용 reflow (M17-3). scrollback은 truncate-on-read 유지.
    fn reflow_main_only(&mut self, new_cols: usize, new_rows: usize);

    /// scrollback + main 통합 reflow (M17-4 이후).
    fn reflow(&mut self, new_cols: usize, new_rows: usize);
}
```

자유 함수 vs 메서드: cells/scrollback/cursor를 모두 mutating해야 하므로 `&mut self` 메서드. 단 reflow 내부의 **logical line 처리 / re-wrap 알고리즘은 자유 함수**로 분리하면 헤드리스 unit test가 쉬움:

```rust
// grid/mod.rs (private free fn)
fn rewrap_lines(
    rows: &[(Vec<Cell>, RowFlags)],   // input rows (scrollback + main 평탄화)
    cursor_global_row: usize,
    cursor_col: usize,
    new_cols: usize,
) -> RewrapResult {
    // RewrapResult { new_rows: Vec<(Vec<Cell>, RowFlags)>, cursor_new_global_row: usize, cursor_new_col: usize }
}
```

이러면 Term 의존 없이 case별 unit test 작성 가능. 인터페이스는 implementation 단계에서 미세 조정.

## 5.B 새 buffer + swap 패턴 (panic safety)

reflow 알고리즘이 cells/row_flags/scrollback을 **in-place mutate** 하면 도중 panic 시 grid invariant(`cells.len() == cols * rows`, `row_flags.len() == rows`)가 깨질 수 있음. 따라서:

- 새 cells / row_flags / scrollback을 **별도 buffer에 build → 끝에서 한 번에 std::mem::swap**.
- panic 시점이 build 중이면 본 grid는 그대로 남음 (panic propagation 후 process 종료지만, 다음 frame까지 부분 mutate된 grid 그리는 사고는 방지).
- 메모리 비용: 일시적으로 grid 두 벌 + scrollback 두 벌. 10k × 200 cols × ~16바이트 = 약 32MB 추가 일시 사용 → 학습 프로젝트에서 수용.

## 5.C Invariant / panic 정책

reflow 진출입 시점에 다음 invariant 유지:

| Invariant | 설명 |
|---|---|
| I1 | `main.cells.len() == main.cols * main.rows` |
| I2 | `main.row_flags.len() == main.rows` |
| I3 | `alt.cells.len() == alt.cols * alt.rows` |
| I4 | `alt.row_flags.len() == alt.rows` |
| I5 | `cursor.row < rows && cursor.col <= cols` (col == cols는 eager wrap pending) |
| I6 | scrollback의 각 row.cells.len() ≤ 어떤 max (현재 무제한 — push 시점 cols 그대로). reflow 후엔 모든 row.cells.len() == new_cols **또는** ≤ new_cols (마지막 logical line의 마지막 row가 trim되면 짧을 수 있음). 단 **WRAPPED row는 항상 new_cols 만큼**. |
| I7 | `view_offset ≤ scrollback.len()` |

resize 함수 시작/끝 모두에서 debug_assert로 체크. 핫패스 unwrap 금지 (architecture.md §7).

### M17-1 — `RowFlags` 자료구조 도입 (refactor only, 기능 변화 0)

- `grid/mod.rs`에 `RowFlags` 추가.
- `Grid` 구조체에 `row_flags: Vec<RowFlags>` 필드. `new`/`resize`/`scroll_up`/`scroll_down`에서 동기 관리.
- `ScrollbackRow { cells, flags }` 신설. `Term::scrollback` 자료형 변경.
- `Term::cell` 등 read 경로 수정 (`.cells` 접근).
- 기존 단위 테스트 모두 그대로 통과해야 함.

검증: `cargo build --release && cargo test`. 시각 검증 1회 (vim/ls 그대로).

### M17-2 — WRAPPED 마킹 / 클리어 + 단위 테스트

- `Term::print` overflow 분기에 마킹 추가.
- §3.2 표대로 클리어 추가:
    - `erase_line` 모든 모드 → cursor row flags clear.
    - `erase_display` 모드별 영역 flags clear.
    - `Grid::scroll_up`/`scroll_down`에서 shift된 영역 flag도 같이 shift, 새로 비는 영역 flag clear.
    - `switch_alt_screen` alt 진입 시 alt grid flags clear.
- 단위 테스트:
    - 80 cols에서 100자 print → row 0 WRAPPED set 확인, row 1 not WRAPPED.
    - `print` 후 `erase_line(2)` → flag cleared.
    - `scroll_up_n(1)` → 첫 행 flag가 위로 이동, 마지막 행 flag empty.

### M17-3 — main grid reflow + cursor 매핑

**M17-3의 역할 명시**: `rewrap_lines` 알고리즘 자체를 검증하는 단계. 입력 = main grid의 rows만(scrollback empty 가정). 단계 4의 검증 인프라 + 알고리즘 정확성을 확보한 뒤, 다음 단계에서 입력만 scrollback + main 평탄화로 확장.

- `Term::resize` 분기. `use_alt`이면 truncate 유지, 아니면 `reflow_main_only(cols, rows)`.
- `reflow_main_only`: main grid의 rows만 평탄화 → `rewrap_lines` → 새 main 채움. scrollback은 건드리지 않음 (위 평탄화 입력에 안 들어감 → main 안에서 시작/끝나는 logical line만 처리됨). M17-4에서 scrollback도 평탄화 입력에 합쳐 통합.
- cursor 매핑: §4.5.
- 단위 테스트:
    - 좁은 cols → 넓은 cols: WRAPPED row 합쳐지고 wrap 풀리는지.
    - 넓은 cols → 좁은 cols: 긴 row 다시 wrap, WRAPPED 마킹.
    - cursor가 wrapped line 중간에 있을 때 cursor 위치 정확.
    - `cursor.col == cols` (eager wrap pending) 상태에서 reflow 후 cursor가 다음 row 0.

검증: `vim`/`top`은 alt이라 영향 없음. shell prompt에서 긴 줄 출력 후 리사이즈 시각 확인.

### M17-4 — scrollback 통합 reflow

- `reflow` 함수가 scrollback + main을 통합 처리.
- evict: 새 logical line 합산 후 SCROLLBACK_CAP 초과분 front drop.
- view_offset 보정: scrollback 길이 변경 시 offset clamp. 정밀 매핑(같은 logical line 위치 유지)은 1차 cut 보류 — `view_offset = view_offset.min(new_scrollback_len)` 정도.
- 단위 테스트:
    - scrollback에 wrap된 긴 줄들이 있을 때 폭 늘리면 합쳐지는지.
    - cap 초과 시 evict 발생, 최신 안전 보존.

검증: `cat large.txt` 후 창 가로 늘려보기.

### M17-5 — WIDE 경계 + 시각 검증 + architecture.md 패치

- WIDE 경계 (§4.3) 단위 테스트:
    - 4 cols, "한a한" → row 0: "한a", flags WRAPPED, row 1: "한".
    - 3 cols, "한a한" → row 0: "한a", row 1: "한". (3 cols에서도 동일)
    - 3 cols, "한한" → row 0: "한", row 1: "한". (마지막 1칸 빈 default + 다음 row)
- 시각 검증 시나리오 (4종, 케이스별로 묶음):

  | # | 명령 | 입력 | 액션 | 기대 결과 |
  |---|---|---|---|---|
  | V1 | 짧은 줄 다수 | `seq 1 30` | 좁히기 → 다시 넓히기 | wrap 발생 안 함, 변화 없음 |
  | V2 | 긴 한 줄 | `printf 'a%.0s' {1..200}; echo` | 좁히기(60 cols) | 약 4 row로 wrap, WRAPPED 마킹. 다시 넓혀(200 cols) → 한 줄로 합쳐짐 |
  | V3 | WIDE | `printf '가나다%.0s' {1..50}; echo` | 좁히기(8 cols) | WIDE가 2 cols 차지 보존, 분할 없음. 마지막 1칸이면 빈 default + 다음 row |
  | V4 | scrollback 가득 | `seq 1 12000` (cap 10k 초과) | 좁히기 후 scroll up | evict로 오래된 줄 사라지지만 최근 줄 보존, view 작은 점프 |
  | V5 | alt screen | `vim test.txt` 진입 → 리사이즈 → `:q` | vim 진입 중 alt만 redraw. `:q` 후 main reflow 트리거, prompt 정합 |
  | V6 | cursor at edge | shell prompt + 글자 입력 (eager wrap pending) → 리사이즈 | cursor row/col 정확 유지 |

- architecture.md §8 표 갱신: "메서드 시그니처 미생성" → "구현 (M17, 2026-05-08)".
- architecture.md 부록 A "Reflow MVP 미지원" 줄 갱신.
- 정책 표 (architecture.md / overview.md) "Reflow: 지원" 으로.

## 6.A 테스트 인프라 위치

- `src/grid/mod.rs` 끝에 `#[cfg(test)] mod tests` 추가하거나, 분리해서 `src/grid/tests.rs` (현재는 단일 mod.rs라 첫 모드).
- `rewrap_lines` 자유 함수는 `pub(crate)` 노출하여 같은 crate `tests` mod에서 직접 호출.
- 헬퍼: `mk_row(s: &str, wrapped: bool) -> (Vec<Cell>, RowFlags)` — ASCII만 받아 quick build. WIDE 테스트는 별도 헬퍼 `mk_wide_row(...)`.
- `cargo test --lib`로 빠르게 회전.

## 6.B 로깅

- `Term::reflow` 진입/종료 시 `log::debug!("reflow: {}x{} → {}x{}, scrollback {} → {}", old_cols, old_rows, new_cols, new_rows, old_sb_len, new_sb_len)`.
- WRAPPED 마킹 자체는 log 안 함 (핫패스 noise).
- 비정상 invariant 위반 감지 시 `log::error!` + `debug_assert!`.

## 6.C 반복 wrap / 극단 cols

- N번 wrap (logical line 매우 김): 알고리즘은 단순 loop라 자연 처리.
- **1 cols / 2 cols 안전망**: cols=1에서 WIDE 글자 들어오면 영구 wrap 시도가 무한 루프 위험. 안전망: `if new_cols < 2 && glyph w == 2 { skip the glyph or substitute '?' }`. 본 마일스톤은 **WIDE 글자 skip + log::warn** 정책.

## 7. 단위 테스트 케이스 체크리스트

| # | 케이스 | 단계 |
|---|---|---|
| T1 | print overflow → WRAPPED 마킹 | M17-2 |
| T2 | EL 후 WRAPPED 클리어 | M17-2 |
| T3 | scroll_up이 flag도 shift | M17-2 |
| T4 | alt 진입 시 alt flag clear | M17-2 |
| T5 | 좁→넓: wrapped logical line 합쳐짐 | M17-3 |
| T6 | 넓→좁: 긴 row 재wrap + 마킹 | M17-3 |
| T7 | cursor가 wrapped middle일 때 (row, col) 정확 | M17-3 |
| T8 | scrollback 가득 + 좁→넓 evict 동반 | M17-4 |
| T9 | view_offset clamp | M17-4 |
| T10 | 빈 줄 다수 보존 | M17-4 |
| T11 | WIDE 경계: 마지막 1칸 + WIDE 다음 | M17-5 |
| T12 | 1 cols 극단 (WIDE는 절대 못 들어감 — 정책 결정) | M17-5 |
| T13 | `cursor.col == cols` (eager wrap pending) 상태 reflow | M17-3 |
| T14 | I1~I7 invariant — reflow 진입/종료 debug_assert | M17-1, M17-3 |
| T15 | alt 모드 frozen + 종료 시 reflow + cursor restore 순서 | M17-3 |

T12 정책: 1 cols일 때 WIDE 글자는? 표준은 빈 행을 마킹하거나 표시 안 함. **본 구현은 WIDE 글자를 그 자리에 통째로 skip + 다음 row의 col 0에 배치 시도**. 또 1 col이면 WIDE는 결국 무한 wrap 필요. 1 cols / 2 cols는 edge case로 panic 없이 동작만 보장 (visual 깨짐은 수용).

## 8. 알려진 한계 / 보류

- **Pending wrap (xterm "last column" semantic)**: 현재 eager wrap. 본 마일스톤에서 안 다룸. 별 milestone (§16 cleanup 후보).
- **DECAWM (autowrap mode)**: 현재 미추적. tput rmam 환경에선 마킹이 안 일어남 → 정상.
- **selection 좌표 안정성**: M12 미진입이라 본 마일스톤 영향 없음. M12 진입 시 AbsLine과 reflow 좌표 맵 상호작용 재검토.
- **부분 스크롤 영역(scroll region 비기본) 중 reflow**: 현재 resize에서 scroll region을 0..rows로 reset함. 그대로 유지.
- **performance**: 매 Resized마다 reflow. 10k scrollback × max cols × 5 phase = 약 수백만 cell 연산. 학습 프로젝트 범위에서 체감 OK 가정. 느리면 frame coalesce 추가 (resize event를 about_to_wait에서 한 번만 적용).
- **scrollback view 정밀 매핑**: §4.8. 본 마일스톤은 단순 clamp. 보고 있던 logical line 위치 보존은 향후 별 패치.

## 9. cross-cut

- M11 (VT 인프라 보강) — IL/DL 추가 시 §3.2 표에 같이 등록 필요.
- M12 selection — AbsLine 카운터 정의에 영향. reflow는 AbsLine을 깨트리지 않게 설계 (logical line 단위라 자연 보존, 다만 wrap 포지션 변경 시 selection을 cancel 정책).
- M16 Cmd+F find — selection 인프라 공유.

## 10. 진입 직전 docs.rs 재확인

- bitflags 2.x — 이미 사용 중 (Attrs). 그대로.
- VecDeque 동작 — 이미 사용 중.
- 새 외부 dep 없음.

---

**다음 작업**: M17-1 진입.
