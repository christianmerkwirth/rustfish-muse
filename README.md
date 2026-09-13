# Rustfish

A UCI-compatible chess engine in Rust. The goal: beat Stockfish 18 limited to
`UCI_Elo=2400`.

Status: v0.3 — selective search (transposition table, null move, LMR,
futility, repetition) on top of the v0.2 engine. Strength work follows the
[roadmap](docs/ROADMAP.md); every step is gated by measurement
([guide](docs/MEASUREMENT.md), [baseline](docs/BASELINE.md)).

## Quickstart

```bash
cargo build
cargo test
python3 tools/uci_compliance.py --engine ./target/debug/rustfish
python3 tools/bench.py --engine ./target/debug/rustfish
python3 tools/play_match.py --pairs 10 --sf-elo 1320
```

Python tools need `python-chess` and `pytest`:

```bash
pip install -r tools/requirements.txt
```

## Layout

- `src/main.rs` — binary entry, UCI stdio loop wiring
- `src/uci.rs` — UCI protocol parsing and command loop
- `src/position.rs` — board state on top of `shakmaty`
- `src/search.rs` — search (`GoLimits` parsing + move choice)
- `tools/uci_compliance.py` — black-box UCI compliance suite (CLI + pytest)
- `tools/bench.py` — speed and tactical benchmark
- `tools/make_tactical_epd.py` — generates the verified mate suites
- `tools/play_match.py` — Rustfish vs Stockfish matches via cutechess-cli
- `tools/data/` — mate suites, opening book (generated, committed)
- `tools/results/` — bench/match output (generated, gitignored)
- `docs/` — measurement guide, roadmap, baseline results

## Development workflow

Major features land through pull requests against `main`; CI runs
`cargo fmt`, `clippy`, `cargo test`, the UCI compliance suite and a bench
smoke test. See [docs/MEASUREMENT.md](docs/MEASUREMENT.md) for how to measure
before/after on every strength change.
