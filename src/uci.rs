//! Universal Chess Interface (UCI) protocol loop.
//!
//! Implements the GUI-to-engine direction of UCI (see
//! <https://www.chessprogramming.org/UCI>): `uci`, `debug`, `isready`,
//! `setoption`, `ucinewgame`, `position`, `go`, `stop`, `ponderhit`, `quit`.
//! Unknown commands are ignored as the spec requires.
//!
//! The loop is synchronous: bounded `go` commands answer immediately, while
//! `go infinite` / `go ponder` enter a wait state that keeps serving `isready`
//! / `stop` / `ponderhit` / `quit` until the search ends. True background
//! search arrives with iterative deepening; the command surface stays the same.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::position::Position;
use crate::search::{self, GoLimits};

/// Engine identity reported via `id name` / `id author`.
pub struct EngineInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub author: &'static str,
}

/// Mutable engine state owned by the UCI loop.
pub struct EngineState {
    pub pos: Position,
    pub hash_mb: u32,
    pub threads: u32,
    pub ponder_enabled: bool,
    pub debug: bool,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            pos: Position::startpos(),
            hash_mb: 16,
            threads: 1,
            ponder_enabled: false,
            debug: false,
        }
    }
}

/// Parsed `setoption name ... [value ...]` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetOption {
    pub name: String,
    pub value: Option<String>,
}

/// Parse `setoption` arguments (everything after the `setoption` keyword).
/// Returns `None` when the line does not start with `name`.
pub fn parse_setoption(args: &str) -> Option<SetOption> {
    let rest = args.trim_start();
    let after_name = rest.strip_prefix("name")?;
    let after_name = after_name.strip_prefix([' ', '\t']).unwrap_or(after_name);
    if after_name.is_empty() {
        return None;
    }
    // Split `name <name> value <value>` on the LAST " value " so option names
    // containing the word "value" keep working.
    if let Some(idx) = after_name.rfind(" value ") {
        let name = after_name[..idx].trim_end().to_string();
        let value = after_name[idx + " value ".len()..].to_string();
        if name.is_empty() {
            return None;
        }
        Some(SetOption {
            name,
            value: Some(value),
        })
    } else if after_name.trim_end().ends_with(" value") {
        let name = after_name.trim_end();
        let name = name[..name.len() - " value".len()].trim_end().to_string();
        if name.is_empty() {
            return None;
        }
        Some(SetOption {
            name,
            value: Some(String::new()),
        })
    } else {
        Some(SetOption {
            name: after_name.trim_end().to_string(),
            value: None,
        })
    }
}

/// Parsed `position ...` command: FEN plus trailing move list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedPosition {
    pub fen: Option<String>,
    pub moves: Vec<String>,
}

/// Parse everything after the `position` keyword. `None` on malformed input.
pub fn parse_position(args: &str) -> Option<ParsedPosition> {
    let rest = args.trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(after) = rest.strip_prefix("startpos") {
        let after = after.trim();
        let moves = parse_moves_suffix(after)?;
        return Some(ParsedPosition { fen: None, moves });
    }
    if let Some(after) = rest.strip_prefix("fen") {
        if after.is_empty() || !after.starts_with([' ', '\t']) {
            return None;
        }
        let after = after.trim();
        // Split "<fen> moves <list>" on the LAST " moves " so a move list
        // containing that text cannot corrupt the FEN.
        if let Some(idx) = after.rfind(" moves ") {
            let fen = after[..idx].trim().to_string();
            let moves: Vec<String> = after[idx + " moves ".len()..]
                .split_whitespace()
                .map(str::to_string)
                .collect();
            if fen.is_empty() {
                return None;
            }
            return Some(ParsedPosition {
                fen: Some(fen),
                moves,
            });
        }
        if after.trim_end().ends_with(" moves") {
            let fen = after.trim_end();
            let fen = fen[..fen.len() - " moves".len()].trim().to_string();
            if fen.is_empty() {
                return None;
            }
            return Some(ParsedPosition {
                fen: Some(fen),
                moves: Vec::new(),
            });
        }
        if after.is_empty() {
            return None;
        }
        return Some(ParsedPosition {
            fen: Some(after.to_string()),
            moves: Vec::new(),
        });
    }
    None
}

