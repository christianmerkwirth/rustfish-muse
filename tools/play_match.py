#!/usr/bin/env python3
"""Play Rustfish vs Stockfish with cutechess-cli and report Elo statistics.

Stockfish 18 anchors the scale through its `UCI_Elo` option (range
1320-3190, verified against the local binary). The target: beat Stockfish
with `UCI_Elo=2400`. Example::

    python3 tools/play_match.py --pairs 10 --sf-elo 1320
    python3 tools/play_match.py --pairs 25 --sf-elo 2400 --tc 15+0.15

Match PGNs land in tools/results/ (gitignored); a JSON summary with the Elo
math is saved next to them. cutechess-cli is required (already used here).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone

import chess

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ENGINE = os.path.join(REPO_ROOT, "target", "debug", "rustfish")
DATA_DIR = os.path.join(REPO_ROOT, "tools", "data")
RESULTS_DIR = os.path.join(REPO_ROOT, "tools", "results")
BOOK_PATH = os.path.join(DATA_DIR, "openings.epd")

# Short, sound opening lines (UCI moves) for colour-balanced variety.
OPENING_LINES = [
    ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"],
    ["e2e4", "c7c5", "g1f3", "d7d6", "d2d4"],
    ["d2d4", "d7d5", "c2c4", "e7e6", "b1c3"],
    ["d2d4", "g8f6", "c2c4", "e7e6", "g1f3"],
    ["e2e4", "e7e6", "d2d4", "d7d5", "b1c3"],
    ["e2e4", "c7c6", "d2d4", "d7d5", "b1c3"],
    ["c2c4", "e7e5", "b1c3", "g8f6", "g1f3"],
    ["g1f3", "d7d5", "g2g3", "g8f6", "f1g2"],
    ["e2e4", "d7d5", "e4d5", "d8d5", "b1c3"],
    ["d2d4", "f7f5", "g2g3", "g8f6", "f1g2"],
    ["e2e4", "g8f6", "e4e5", "f6d5", "d2d4"],
    ["b1c3", "d7d5", "d2d4", "g8f6", "c1f4"],
]


def ensure_book() -> str:
    if os.path.isfile(BOOK_PATH):
        return BOOK_PATH
    os.makedirs(DATA_DIR, exist_ok=True)
    with open(BOOK_PATH, "w") as f:
        for i, line in enumerate(OPENING_LINES):
            board = chess.Board()
            for m in line:
                board.push_uci(m)
            f.write(f"{board.fen()} id \"opening-{i:02d}\";\n")
    return BOOK_PATH


def normal_cdf(x: float) -> float:
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


def elo_stats(wins: int, draws: int, losses: int) -> dict:
    n = wins + draws + losses
    if n == 0:
        return {"games": 0}
    p = (wins + draws / 2) / n
    var = (wins * (1 - p) ** 2 + draws * (0.5 - p) ** 2 + losses * p**2) / n
    se_p = math.sqrt(var / n) if n > 1 else float("inf")
    diff: float
    se_diff: float | None
    if p <= 0.0:
        diff, se_diff = float("-inf"), None
    elif p >= 1.0:
        diff, se_diff = float("inf"), None
    else:
        diff = -400 * math.log10(1 / p - 1)
        se_diff = 400 / (math.log(10) * p * (1 - p)) * se_p if se_p else 0.0
    los = normal_cdf((p - 0.5) / se_p) if se_p else (1.0 if p > 0.5 else 0.0)
    err95 = 1.96 * se_diff if se_diff is not None else None
    return {
        "games": n,
        "wins": wins,
        "draws": draws,
        "losses": losses,
        "score": round(p, 4),
        "elo_diff": None if diff in (float("inf"), float("-inf")) else round(diff, 1),
        "elo_err95": None if err95 is None else round(err95, 1),
        "los": round(los, 4),
    }


def tally_pgn(path: str, engine_name: str) -> tuple[int, int, int]:
    wins = draws = losses = 0
    if not os.path.isfile(path):
        return wins, draws, losses
    with open(path) as f:
        content = f.read()
    # Split into per-game blocks and score each from the engine's side,
    # identified via the White tag (colours alternate, so never assume).
    games = re.split(r"\n\n(?=\[Event )", content)
    for g in games:
        rm = re.search(r'\[Result "([^"]+)"\]', g)
        wm = re.search(r'\[White "([^"]+)"\]', g)
        if not rm or not wm:
            continue
        white_is_engine = engine_name.lower() in wm.group(1).lower()
        r = rm.group(1)
        if r == "1-0":
            wins, losses = (wins + 1, losses) if white_is_engine else (wins, losses + 1)
        elif r == "0-1":
            losses, wins = (losses + 1, wins) if white_is_engine else (losses, wins + 1)
        elif r == "1/2-1/2":
            draws += 1
    return wins, draws, losses


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Rustfish vs Stockfish match runner")
    parser.add_argument("--engine", default=DEFAULT_ENGINE)
    parser.add_argument("--engine-name", default="Rustfish")
    parser.add_argument("--stockfish", default="stockfish")
    parser.add_argument("--sf-elo", type=int, default=1320,
                        help="Stockfish UCI_Elo anchor (1320-3190)")
    parser.add_argument("--pairs", type=int, default=10,
                        help="game pairs; total games = 2*pairs")
    parser.add_argument("--tc", default="10+0.1")
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--engine-opts", default="",
                        help="extra 'option.X=value' for the engine under test")
    args = parser.parse_args(argv)

    if not os.path.isfile(args.engine):
        print(f"engine binary not found: {args.engine} (run cargo build first)")
        return 2
    if shutil.which("cutechess-cli") is None:
        print("cutechess-cli not found on PATH")
        return 2
    if shutil.which(args.stockfish) is None and not os.path.isfile(args.stockfish):
        print(f"stockfish not found: {args.stockfish}")
        return 2
    if not 1320 <= args.sf_elo <= 3190:
        print("--sf-elo must be within Stockfish range 1320-3190")
        return 2

    os.makedirs(RESULTS_DIR, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    pgn = os.path.join(RESULTS_DIR, f"match_{stamp}.pgn")
    book = ensure_book()

    sf_opts = [
        f"option.UCI_LimitStrength=true",
        f"option.UCI_Elo={args.sf_elo}",
        "option.Hash=32",
        "option.Threads=1",
    ]
    eng_opts = ["option.Hash=32", "option.Threads=1"]
    if args.engine_opts:
        eng_opts.extend(args.engine_opts.split())

    cmd = [
        "cutechess-cli",
        "-engine", f"name={args.engine_name}", f"cmd={args.engine}", *eng_opts,
        "-engine", "name=Stockfish", f"cmd={args.stockfish}", *sf_opts,
        "-each", f"tc={args.tc}", "proto=uci",
        "-games", "2", "-rounds", str(args.pairs),
        "-repeat", "-recover",
        "-concurrency", str(args.concurrency),
        "-openings", f"file={book}", "format=epd", "order=random",
        "-resign", "movecount=3", "score=700",
        "-draw", "movenumber=40", "movecount=10", "score=15",
        "-pgnout", pgn,
    ]
    print("+ " + " ".join(cmd))
    proc = subprocess.run(cmd, capture_output=True, text=True)
    output = proc.stdout + proc.stderr
    print(output)

    wins, draws, losses = tally_pgn(pgn, args.engine_name)
    stats = elo_stats(wins, draws, losses)

    # Cross-check against cutechess's own Elo line when present.
    m = re.search(r"Elo difference:\s*(\S+)", output)
    if m:
        try:
            stats["cutechess_elo_diff"] = float(m.group(1))
        except ValueError:
            stats["cutechess_elo_diff"] = m.group(1)

    summary = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "engine": args.engine,
        "stockfish": args.stockfish,
        "sf_elo": args.sf_elo,
        "tc": args.tc,
        "pgn": pgn,
        "stats": stats,
    }
    jpath = os.path.join(RESULTS_DIR, f"match_{stamp}.json")
    with open(jpath, "w") as f:
        json.dump(summary, f, indent=2)
    s = stats
    diff = "n/a (no score)" if s["elo_diff"] is None else f"{s['elo_diff']}±{s['elo_err95']}"
    print(
        f"Rustfish vs Stockfish(UCI_Elo={args.sf_elo}): "
        f"+{s['wins']} ={s['draws']} -{s['losses']} "
        f"score={s['score']} elo_diff={diff} los={s['los']}"
    )
    print(f"saved {jpath}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
