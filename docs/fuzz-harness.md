# Fuzz harness — VT parser

작성일: 2026-05-15
관련 코드: `crates/pj001-core/src/vt/perform.rs#[cfg(test)] mod tests` —
`fuzz_random_bytes_no_panic`, `fuzz_corpus_known_sequences_no_panic`,
`fuzz_byte_by_byte_feed_no_panic`

## 1차 cut (현재)

- dep 없음 (cargo-fuzz/nightly 미사용).
- deterministic LCG로 random byte sequence 1000 iteration 흘려 vte::Parser + TermPerform
  panic 없는지 확인.
- known VT sequence corpus 35건 (ANSI/CSI/SGR/DEC modes/OSC 2-8-133/line drawing/
  ICH/DCH/long params/mouse SGR/DSR/DA) 정상 처리 확인.
- 1-byte-at-a-time 점진 feed로 incremental parser state machine 회귀 검증.
- 모두 `cargo test --quiet fuzz`로 실행.

## 2차 cut — cargo-fuzz skeleton 도입 (2026-05-15)

### 디렉터리 구조 (commit 추가)

```
pj001/
├── fuzz/
│   ├── Cargo.toml           # libfuzzer-sys + pj001-core path dep
│   ├── fuzz_targets/
│   │   └── vt_parser.rs     # Term + vte::Parser fuzz target
│   ├── corpus/
│   │   └── vt_parser/       # 10 seed bytes (clear/SGR/OSC 133/OSC 8/
│   │                        # DEC line drawing/mouse SGR/utf-8/invalid/
│   │                        # vim startup/DECSTBM+IND)
│   └── .gitignore           # target/ artifacts/ coverage/
```

`pj001/Cargo.toml`은 `workspace.exclude = ["fuzz"]`로 stable `cargo build`/`cargo test`
영향 차단.

### 실행
```bash
# 1회 설정
rustup install nightly
cargo install cargo-fuzz

# 실행 (60s smoke)
cd pj001
cargo +nightly fuzz run vt_parser -- -max_total_time=60

# 또는 무제한
cargo +nightly fuzz run vt_parser
```

### crash 발견 시 회귀 방지 path
1. `fuzz/artifacts/vt_parser/`에 minimized input 자동 저장.
2. 그 input을 `crates/pj001-core/src/vt/perform.rs::fuzz_corpus_known_sequences_no_panic`
   corpus 배열에 추가 → stable test로 회귀 잡음.

### CI 통합
- nightly만 가능 — CI에 nightly job 별도. 또는 manual periodic run.
- `cargo +nightly fuzz run vt_parser -- -max_total_time=60` 60초 단위 quick smoke.

### 향후 추가 target
- `osc_parse.rs` — OSC 8/133 specific (현재 vt_parser에 포함)
- `grid_resize.rs` — rewrap + scroll edge
- `term_input.rs` — Term::print에 utf-8 + Wide char 시퀀스

## 발견 시 회귀 방지

cargo-fuzz가 crash 발견하면:
1. minimized input을 `fuzz_corpus_known_sequences_no_panic` corpus에 추가
2. 1차 cut 테스트로 매번 regress 확인 가능

## 알려진 한계 (1차 cut)

- vte parser만 fuzz. PTY reader / IME / mouse reporting / shader pipeline은 미커버.
- 1000 iter는 짧음 (cargo-fuzz는 무제한). 단 CI 시간 제약 위해 1차 cut.
- coverage feedback 없음 — cargo-fuzz의 핵심 강점 미활용.

## 출처
- vte 0.15 — vt100 state machine. Paul Williams VT500 docs.
- cargo-fuzz: https://rust-fuzz.github.io/book/cargo-fuzz.html
- libfuzzer-sys
