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
`tools/results/match_20260913T185555Z.json` (4-game smoke),
`tools/results/match_20260913T185902Z.json` (10-game calibration)
(gitignored; rerun to reproduce).

---

# v0.2 real search (this milestone, numbers filling in)

Engine: negamax alpha-beta, iterative deepening, capture quiescence with
check evasions, PV/MVV-LVA/killer/history ordering, tapered PST eval.
Matches play the release build; protocol tests use either build.

## Correctness

- `cargo test`: **45/45 pass** (adds perft startpos 1–4 + kiwipete 1–3,
  mate-in-1 finding, info-per-iteration, movetime respect, determinism,
  sequential-position regression); clippy `-D warnings` clean, fmt clean
- UCI compliance CLI: **60/60 pass** (adds `sequential_searches`: back-to-back
  searches on one engine must each see their own board — this check fails on
  the pre-fix loop, which dropped `position` commands arriving right after
  `bestmove` and played illegal stale moves in real games); pytest: 15/15
- Tactical suites re-verified (unchanged data)

## Speed and tactics (`bench.py`)

- Debug: 199 positions, handshake 0.43 ms, mean 4.17 ms/pos, p95 7.90 ms/pos,
  239.9 pos/s at `go depth 1`
- Release: handshake 0.31 ms, mean 0.79 ms/pos, p95 3.07 ms/pos, 1269.5 pos/s
- mate_in_1: **20/20 (100%)**; mate_in_2: **7/10 (70%)** at 200 ms/position
  (same on both builds) — clears the "mate-in-1 mostly solved" gate

## Matches vs Stockfish (release build, 10+0.1)

- Framework bug found by the gate match: the command loop dropped `position`
  commands arriving after a finished-but-unreaped search, so the engine
  replayed stale moves (0–50 with illegal-move forfeits). Fixed in this
  milestone, regression-tested both directions (new check fails pre-fix with
  a repeated stale move, passes post-fix), rerun below.
- Calibration vs Stockfish `UCI_Elo=1320`, 100 games (2×25 pairs, 12-line
  book, resign/draw adjudication): **+61 =0 −39 (61%)**, Elo **+77.7±69.8**,
  LOS **98.8%**, zero illegal-move terminations.
- Honest gate read: the roadmap asked >65% over 50 pairs for "clearly beats
  1320". At 61% the score line is narrowly missed, but +78 Elo with LOS 99%
  (from 0–10 in v0.1) shows a real engine that outplays the weakest anchor.
  v0.3 (transposition table, null move, LMR) must clear the bar outright.

## Raw artifacts (v0.2)

`tools/results/bench_20260913T191959Z.json` (debug),
`tools/results/bench_20260913T193536Z.json` (release),
`tools/results/match_20260913T192511Z.json` + `.pgn` (leg 1, +32−18),
`tools/results/match_20260913T193056Z.json` + `.pgn` (leg 2, +29−21).
