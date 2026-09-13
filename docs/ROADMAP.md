# Roadmap to beating Stockfish at UCI_Elo 2400

Each milestone is a PR gated by the measurement framework. Advance only when
the gate numbers move; if a change does not move them, revert it.

## v0.1 — UCI core + measurement framework (this PR)

Correct protocol surface, deterministic first-legal-move search, 32 Rust unit
tests, 59-check compliance suite, speed/tactics bench, cutechess match
pipeline with Elo stats. Baseline: loses 0–4 to Stockfish at Elo 1320,
0/30 tactics.

## v0.2 — Real search, first strength (~1000–1400)

- Negamax alpha-beta with iterative deepening and a background search thread
  (true `stop` handling, `info depth/score/nps` output)
- Move ordering: PV move, MVV-LVA captures, killers, history (hash move
  arrives with the transposition table in v0.3)
- Quiescence search (captures, check evasions), basic time management
  (clock + movetime + depth/nodes/mate)
- Evaluation: material + piece-square tables, game-phase taper

Gate: beats Stockfish Elo 1320 clearly (>65% over 50 pairs); mate-in-1
mostly solved; perft-validated movegen.

## v0.3 — Selective search (~1600–1900)

- Transposition table (Zobrist, exact/lower/upper, age/replace)
- Null-move pruning, late-move reductions, futility + razoring
- Check extensions, passed-pawn and mobility eval terms

Gate: competitive with Stockfish Elo 1600–1800 anchors; mate-in-2 mostly
solved.

## v0.4 — Evaluation depth (~2000–2200)

- King safety, pawn structure (isolated/doubled/passed), rook on open file,
  bishop pair, tempo; tuned by self-play matches (SPSA or logistic regression
  on game pairs)
- Aspiration windows, principal-variation search, SMP (threads) scaling

Gate: beats Stockfish Elo 2000 over 50+ pairs with LOS > 99%.

## v0.5 — NNUE and the 2400 target (~2300–2500+)

- NNUE inference in Rust (halfKP-style features, incremental updates),
  trained on Stockfish-evaluated Rustfish self-play data
- Full time management (ponder, movestogo, increment model), contempt,
  Syzygy probing optional

Gate: **beats Stockfish at UCI_Elo 2400**: >50% over ≥100 games at 15+0.15
with LOS ≥ 99%. That is the definition of done for the headline goal.

## Notes on the target

Stockfish's `UCI_Elo` scale is a calibrated strength limiter, not a FIDE
rating, but it is the agreed, reproducible yardstick: same binary, same
book, same TC, quoted with error bars. Final validation additionally plays
full-strength Stockfish at fixed nodes for an absolute (if humbling) number.
