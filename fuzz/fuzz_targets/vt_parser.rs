//! cargo-fuzz target: vte::Parser + pj001-core Term에 임의 바이트 시퀀스를 흘려
//! panic이 발생하지 않는지 확인. corpus seed는 `fuzz/corpus/vt_parser/`.
//!
//! 실행:
//!   cd pj001
//!   cargo +nightly fuzz run vt_parser
//!
//! 발견된 crash는 `fuzz/artifacts/vt_parser/`에 자동 저장 → minimized input을
//! corpus에 추가 후 dep-free `crates/pj001-core/src/vt/perform.rs::fuzz_corpus_*` test로
//! 회귀 방지.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pj001_core::grid::Term;
use pj001_core::vt::TermPerform;
use vte::Parser;

fuzz_target!(|data: &[u8]| {
    // 표준 80x24 + max scrollback (cap 10_000)으로 다양한 grid state 노출.
    let mut term = Term::new(80, 24);
    let mut parser = Parser::new();
    let mut perform = TermPerform::new(&mut term);
    parser.advance(&mut perform, data);
    // invariant: rows/cols 변동 없음.
    assert_eq!(term.rows(), 24);
    assert_eq!(term.cols(), 80);
});