fn parse_moves_suffix(after: &str) -> Option<Vec<String>> {
    let after = after.trim();
    if after.is_empty() {
        return Some(Vec::new());
    }
    let keyword = after.strip_prefix("moves")?;
    if keyword.is_empty() || !keyword.starts_with([' ', '\t']) {
        return None;
    }
    Some(keyword.split_whitespace().map(str::to_string).collect())
}

fn uci_id_block(info: &EngineInfo) -> Vec<String> {
    vec![
        format!("id name {} {}", info.name, info.version),
        format!("id author {}", info.author),
        "option name Hash type spin default 16 min 1 max 1024".to_string(),
        "option name Threads type spin default 1 min 1 max 512".to_string(),
        "option name Ponder type check default false".to_string(),
        "uciok".to_string(),
    ]
}

fn apply_option(state: &mut EngineState, opt: &SetOption) {
    match opt.name.to_ascii_lowercase().as_str() {
        "hash" => {
            if let Some(v) = opt.value.as_deref()
                && let Ok(mb) = v.trim().parse::<u32>()
            {
                state.hash_mb = mb.clamp(1, 1024);
            }
        }
        "threads" => {
            if let Some(v) = opt.value.as_deref()
                && let Ok(t) = v.trim().parse::<u32>()
            {
                state.threads = t.clamp(1, 512);
            }
        }
        "ponder" => {
            if let Some(v) = opt.value.as_deref() {
                state.ponder_enabled =
                    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "on");
            }
        }
        _ => {
            // Unknown options are ignored per the UCI spec.
        }
    }
}

/// Write one protocol line, tolerating a poisoned mutex (a panicked search
/// thread must not take the command loop down with it).
fn emit<W: Write>(out: &Mutex<W>, line: &str) {
    let mut guard = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(guard, "{line}");
    let _ = guard.flush();
}

/// A running background search.
struct ActiveSearch {
    handle: thread::JoinHandle<()>,
    control: Arc<search::Control>,
}

/// Launch a search thread. It owns all `info`/`bestmove` output for the
/// search and prints exactly one `bestmove` line when it ends, whether it
/// finishes, hits a limit, or is stopped.
fn spawn_search<W: Write + Send + 'static>(
    out: &Arc<Mutex<W>>,
    pos: &Position,
    limits: GoLimits,
) -> ActiveSearch {
    let control = Arc::new(search::Control::new());
    let worker_control = Arc::clone(&control);
    let worker_out = Arc::clone(out);
    let worker_pos = pos.clone();
    let handle = thread::spawn(move || {
        let result = search::search(&worker_pos, &limits, &worker_control, &|line| {
            emit(&worker_out, line);
        });
        match result.ponder {
            Some(ponder) => emit(
                &worker_out,
                &format!("bestmove {} ponder {ponder}", result.bestmove),
            ),
            None => emit(&worker_out, &format!("bestmove {}", result.bestmove)),
        }
    });
    ActiveSearch { handle, control }
}

/// Join a finished search thread. On panic (the only path where the worker
/// could not print), fall back to a null `bestmove` so the GUI never hangs.
fn reap<W: Write>(active: &mut Option<ActiveSearch>, out: &Arc<Mutex<W>>) {
    if let Some(search) = active
        && search.handle.is_finished()
    {
        let search = active.take().expect("active search");
        if search.handle.join().is_err() {
            emit(out, "info string search thread failed");
            emit(out, "bestmove 0000");
        }
    }
}

/// Stop a running search and wait for its `bestmove`.
fn stop_search<W: Write>(active: &mut Option<ActiveSearch>, out: &Arc<Mutex<W>>) {
    if let Some(search) = active.take() {
        search
            .control
            .stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if search.handle.join().is_err() {
            emit(out, "info string search thread failed");
            emit(out, "bestmove 0000");
        }
    }
}

