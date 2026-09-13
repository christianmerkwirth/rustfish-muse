#!/usr/bin/env python3
"""Generate verified tactical EPD suites for the bench harness.

Random middlegame positions are brute-force checked with python-chess, so
every stored `bm` move is proven to mate in N. Deterministic for a fixed
seed. Regenerate with::

    python3 tools/make_tactical_epd.py --mate1 20 --mate2 10 --seed 7
"""

from __future__ import annotations

import argparse
import os
import random

import chess

DATA_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")


def mating_moves(board: chess.Board) -> list[chess.Move]:
    """All legal moves that deliver checkmate immediately."""
    mates = []
    for move in board.legal_moves:
        board.push(move)
        if board.is_checkmate():
            mates.append(move)
        board.pop()
    return mates


def is_forced_mate_in_2(board: chess.Board, move: chess.Move) -> bool:
    """True if `move` forces mate on the next move against every reply."""
    depth = len(board.move_stack)
    board.push(move)
    try:
        if board.is_game_over():
            return False  # mate in 1 (or stalemate), not mate in 2
        for reply in list(board.legal_moves):
            board.push(reply)
            try:
                if board.is_game_over():
                    if not board.is_checkmate():
                        return False  # stalemate or we get mated: not forced
                    continue
                if not mating_moves(board):
                    return False
            finally:
                board.pop()
        return True
    finally:
        while len(board.move_stack) > depth:
            board.pop()


def random_middlegame(rng: random.Random) -> chess.Board:
    board = chess.Board()
    for _ in range(rng.randint(20, 50)):
        if board.is_game_over():
            break
        # Prefer captures/checks to raise the tactics density.
        moves = list(board.legal_moves)
        tactical = [m for m in moves if board.is_capture(m) or board.gives_check(m)]
        pool = tactical * 3 + moves
        board.push(rng.choice(pool))
    return board


def collect(count: int, want_mate_in: int, seed: int) -> list[tuple[str, list[str]]]:
    rng = random.Random(seed)
    found: dict[str, list[str]] = {}
    attempts = 0
    while len(found) < count and attempts < 200000:
        attempts += 1
        board = random_middlegame(rng)
        if board.is_game_over() or board.is_check():
            continue
        if want_mate_in == 1:
            mates = mating_moves(board)
            if len(mates) == 1:
                found[board.fen()] = [mates[0].uci()]
        else:
            for move in list(board.legal_moves):
                if is_forced_mate_in_2(board, move):
                    assert len(board.move_stack) >= 0
                    found.setdefault(board.fen(), []).append(move.uci())
                    break
    if len(found) < count:
        raise RuntimeError(
            f"only found {len(found)}/{count} mate-in-{want_mate_in} "
            f"after {attempts} attempts"
        )
    return sorted(found.items())


def write_epd(path: str, entries: list[tuple[str, list[str]]], tag: str) -> None:
    with open(path, "w") as f:
        for i, (fen, bms) in enumerate(entries):
            f.write(f"{fen} bm {' '.join(bms)}; id \"{tag}-{i:02d}\";\n")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mate1", type=int, default=20)
    parser.add_argument("--mate2", type=int, default=10)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args(argv)

    os.makedirs(DATA_DIR, exist_ok=True)
    m1 = collect(args.mate1, 1, args.seed)
    m2 = collect(args.mate2, 2, args.seed + 1)
    write_epd(os.path.join(DATA_DIR, "mate_in_1.epd"), m1, "m1")
    write_epd(os.path.join(DATA_DIR, "mate_in_2.epd"), m2, "m2")
    print(f"wrote {len(m1)} mate-in-1 and {len(m2)} mate-in-2 positions to {DATA_DIR}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
