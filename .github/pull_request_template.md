## What changes

## Milestone / gate (see docs/ROADMAP.md)

## Verification (paste numbers, not just green)

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
- [ ] `python3 tools/uci_compliance.py --engine ./target/debug/rustfish`
- [ ] `python3 tools/bench.py` before → after (speed + mate_in_1/mate_in_2)
- [ ] Strength change? `python3 tools/play_match.py` result vs Stockfish anchor:
      `+W =D -L`, Elo diff ± err95, LOS, TC, games

## Baseline comparison

<!-- For strength PRs: quote the docs/BASELINE.md numbers you moved and
     attach the tools/results JSON names. If no strength claim, say so. -->