/// Run the command loop until `quit` or end of input. Exposed for tests with
/// in-memory buffers. Returns the writer so tests can inspect the output.
pub fn run_loop<R: BufRead, W: Write + Send + 'static>(input: R, output: W, info: EngineInfo) -> W {
    let out = Arc::new(Mutex::new(output));
    let mut state = EngineState::default();
    let mut active: Option<ActiveSearch> = None;

    for line in input.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }
        let (keyword, args) = match cmd.find([' ', '\t']) {
            Some(i) => (&cmd[..i], cmd[i + 1..].trim_start()),
            None => (cmd, ""),
        };

        // Commands that act on a running search take effect immediately.
        match keyword {
            "stop" => {
                stop_search(&mut active, &out);
                continue;
            }
            "quit" => {
                stop_search(&mut active, &out);
                break;
            }
            "ponderhit" => {
                if let Some(search) = active.as_ref() {
                    search
                        .control
                        .ponderhit
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                continue;
            }
            _ => {}
        }

        // Reap a finished worker first: a `position`/`go` arriving right after
        // our `bestmove` must be processed normally, never dropped. (Dropping
        // it desynchronises the board and the next `bestmove` is illegal in
        // the real game.)
        reap(&mut active, &out);

        if active.is_some() {
            // A search is still running: only sync commands get answers.
            match keyword {
                "isready" => emit(&out, "readyok"),
                "debug" => {
                    state.debug = matches!(args.to_ascii_lowercase().as_str(), "on");
                }
                _ => {}
            }
            continue;
        }

        match keyword {
            "uci" => {
                for line in uci_id_block(&info) {
                    emit(&out, &line);
                }
            }
            "debug" => {
                state.debug = matches!(args.to_ascii_lowercase().as_str(), "on");
            }
            "isready" => {
                emit(&out, "readyok");
            }
            "setoption" => {
                if let Some(opt) = parse_setoption(args) {
                    apply_option(&mut state, &opt);
                }
            }
            "ucinewgame" => {
                state.pos = Position::startpos();
            }
            "position" => match parse_position(args) {
                Some(parsed) => {
                    let mut next = match parsed.fen {
                        Some(fen) => match Position::from_fen(&fen) {
                            Ok(p) => p,
                            Err(e) => {
                                if state.debug {
                                    emit(&out, &format!("info string position error: {e}"));
                                }
                                continue;
                            }
                        },
                        None => Position::startpos(),
                    };
                    if let Err(e) = next.apply_uci_moves(&parsed.moves) {
                        if state.debug {
                            emit(&out, &format!("info string position error: {e}"));
                        }
                        continue;
                    }
                    state.pos = next;
                }
                None => {
                    if state.debug {
                        emit(&out, "info string position error: malformed command");
                    }
                }
            },
            "go" => {
                let limits = search::parse_go(&args.split_whitespace().collect::<Vec<_>>());
                active = Some(spawn_search(&out, &state.pos, limits));
            }
            "stop" => {
                // No search running: nothing to do.
            }
            "ponderhit" => {
                // No search running: nothing to do.
            }
            "quit" => break,
            _ => {
                // Unknown commands are ignored per the UCI spec.
            }
        }
    }
    stop_search(&mut active, &out);
    // All worker threads are joined, so this is the last Arc.
    Arc::try_unwrap(out)
        .unwrap_or_else(|_| panic!("output still shared"))
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_info() -> EngineInfo {
        EngineInfo {
            name: "Rustfish-Test",
            version: "0.0.0",
            author: "Tester",
        }
    }

    fn drive(input: &str) -> String {
        let output = run_loop(Cursor::new(input.as_bytes()), Vec::new(), test_info());
        String::from_utf8(output).unwrap()
    }

    fn bestmove_of(output: &str) -> Option<String> {
        output.lines().find_map(|l| {
            l.strip_prefix("bestmove ")
                .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
        })
    }

    #[test]
    fn uci_handshake_reports_identity_and_options() {
        let out = drive("uci\nquit\n");
        assert!(out.contains("id name Rustfish-Test"), "got:\n{out}");
        assert!(out.contains("id author Tester"), "got:\n{out}");
        assert!(out.contains("option name Hash"), "got:\n{out}");
        assert!(out.contains("option name Threads"), "got:\n{out}");
        assert!(out.lines().any(|l| l == "uciok"), "got:\n{out}");
    }

    #[test]
    fn isready_before_and_after_uci() {
        let out = drive("isready\nuci\nisready\nquit\n");
        assert_eq!(out.lines().filter(|l| *l == "readyok").count(), 2);
    }

    #[test]
    fn go_depth_returns_legal_bestmove() {
        let out = drive("position startpos\ngo depth 1\nquit\n");
        let bm = bestmove_of(&out).expect("no bestmove");
        let legal: Vec<String> = Position::startpos()
            .legal_moves_sorted()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(legal.contains(&bm), "illegal bestmove {bm}");
    }

    #[test]
    fn go_movetime_and_clock_limits_answer() {
        for go in [
            "go movetime 50",
            "go wtime 60000 btime 60000 winc 500 binc 500",
            "go nodes 100 depth 2 movestogo 30",
            "go mate 2",
        ] {
            let out = drive(&format!("position startpos\n{go}\nquit\n"));
            assert!(
                bestmove_of(&out).is_some(),
                "no bestmove for {go:?}:\n{out}"
            );
        }
    }

    #[test]
    fn infinite_search_needs_stop() {
        let out = drive("position startpos\ngo infinite\nstop\nquit\n");
        assert!(bestmove_of(&out).is_some(), "got:\n{out}");
    }

    #[test]
    fn infinite_search_ignores_other_commands_until_stop() {
        let out = drive("go infinite\nposition startpos\ngo depth 1\nstop\nquit\n");
        // Exactly one bestmove: the pending search's. The mid-search `go` is ignored.
        assert_eq!(out.lines().filter(|l| l.starts_with("bestmove")).count(), 1);
    }

    #[test]
    fn ponderhit_then_stop_returns_bestmove() {
        let out = drive(
            "position startpos moves e2e4\ngo ponder wtime 60000 btime 60000\nponderhit\nstop\nquit\n",
        );
        assert!(bestmove_of(&out).is_some(), "got:\n{out}");
    }

    #[test]
    fn setoption_updates_state_and_survives_unknown_options() {
        let out = drive(
            "setoption name Hash value 64\nsetoption name Threads value 4\nsetoption name Ponder value true\nsetoption name NoSuchOption value 1\nisready\nquit\n",
        );
        assert!(out.contains("readyok"));
    }

    fn search_at_depth(pos: &Position, depth: u64) -> String {
        let limits = search::parse_go(&["depth", &depth.to_string()]);
        search::search(pos, &limits, &search::Control::new(), &|_| {}).bestmove
    }

    #[test]
    fn ucinewgame_resets_position() {
        // One search only: a second `go` while the first runs is ignored, so
        // reset first, then search from the fresh startpos.
        let out = drive("position startpos moves e2e4 e7e5\nucinewgame\ngo depth 1\nquit\n");
        let expected = search_at_depth(&Position::startpos(), 1);
        let last = out
            .lines()
            .filter_map(|l| l.strip_prefix("bestmove "))
            .next_back()
            .unwrap();
        assert_eq!(last.split_whitespace().next().unwrap(), expected);
    }

    #[test]
    fn go_depth_emits_info_lines() {
        // The trailing `quit` may cut deepening short, but iteration 1 always
        // completes and reports before the worker can observe the stop.
        let out = drive("position startpos\ngo depth 2\nquit\n");
        assert!(out.contains("info depth 1"), "got:\n{out}");
        assert!(out.contains("score"), "got:\n{out}");
        assert!(out.contains(" pv "), "got:\n{out}");
        assert!(bestmove_of(&out).is_some());
    }

    #[test]
    fn isready_answered_during_search() {
        let out = drive("position startpos\ngo infinite\nisready\nstop\nquit\n");
        assert!(out.contains("readyok"), "got:\n{out}");
        assert!(bestmove_of(&out).is_some(), "got:\n{out}");
    }

    #[test]
    fn quit_during_search_exits() {
        // Must return (join the worker) rather than hang.
        let out = drive("position startpos\ngo infinite\nquit\n");
        let _ = out;
    }

    #[test]
    fn sequential_searches_each_see_their_position() {
        // `stop` fully joins the worker, so the next `position`+`go` pair
        // must be searched from the new board, not the old one.
        let out = drive(
            "position startpos\ngo depth 1\nstop\nposition startpos moves e2e4\ngo depth 1\nquit\n",
        );
        let moves: Vec<&str> = out
            .lines()
            .filter_map(|l| l.strip_prefix("bestmove "))
            .map(|rest| rest.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(moves.len(), 2, "got:\n{out}");
        let mut after_e4 = Position::startpos();
        after_e4
            .apply_uci_moves(&["e2e4".to_string()])
            .unwrap();
        assert_eq!(moves[1], search_at_depth(&after_e4, 1));
    }

    #[test]
    fn fen_position_with_moves() {
        let out = drive(
            "position fen rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2 moves g1f3\ngo depth 1\nquit\n",
        );
        assert!(bestmove_of(&out).is_some(), "got:\n{out}");
    }

    #[test]
    fn bad_position_keeps_old_state() {
        // Illegal move sequence is rejected; the engine must stay alive and
        // answer from the previous (startpos) position.
        let out = drive("position startpos moves e2e5\ngo depth 1\nquit\n");
        let bm = bestmove_of(&out).expect("engine died on bad position");
        let expected = search_at_depth(&Position::startpos(), 1);
        assert_eq!(bm, expected);
    }

    #[test]
    fn unknown_commands_are_ignored() {
        let out = drive("frobnicate 1 2 3\nisready\nquit\n");
        assert!(out.contains("readyok"));
    }

    #[test]
    fn debug_flag_accepted() {
        let out = drive("debug on\ndebug off\nisready\nquit\n");
        assert!(out.contains("readyok"));
    }

    #[test]
    fn stop_without_search_is_noop() {
        let out = drive("stop\nisready\nquit\n");
        assert!(out.contains("readyok"));
        assert!(!out.contains("bestmove"));
    }

    #[test]
    fn setoption_parsing() {
        assert_eq!(
            parse_setoption("name Hash value 128"),
            Some(SetOption {
                name: "Hash".to_string(),
                value: Some("128".to_string())
            })
        );
        assert_eq!(
            parse_setoption("name Ponder value true"),
            Some(SetOption {
                name: "Ponder".to_string(),
                value: Some("true".to_string())
            })
        );
        assert_eq!(
            parse_setoption("name Clear Hash"),
            Some(SetOption {
                name: "Clear Hash".to_string(),
                value: None
            })
        );
        assert_eq!(parse_setoption("value 3"), None);
        assert_eq!(parse_setoption("name"), None);
    }

    #[test]
    fn position_parsing() {
        assert_eq!(
            parse_position("startpos"),
            Some(ParsedPosition {
                fen: None,
                moves: vec![]
            })
        );
        assert_eq!(
            parse_position("startpos moves e2e4 e7e5"),
            Some(ParsedPosition {
                fen: None,
                moves: vec!["e2e4".to_string(), "e7e5".to_string()]
            })
        );
        let p = parse_position(
            "fen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1 moves e2e4",
        )
        .unwrap();
        assert!(p.fen.unwrap().starts_with("rnbqkbnr"));
        assert_eq!(p.moves, vec!["e2e4".to_string()]);
        assert!(parse_position("").is_none());
        assert!(parse_position("middlepos").is_none());
    }

    #[test]
    fn checkmate_position_reports_null_bestmove() {
        // Fool's mate final position, black just mated white... white to move is mated.
        let out = drive(
            "position fen rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 0 3\ngo depth 1\nquit\n",
        );
        assert!(out.contains("bestmove 0000"), "got:\n{out}");
    }
}
