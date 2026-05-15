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

## 다음 단계 — cargo-fuzz 본격 도입

### 환경 준비
1. nightly toolchain 설치: `rustup install nightly`
2. cargo-fuzz 설치: `cargo install cargo-fuzz`
3. pj001/crates/pj001-core 디렉터리에서 `cargo fuzz init`

### fuzz target 분리
```
fuzz/
├── Cargo.toml
├── fuzz_targets/
│   ├── vt_parser.rs      # Term::new + vte::Parser advance
│   ├── osc_parse.rs      # OSC 8/133 specific
│   └── grid_resize.rs    # rewrap + scroll edge
└── corpus/
    ├── vt_parser/        # 위 corpus seeds + vttest 표준 fixture
    └── osc_parse/
```

### golden corpus
- `crates/pj001-core/src/vt/perform.rs`의 `fuzz_corpus_known_sequences_no_panic` 안
  35건을 `fuzz/corpus/vt_parser/seed_<n>.bin`으로 export.
- vttest 표준 (https://invisible-island.net/vttest/) 시퀀스 추가.
- xterm-256color terminfo escape 시퀀스 fixture.

### CI 통합
- nightly만 가능 — CI에 nightly job 별도. 또는 manual periodic run.
- `cargo fuzz run vt_parser -- -max_total_time=60` 60초 단위 quick smoke.

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
