//! Block UI Phase 1 — Step 1 자료구조.
//!
//! OSC 133 A/B/C/D 경계에서 만들어지는 `Block`의 생명주기와 식별을 관리한다.
//! 본 단계는 모듈 정의만 + 단위 테스트. Term/Grid 통합은 Step 2 이후.
//! 설계 근거: `docs/block-ui-design.md` §3.

use std::time::Instant;

/// 세션 내 monotonic block 식별자. 재사용 없음.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct BlockId(pub u64);

/// command block 생명주기.
///
/// 전이도: `Prompt` → `Command` → `Running` → `Completed`
/// (`Abandoned`는 어느 단계에서든 진입 가능).
#[derive(Clone, Debug, PartialEq)]
pub enum BlockState {
    /// `OSC 133;A` 수신, `B` 미수신.
    Prompt,
    /// `OSC 133;B` 수신, `C` 미수신.
    Command,
    /// `OSC 133;C` 수신, `D` 미수신.
    Running,
    /// `OSC 133;D[;exit]` 수신. `exit_code = None`이면 D는 받았지만 코드 unknown.
    Completed { exit_code: Option<i32> },
    /// 비정상 종료 — `reason`으로 사유 구분.
    Abandoned { reason: AbandonReason },
}

/// `Abandoned` 사유. user-visible은 통합되지만 디버깅/통계용 구분.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AbandonReason {
    /// alt screen 진입 (vim/htop 등).
    AltScreen,
    /// 다음 prompt `A` 수신, 직전 block이 D 미수신.
    NewPrompt,
    /// DECSTR (`CSI ! p`) 또는 RIS (`ESC c`) 수신.
    Reset,
    /// scrollback eviction으로 prompt_start_abs < oldest_kept_abs.
    /// 이 enum은 BlockStream에서 drop 직전 거치는 transitional state.
    Evicted,
}

/// 단일 명령 블록의 경계와 메타데이터.
///
/// 좌표는 모두 절대 행 — `Term.oldest_kept_abs + scrollback.len() + cursor.row` 기준.
/// Step 3에서 oldest_kept_abs가 도입된 후에야 절대 행이 안정. 본 step에서는 push API만.
#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub prompt_start_abs: u64,
    pub command_start_abs: Option<u64>,
    pub output_start_abs: Option<u64>,
    pub output_end_abs: Option<u64>,
    /// `B` 수신 시점. duration 측정 시작.
    pub started_at: Option<Instant>,
    /// `D` 수신 시점. duration 측정 끝.
    pub ended_at: Option<Instant>,
    pub state: BlockState,
}

/// 하나의 row에 붙는 block 경계 태그. 한 row에 여러 boundary가 같이 있을 수 있다
/// (예: 한 줄 prompt+command).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RowBlockTag {
    pub block_id: BlockId,
    pub kind: BlockBoundary,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockBoundary {
    PromptStart,
    CommandStart,
    OutputStart,
    OutputEnd,
}

/// append-only block 컬렉션. eviction은 oldest first.
#[derive(Default, Debug)]
pub struct BlockStream {
    blocks: Vec<Block>,
    next_id: u64,
}

impl BlockStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// 다음 BlockId를 할당하고 빈 Prompt 상태 Block을 push. 호출자는 prompt_start_abs를 채워서 반환된 BlockId로 후속 업데이트한다.
    pub fn start_prompt(&mut self, prompt_start_abs: u64) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("BlockId u64 overflow — 실용 한도 초과");
        self.blocks.push(Block {
            id,
            prompt_start_abs,
            command_start_abs: None,
            output_start_abs: None,
            output_end_abs: None,
            started_at: None,
            ended_at: None,
            state: BlockState::Prompt,
        });
        id
    }

    pub fn get(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn get_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// scrollback eviction 시 호출. `prompt_start_abs < oldest_kept_abs`인 block은
    /// drop. drop 직전 `Abandoned { Evicted }`로 마킹할 책임은 호출자에 있다 — 본 메서드는
    /// 단순 drop만 한다.
    pub fn drop_below(&mut self, oldest_kept_abs: u64) {
        self.blocks
            .retain(|b| b.prompt_start_abs >= oldest_kept_abs);
    }

    /// 마지막 block이 진행중(Prompt/Command/Running)이면 mutable 참조 반환. Completed/Abandoned면 None.
    pub fn active_mut(&mut self) -> Option<&mut Block> {
        let last = self.blocks.last_mut()?;
        match last.state {
            BlockState::Prompt | BlockState::Command | BlockState::Running => Some(last),
            _ => None,
        }
    }

    /// 진행중 block이 있으면 Abandoned로 전환. 없으면 no-op.
    pub fn abandon_active(&mut self, reason: AbandonReason) {
        if let Some(b) = self.active_mut() {
            b.state = BlockState::Abandoned { reason };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_prompt_allocates_monotonic_ids() {
        let mut stream = BlockStream::new();
        let a = stream.start_prompt(0);
        let b = stream.start_prompt(10);
        let c = stream.start_prompt(20);
        assert_eq!(a, BlockId(0));
        assert_eq!(b, BlockId(1));
        assert_eq!(c, BlockId(2));
        assert_eq!(stream.len(), 3);
    }

    #[test]
    fn get_and_get_mut_resolve_existing_ids() {
        let mut stream = BlockStream::new();
        let id = stream.start_prompt(42);
        assert_eq!(stream.get(id).map(|b| b.prompt_start_abs), Some(42));
        stream.get_mut(id).unwrap().state = BlockState::Command;
        assert_eq!(stream.get(id).unwrap().state, BlockState::Command);
        assert!(stream.get(BlockId(999)).is_none());
    }

    #[test]
    fn drop_below_removes_old_blocks_only() {
        let mut stream = BlockStream::new();
        stream.start_prompt(5);
        stream.start_prompt(15);
        stream.start_prompt(25);
        stream.drop_below(20);
        let remaining: Vec<u64> = stream.iter().map(|b| b.prompt_start_abs).collect();
        assert_eq!(remaining, vec![25]);
    }

    #[test]
    fn block_state_completed_carries_exit_code() {
        let mut stream = BlockStream::new();
        let id = stream.start_prompt(0);
        let block = stream.get_mut(id).unwrap();
        block.state = BlockState::Completed { exit_code: Some(0) };
        match stream.get(id).unwrap().state {
            BlockState::Completed { exit_code } => assert_eq!(exit_code, Some(0)),
            _ => panic!("expected Completed"),
        }
    }

    #[test]
    fn abandon_reasons_are_distinct() {
        assert_ne!(AbandonReason::AltScreen, AbandonReason::NewPrompt);
        assert_ne!(AbandonReason::Reset, AbandonReason::Evicted);
    }

    #[test]
    fn row_block_tag_equality_compares_id_and_kind() {
        let a = RowBlockTag {
            block_id: BlockId(7),
            kind: BlockBoundary::PromptStart,
        };
        let b = RowBlockTag {
            block_id: BlockId(7),
            kind: BlockBoundary::PromptStart,
        };
        let c = RowBlockTag {
            block_id: BlockId(7),
            kind: BlockBoundary::CommandStart,
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn block_id_is_ord_for_btreeset_use() {
        // M12 정합: BlockId가 BTreeSet/BTreeMap 키로 쓰일 때 Ord 필요.
        let mut ids = [BlockId(3), BlockId(1), BlockId(2)];
        ids.sort();
        assert_eq!(ids, [BlockId(1), BlockId(2), BlockId(3)]);
    }
}
