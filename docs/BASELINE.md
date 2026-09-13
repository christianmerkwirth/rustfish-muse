# Baseline — v0.1 (2026-09-13)

Engine: deterministic first-legal-move search, debug build, Hash=32 Threads=1.
Stockfish: version 18, `UCI_LimitStrength` + `UCI_Elo`, Hash=32 Threads=1.

## Correctness

- `cargo test`: **32/32 pass**; `cargo clippy --all-targets -- -D warnings`
  clean; `cargo fmt --check` clean
- UCI compliance CLI: **59/59 pass**; pytest: **14/14 pass**
- Tactical suites verified independently: all 30 EPD `bm` lines provably mate

## Speed (`bench.py`, 200 seeded positions, `go depth 1`)

- Handshake (`uci` + `isready`): 0.31 ms
- Mean 0.12 ms/pos, p95 0.165 ms/pos → **8364.7 pos/s** (protocol overhead
  only; no real search yet)

## Tactics (`go movetime 100` per position)

- mate_in_1: **0/20**; mate_in_2: **0/10** — expected for a stub that plays
  the first legal move; the suites are proven solvable, so future search
  work must drive these up

## Matches vs Stockfish

- v0.1 calibration: **+0 =0 −10** vs Stockfish `UCI_Elo=1320` at 10+0.1
  (5 pairs, 12-line opening book, resign/draw adjudication).
  The stub loses everything, including to the weakest Stockfish setting —
  this is the floor every later milestone must rise above. The 2400
  campaign starts in v0.2.

## Raw artifacts

`tools/results/bench_20260913T185443Z.json`,
`tools/results/match_20260913T185555Z.json` (gitignored; rerun to reproduce).
