#!/usr/bin/env python3
"""Speed + tactical benchmark for Rustfish.

Two independent signal sources, one command::

    python3 tools/bench.py --engine ./target/debug/rustfish

* speed: handshake latency plus sustained `go depth` throughput over random
  middlegame positions (mean/p95 latency, positions per second).
* tactical: mate-in-1 / mate-in-2 EPD suites from tools/data; the engine gets
  `go movetime <ms>` per position and scores when bestmove is a `bm` move.

Results print as text and save as JSON under tools/results/ (gitignored).
"""

from __future__ import annotations

import argparse
import json
import os
import random
import re
import statistics
import sys
import time
from datetime import datetime, timezone

import chess

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from uci_compliance import Engine, EngineError  # noqa: E402

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ENGINE = os.path.join(REPO_ROOT, "target", "debug", "rustfish")
DATA_DIR = os.path.join(REPO_ROOT, "tools", "data")
RESULTS_DIR = os.path.join(REPO_ROOT, "tools", "results")


def random_middlegames(count: int, seed: int) -> list[list[str]]:
    rng = random.Random(seed)
    out = []
    for _ in range(count):
        board = chess.Board()
        for _ in range(rng.randint(15, 60)):
            if board.is_game_over():
                break
            board.push(rng.choice(list(board.legal_moves)))
        if board.is_game_over():
            continue
        out.append([m.uci() for m in board.move_stack])
    return out


def measure_speed(engine_path: str, positions: int, seed: int) -> dict:
    games = random_middlegames(positions, seed)
    eng = Engine(engine_path)
    try:
        t0 = time.perf_counter()
        eng.send("uci")
        eng.wait_for(lambda l: l == "uciok", 5.0)
        eng.send("isready")
        eng.wait_for(lambda l: l == "readyok", 5.0)
        handshake_ms = (time.perf_counter() - t0) * 1000

        lat = []
        for moves in games:
            eng.send(
                "position startpos moves " + " ".join(moves)
                if moves
                else "position startpos"
            )
            t0 = time.perf_counter()
            eng.send("go depth 1")
            eng.wait_bestmove(10.0)
            lat.append((time.perf_counter() - t0) * 1000)
        lat.sort()
        p95 = lat[int(0.95 * (len(lat) - 1))]
        return {
            "positions": len(lat),
            "handshake_ms": round(handshake_ms, 2),
            "mean_ms": round(statistics.mean(lat), 3),
            "p95_ms": round(p95, 3),
            "positions_per_sec": round(1000.0 / statistics.mean(lat), 1),
        }
    finally:
        eng.quit()


def load_epd(path: str) -> list[tuple[str, list[str], str]]:
    entries = []
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        m = re.match(r"(.+?)\s+bm\s+([^;]+);\s*id\s+\"([^\"]+)\"", line)
        assert m, f"malformed EPD line: {line}"
        entries.append((m.group(1), m.group(2).split(), m.group(3)))
    return entries


def mates_immediately(fen: str, move: str) -> bool:
    try:
        board = chess.Board(fen)
        board.push_uci(move)
        return board.is_checkmate()
    except (ValueError, chess.IllegalMoveError):
        return False


def has_mate_in_1(board: chess.Board) -> bool:
    for m in board.legal_moves:
        board.push(m)
        mated = board.is_checkmate()
        board.pop()
        if mated:
            return True
    return False


def forces_mate_in_2(fen: str, move: str) -> bool:
    """Bestmove mates at once or forces mate on the following move."""
    try:
        board = chess.Board(fen)
        board.push_uci(move)
    except (ValueError, chess.IllegalMoveError):
        return False
    if board.is_checkmate():
        return True
    if board.is_game_over():
        return False
    for reply in list(board.legal_moves):
        board.push(reply)
        try:
            if board.is_game_over():
                if not board.is_checkmate():
                    return False
                continue
            if not has_mate_in_1(board):
                return False
        finally:
            board.pop()
    assert len(board.move_stack) == 1
    return True


def measure_tactical(engine_path: str, movetime_ms: int) -> dict:
    eng = Engine(engine_path)
    # Suites may hold positions with several mating moves, so a bestmove
    # scores when it mates (in 1) or forces mate (in 2) — not only when it
    # equals a listed `bm`. The `bm` fields stay as documented solutions.
    checkers = {"mate_in_1.epd": mates_immediately, "mate_in_2.epd": forces_mate_in_2}
    summary: dict[str, dict] = {}
    try:
        eng.send("uci")
        eng.wait_for(lambda l: l == "uciok", 5.0)
        for name, check in checkers.items():
            entries = load_epd(os.path.join(DATA_DIR, name))
            solved = 0
            details = []
            for fen, bms, pid in entries:
                eng.send("ucinewgame")
                eng.send(f"position fen {fen}")
                eng.send(f"go movetime {movetime_ms}")
                try:
                    best, _ = eng.wait_bestmove(movetime_ms / 1000 + 10.0)
                except EngineError:
                    best = "<timeout>"
                hit = check(fen, best)
                solved += hit
                details.append({"id": pid, "bestmove": best, "expected": bms, "hit": hit})
            summary[name] = {
                "solved": solved,
                "total": len(entries),
                "rate": round(solved / len(entries), 3),
                "details": details,
            }
        return summary
    finally:
        eng.quit()


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Rustfish speed + tactical bench")
    parser.add_argument("--engine", default=DEFAULT_ENGINE)
    parser.add_argument("--positions", type=int, default=200)
    parser.add_argument("--seed", type=int, default=1234)
    parser.add_argument("--movetime", type=int, default=200)
    parser.add_argument("--no-save", action="store_true")
    args = parser.parse_args(argv)

    if not os.path.isfile(args.engine):
        print(f"engine binary not found: {args.engine} (run cargo build first)")
        return 2

    speed = measure_speed(args.engine, args.positions, args.seed)
    tactical = measure_tactical(args.engine, args.movetime)
    result = {
        "engine": args.engine,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "speed": speed,
        "tactical": tactical,
    }

    print(f"engine: {args.engine}")
    print(
        "speed: "
        f"{speed['positions']} positions, handshake {speed['handshake_ms']} ms, "
        f"mean {speed['mean_ms']} ms/pos, p95 {speed['p95_ms']} ms/pos, "
        f"{speed['positions_per_sec']} pos/s"
    )
    for name, s in tactical.items():
        print(f"tactical {name}: {s['solved']}/{s['total']} ({s['rate'] * 100:.1f}%)")

    if not args.no_save:
        os.makedirs(RESULTS_DIR, exist_ok=True)
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        path = os.path.join(RESULTS_DIR, f"bench_{stamp}.json")
        with open(path, "w") as f:
            json.dump(result, f, indent=2)
        print(f"saved {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
