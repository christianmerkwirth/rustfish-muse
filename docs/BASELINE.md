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

---

# v0.3 selective search (this milestone)

Engine: v0.2 + Zobrist transposition table (bounds, mate adjustment,
generations, Hash option, `hashfull` reporting), null-move pruning (R=2/3,
non-pawn guard), LMR with PVS re-search, reverse futility, razoring, check
extensions, threefold-repetition scoring, allocation-free search temps.

## Correctness

- `cargo test`: **57/57 pass** (adds TT roundtrips/mate-adjust/replacement,
  repetition counting, TT-reuse node reduction, mate-in-2 vs pruning,
  play_temp key equivalence); clippy `-D warnings` clean, fmt clean
- UCI compliance CLI: **60/60**, pytest **15/15** (unchanged suite, still green)
- Perft unchanged (no movegen changes)

## Speed and tactics (`bench.py`, release)

- 1282 pos/s at `go depth 1`; depth-6 startpos: 24,103 nodes (v0.2: 163,549 —
  **6.8× smaller trees**), 2.61M nps after the play_temp optimization
  (1.52M before, same node counts: behavior-identical speedup)
- mate_in_1: **20/20**; mate_in_2: **10/10** — bench now verifies mates
  instead of matching `bm` (three EPDs hold shorter mates than listed)

## Matches (release build, 10+0.1, 12-line book)

- vs Stockfish `UCI_Elo=1320`, 50 games: **+28 =0 −22 (56%)**, +41.9±97.0
- vs `UCI_Elo=1600`, 20 games: **+10 =0 −10 (50%)** (v0.2 context: 8−12, 40%)
- vs `UCI_Elo=1800`, 20 games: **+9 =1 −10 (47.5%)**
- Head-to-head v0.3 vs v0.2, 100 games: **+41 =17 −42 (49.5%)**, ≈0 Elo
- Zero illegal-move terminations everywhere

## Honest gate read

Mixed. "Competitive with 1600–1800" is met (even scores, and ahead of v0.2's
40% vs 1600). "Mate-in-2 mostly solved" is met (10/10). But the 1320 bar
(>65%) is still missed, and the 100-game A/B says selectivity adds no Elo at
bullet: both versions already out-tactic weak opposition, and the PST eval is
now the binding constraint — extra depth does not convert. v0.4 (deeper eval:
king safety, pawn structure, tuned values) attacks exactly that ceiling.

## Raw artifacts (v0.3)

`tools/results/bench_20260913T195124Z.json`,
`tools/results/bench_20260913T195302Z.json` (mate-verified scoring),
`tools/results/match_20260913T195314Z.json` (1320, +28−22),
`tools/results/match_20260913T195509Z.json` (1600, +10−10),
`tools/results/match_20260913T195608Z.json` (1800, +9=1−10),
`tools/results/selfplay_v03_v02.pgn` + `_b.pgn` (A/B, 49.5%).
