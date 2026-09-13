#!/usr/bin/env python3
"""Black-box UCI compliance suite for Rustfish.

Speaks raw UCI to the engine over stdio and checks protocol behaviour:
handshake, options, position handling, all `go` flavours, ponder, stop,
robustness against bad input, move legality (verified with python-chess),
and clean shutdown.

Use as pytest::

    pytest tools/uci_compliance.py --engine ./target/debug/rustfish

or as a CLI::

    python3 tools/uci_compliance.py --engine ./target/debug/rustfish

Exit code is 0 only when every check passes.
"""

from __future__ import annotations

import argparse
import os
import queue
import random
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field

import chess

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ENGINE = os.path.join(REPO_ROOT, "target", "debug", "rustfish")

KIWIPETE = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"
FOOLS_MATE = "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 0 3"
STALEMATE = "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"


class EngineError(Exception):
    pass


class Engine:
    """A running UCI engine subprocess with a background stdout reader."""

    def __init__(self, path: str, startup_timeout: float = 5.0):
        if not os.path.isfile(path):
            raise EngineError(f"engine binary not found: {path}")
        self.proc = subprocess.Popen(
            [path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        self.lines: queue.Queue[str] = queue.Queue()
        self._reader = threading.Thread(target=self._pump, daemon=True)
        self._reader.start()
        self.startup_timeout = startup_timeout

    def _pump(self):
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            self.lines.put(line.rstrip("\n"))
        self.lines.put("<eof>")

    def send(self, cmd: str) -> None:
        assert self.proc.stdin is not None
        try:
            self.proc.stdin.write(cmd + "\n")
            self.proc.stdin.flush()
        except BrokenPipeError as e:
            raise EngineError(f"broken pipe sending {cmd!r}") from e

    def _next(self, timeout: float) -> str:
        try:
            line = self.lines.get(timeout=timeout)
        except queue.Empty as e:
            raise EngineError("timed out waiting for engine output") from e
        if line == "<eof>":
            raise EngineError("engine closed stdout unexpectedly")
        return line

    def wait_for(self, predicate, timeout: float = 5.0) -> str:
        """Consume lines until predicate(line) is true; return that line."""
        deadline = time.time() + timeout
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                raise EngineError("timed out waiting for expected output")
            line = self._next(remaining)
            if predicate(line):
                return line

    def wait_bestmove(self, timeout: float = 5.0) -> tuple[str, str | None]:
        line = self.wait_for(lambda l: l.startswith("bestmove"), timeout)
        parts = line.split()
        move = parts[1] if len(parts) > 1 else ""
        ponder = None
        if len(parts) > 3 and parts[2] == "ponder":
            ponder = parts[3]
        return move, ponder

    def drain(self) -> list[str]:
        out = []
        while True:
            try:
                out.append(self.lines.get_nowait())
            except queue.Empty:
                return out

    def is_alive(self) -> bool:
        return self.proc.poll() is None

    def quit(self, timeout: float = 5.0) -> bool:
        try:
            self.send("quit")
        except EngineError:
            return False
        try:
            self.proc.wait(timeout=timeout)
            return True
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return False


@dataclass
class Check:
    name: str
    ok: bool
    detail: str = ""


@dataclass
class Suite:
    engine_path: str
    timeout: float = 5.0
    results: list[Check] = field(default_factory=list)

    # -- helpers ---------------------------------------------------------
    def fresh(self) -> Engine:
        return Engine(self.engine_path)

    def record(self, name: str, ok: bool, detail: str = "") -> bool:
        self.results.append(Check(name, ok, detail))
        return ok

    def board_after(self, fen: str | None, moves: list[str]) -> chess.Board:
        board = chess.Board(fen) if fen else chess.Board()
        for m in moves:
            board.push_uci(m)
        return board

    def check_legal_bestmove(
        self, name: str, fen: str | None, moves: list[str], go: str
    ) -> bool:
        eng = self.fresh()
        try:
            if fen:
                eng.send(f"position fen {fen} moves {' '.join(moves)}".rstrip())
            elif moves:
                eng.send(f"position startpos moves {' '.join(moves)}")
            else:
                eng.send("position startpos")
            eng.send(go)
            best, _ = eng.wait_bestmove(self.timeout)
            board = self.board_after(fen, moves)
            if board.is_game_over():
                ok = best == "0000"
                return self.record(name, ok, f"terminal pos, bestmove={best}")
            try:
                move = chess.Move.from_uci(best)
            except ValueError:
                return self.record(name, False, f"unparsable bestmove {best!r}")
            ok = move in board.legal_moves
            return self.record(name, ok, f"bestmove={best}")
        except EngineError as e:
            return self.record(name, False, str(e))
        finally:
            eng.quit()

    # -- individual checks ------------------------------------------------
    def check_isready_before_uci(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("isready")
            eng.wait_for(lambda l: l == "readyok", self.timeout)
            return self.record("isready_before_uci", True)
        except EngineError as e:
            return self.record("isready_before_uci", False, str(e))
        finally:
            eng.quit()

    def check_uci_handshake(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("uci")
            seen_name = eng.wait_for(lambda l: l.startswith("id name"), self.timeout)
            seen_author = eng.wait_for(lambda l: l.startswith("id author"), self.timeout)
            eng.wait_for(lambda l: l == "uciok", self.timeout)
            opts = eng.drain()
            ok = bool(seen_name) and bool(seen_author)
            return self.record("uci_handshake", ok, f"{seen_name} / {seen_author}")
        except EngineError as e:
            return self.record("uci_handshake", False, str(e))
        finally:
            eng.quit()

    def check_required_options(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("uci")
            lines = []
            deadline = time.time() + self.timeout
            while time.time() < deadline:
                try:
                    line = eng.lines.get(timeout=0.2)
                except queue.Empty:
                    continue
                lines.append(line)
                if line == "uciok":
                    break
            text = "\n".join(lines)
            ok = "option name Hash" in text and "option name Threads" in text
            return self.record("required_options", ok, "Hash+Threads advertised")
        except EngineError as e:
            return self.record("required_options", False, str(e))
        finally:
            eng.quit()

    def check_setoption(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("uci")
            eng.wait_for(lambda l: l == "uciok", self.timeout)
            for cmd in [
                "setoption name Hash value 64",
                "setoption name Threads value 2",
                "setoption name Ponder value true",
                "setoption name NoSuchOption value 1",
                "setoption name Hash value banana",
            ]:
                eng.send(cmd)
            eng.send("isready")
            eng.wait_for(lambda l: l == "readyok", self.timeout)
            ok = eng.is_alive()
            return self.record("setoption", ok, "survived valid+unknown+bad values")
        except EngineError as e:
            return self.record("setoption", False, str(e))
        finally:
            eng.quit()

    def check_ucinewgame(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("position startpos moves e2e4 e7e5")
            eng.send("ucinewgame")
            eng.send("isready")
            eng.wait_for(lambda l: l == "readyok", self.timeout)
            return self.record("ucinewgame", eng.is_alive())
        except EngineError as e:
            return self.record("ucinewgame", False, str(e))
        finally:
            eng.quit()

    def check_debug_and_unknown_commands(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("debug on")
            eng.send("frobnicate 1 2 3")
            eng.send("debug off")
            eng.send("isready")
            eng.wait_for(lambda l: l == "readyok", self.timeout)
            return self.record("debug_and_unknown", eng.is_alive())
        except EngineError as e:
            return self.record("debug_and_unknown", False, str(e))
        finally:
            eng.quit()

    def check_stop_without_search(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("stop")
            time.sleep(0.2)
            extra = eng.drain()
            ok = not any(l.startswith("bestmove") for l in extra)
            eng.send("isready")
            eng.wait_for(lambda l: l == "readyok", self.timeout)
            return self.record("stop_without_search", ok and eng.is_alive())
        except EngineError as e:
            return self.record("stop_without_search", False, str(e))
        finally:
            eng.quit()

    def check_go_variants(self) -> bool:
        cases = [
            ("go depth 3", None, []),
            ("go nodes 1000", None, []),
            ("go movetime 50", None, []),
            ("go wtime 60000 btime 60000 winc 500 binc 500", None, []),
            ("go wtime 1000 btime 1000 movestogo 10", None, []),
            ("go mate 2", None, []),
            ("go depth 2 searchmoves e2e4 d2d4", None, []),
            ("go infinite", None, []),  # handled separately; placeholder never runs
        ]
        all_ok = True
        for go, fen, moves in cases:
            if go == "go infinite":
                continue
            ok = self.check_legal_bestmove(f"go:{go}", fen, moves, go)
            all_ok = all_ok and ok
        # searchmoves restriction: answer must be one of the listed moves.
        eng = self.fresh()
        try:
            eng.send("position startpos")
            eng.send("go depth 2 searchmoves e2e4")
            best, _ = eng.wait_bestmove(self.timeout)
            ok = best == "e2e4"
            self.record("go:searchmoves_restriction", ok, f"bestmove={best}")
            all_ok = all_ok and ok
        except EngineError as e:
            self.record("go:searchmoves_restriction", False, str(e))
            all_ok = False
        finally:
            eng.quit()
        return all_ok

    def check_positions(self) -> bool:
        all_ok = True
        all_ok &= self.check_legal_bestmove("pos:startpos", None, [], "go depth 2")
        all_ok &= self.check_legal_bestmove(
            "pos:startpos_moves", None, ["e2e4", "e7e5", "g1f3"], "go depth 2"
        )
        all_ok &= self.check_legal_bestmove("pos:kiwipete", KIWIPETE, [], "go depth 2")
        all_ok &= self.check_legal_bestmove(
            "pos:kiwipete_moves", KIWIPETE, ["a2a3", "a6b5"], "go depth 2"
        )
        all_ok &= self.check_legal_bestmove(
            "pos:mate_terminal", FOOLS_MATE, [], "go depth 2"
        )
        all_ok &= self.check_legal_bestmove(
            "pos:stalemate_terminal", STALEMATE, [], "go depth 2"
        )
        return all_ok

    def check_bad_input(self) -> bool:
        all_ok = True
        for name, cmd in [
            ("bad:fen", "position fen not-a-fen"),
            ("bad:moves", "position startpos moves e2e5"),
            ("bad:garbage", "position middlepos"),
            ("bad:empty", "position"),
        ]:
            eng = self.fresh()
            try:
                eng.send(cmd)
                eng.send("isready")
                eng.wait_for(lambda l: l == "readyok", self.timeout)
                eng.send("go depth 1")
                best, _ = eng.wait_bestmove(self.timeout)
                ok = eng.is_alive() and bool(best)
                self.record(name, ok, f"survived {cmd!r}, bestmove={best}")
                all_ok = all_ok and ok
            except EngineError as e:
                self.record(name, False, str(e))
                all_ok = False
            finally:
                eng.quit()
        return all_ok

    def check_infinite_then_stop(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("position startpos")
            eng.send("go infinite")
            time.sleep(0.5)
            early = eng.drain()
            if any(l.startswith("bestmove") for l in early):
                return self.record(
                    "go:infinite_stop", False, "bestmove arrived before stop"
                )
            eng.send("stop")
            best, _ = eng.wait_bestmove(self.timeout)
            board = chess.Board()
            ok = chess.Move.from_uci(best) in board.legal_moves
            return self.record("go:infinite_stop", ok, f"bestmove={best}")
        except (EngineError, ValueError) as e:
            return self.record("go:infinite_stop", False, str(e))
        finally:
            eng.quit()

    def check_ponder(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("position startpos moves e2e4")
            eng.send("go ponder wtime 60000 btime 60000")
            time.sleep(0.3)
            eng.send("ponderhit")
            time.sleep(0.2)
            eng.send("stop")
            best, _ = eng.wait_bestmove(self.timeout)
            board = chess.Board()
            board.push_uci("e2e4")
            ok = chess.Move.from_uci(best) in board.legal_moves
            return self.record("go:ponder", ok, f"bestmove={best}")
        except (EngineError, ValueError) as e:
            return self.record("go:ponder", False, str(e))
        finally:
            eng.quit()

    def check_fuzz_random_positions(self, count: int = 30, seed: int = 20260913) -> bool:
        rng = random.Random(seed)
        all_ok = True
        for i in range(count):
            board = chess.Board()
            for _ in range(rng.randint(0, 60)):
                if board.is_game_over():
                    break
                board.push(rng.choice(list(board.legal_moves)))
            if board.is_game_over():
                continue
            moves = [m.uci() for m in board.move_stack]
            # Reconstruct via startpos + moves for realism.
            ok = self.check_legal_bestmove(f"fuzz:{i}", None, moves, "go depth 1")
            all_ok = all_ok and ok
        self.results.append(
            Check("fuzz:summary", all_ok, f"{count} random positions, seed={seed}")
        )
        return all_ok

    def check_sequential_searches(self) -> bool:
        # Regression test: a `position` arriving right after our `bestmove`
        # (worker finished but unreaped) must update the board. Uses one
        # engine for two back-to-back searches; the second answer must be
        # legal in the second position.
        eng = self.fresh()
        try:
            eng.send("position startpos")
            eng.send("go depth 3")
            first, _ = eng.wait_bestmove(self.timeout)
            eng.send("position startpos moves e2e4")
            eng.send("go depth 3")
            second, _ = eng.wait_bestmove(self.timeout)
            board = chess.Board()
            board.push_uci("e2e4")
            ok = chess.Move.from_uci(second) in board.legal_moves
            return self.record(
                "sequential_searches", ok, f"first={first} second={second}"
            )
        except (EngineError, ValueError) as e:
            return self.record("sequential_searches", False, str(e))
        finally:
            eng.quit()

    def check_quit(self) -> bool:
        eng = self.fresh()
        try:
            eng.send("isready")
            eng.wait_for(lambda l: l == "readyok", self.timeout)
            ok = eng.quit()
            return self.record("quit", ok, "process exited on quit")
        except EngineError as e:
            return self.record("quit", False, str(e))

    def run_all(self) -> list[Check]:
        self.results = []
        self.check_isready_before_uci()
        self.check_uci_handshake()
        self.check_required_options()
        self.check_setoption()
        self.check_ucinewgame()
        self.check_debug_and_unknown_commands()
        self.check_stop_without_search()
        self.check_positions()
        self.check_go_variants()
        self.check_bad_input()
        self.check_infinite_then_stop()
        self.check_ponder()
        self.check_sequential_searches()
        self.check_fuzz_random_positions()
        self.check_quit()
        return self.results


def format_report(results: list[Check]) -> tuple[str, bool]:
    lines = []
    all_ok = True
    for c in results:
        mark = "PASS" if c.ok else "FAIL"
        all_ok = all_ok and c.ok
        extra = f" -- {c.detail}" if c.detail else ""
        lines.append(f"[{mark}] {c.name}{extra}")
    passed = sum(1 for c in results if c.ok)
    lines.append(f"\n{passed}/{len(results)} checks passed")
    return "\n".join(lines), all_ok


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="UCI compliance suite for Rustfish")
    parser.add_argument("--engine", default=DEFAULT_ENGINE)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--fuzz", type=int, default=30)
    args = parser.parse_args(argv)

    suite = Suite(engine_path=args.engine, timeout=args.timeout)
    results = suite.run_all()
    report, all_ok = format_report(results)
    print(report)
    return 0 if all_ok else 1


# -- pytest integration -----------------------------------------------------
def _pytest_engine_path(config) -> str:
    opt = config.getoption("engine", default=None)
    if opt:
        return opt
    return os.environ.get("RUSTFISH_BIN", DEFAULT_ENGINE)


def pytest_addoption(parser):
    parser.addoption("--engine", action="store", default=None)


def pytest_generate_tests(metafunc):
    if "check_name" in metafunc.fixturenames:
        names = [
            "isready_before_uci",
            "uci_handshake",
            "required_options",
            "setoption",
            "ucinewgame",
            "debug_and_unknown",
            "stop_without_search",
            "positions",
            "go_variants",
            "bad_input",
            "infinite_stop",
            "ponder",
            "sequential",
            "fuzz",
            "quit",
        ]
        metafunc.parametrize("check_name", names)


def test_uci_compliance(check_name, pytestconfig):
    engine = _pytest_engine_path(pytestconfig)
    if not os.path.isfile(engine):
        import pytest as _pytest

        _pytest.skip(f"engine binary not found: {engine} (build with cargo build)")
    suite = Suite(engine_path=engine)
    method = {
        "isready_before_uci": suite.check_isready_before_uci,
        "uci_handshake": suite.check_uci_handshake,
        "required_options": suite.check_required_options,
        "setoption": suite.check_setoption,
        "ucinewgame": suite.check_ucinewgame,
        "debug_and_unknown": suite.check_debug_and_unknown_commands,
        "stop_without_search": suite.check_stop_without_search,
        "positions": suite.check_positions,
        "go_variants": suite.check_go_variants,
        "bad_input": suite.check_bad_input,
        "infinite_stop": suite.check_infinite_then_stop,
        "ponder": suite.check_ponder,
        "sequential": suite.check_sequential_searches,
        "fuzz": suite.check_fuzz_random_positions,
        "quit": suite.check_quit,
    }[check_name]
    assert method(), f"compliance check failed: {check_name}\n{suite.results[-1].detail}"


if __name__ == "__main__":
    sys.exit(main())
