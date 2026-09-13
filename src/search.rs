//! Search placeholder (v0.1): deterministic first-legal-move choice.
//!
//! This module owns the [`GoLimits`] parser target and the [`search`]
//! function. The current implementation answers immediately with the
//! lexicographically first legal move so the UCI loop, the compliance suite
//! and the measurement harness have a correct target to exercise. Real
//! strength (alpha-beta, quiescence, eval, time management) lands here in
//! later milestones without changing the UCI surface.

use std::time::{Duration, Instant};

use crate::position::Position;

/// Parsed `go` command limits. Every field is optional; `None` means the GUI
/// did not constrain that axis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GoLimits {
    pub searchmoves: Vec<String>,
    pub ponder: bool,
    pub infinite: bool,
    pub white_time_ms: Option<u64>,
    pub black_time_ms: Option<u64>,
    pub white_inc_ms: Option<u64>,
    pub black_inc_ms: Option<u64>,
    pub moves_to_go: Option<u64>,
    pub depth: Option<u64>,
    pub nodes: Option<u64>,
    pub mate_in: Option<u64>,
    pub move_time_ms: Option<u64>,
}

impl GoLimits {
    /// True when the search must not return until `stop`/`ponderhit`/`quit`.
    pub fn is_unbounded(&self) -> bool {
        self.infinite || self.ponder
    }
}

/// Parse the token stream after `go`. Unknown tokens are ignored so newer GUI
/// extensions cannot break the engine.
pub fn parse_go(args: &[&str]) -> GoLimits {
    let mut limits = GoLimits::default();
    let mut i = 0;
    let num = |args: &[&str], i: &mut usize| -> Option<u64> {
        if *i + 1 < args.len() {
            if let Ok(v) = args[*i + 1].parse::<u64>() {
                *i += 2;
                return Some(v);
            }
        }
        *i += 1;
        None
    };
    while i < args.len() {
        match args[i] {
            "searchmoves" => {
                i += 1;
                while i < args.len() && !is_keyword(args[i]) {
                    limits.searchmoves.push(args[i].to_string());
                    i += 1;
                }
            }
            "ponder" => {
                limits.ponder = true;
                i += 1;
            }
            "infinite" => {
                limits.infinite = true;
                i += 1;
            }
            "wtime" => limits.white_time_ms = num(args, &mut i),
            "btime" => limits.black_time_ms = num(args, &mut i),
            "winc" => limits.white_inc_ms = num(args, &mut i),
            "binc" => limits.black_inc_ms = num(args, &mut i),
            "movestogo" => limits.moves_to_go = num(args, &mut i),
            "depth" => limits.depth = num(args, &mut i),
            "nodes" => limits.nodes = num(args, &mut i),
            "mate" => limits.mate_in = num(args, &mut i),
            "movetime" => limits.move_time_ms = num(args, &mut i),
            _ => {
                // Unknown extension token: skip it.
                i += 1;
            }
        }
    }
    limits
}

fn is_keyword(tok: &str) -> bool {
    matches!(
        tok,
        "searchmoves"
            | "ponder"
            | "infinite"
            | "wtime"
            | "btime"
            | "winc"
            | "binc"
            | "movestogo"
            | "depth"
            | "nodes"
            | "mate"
            | "movetime"
    )
}

/// Outcome of one synchronous search.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// Best move in UCI notation, or `"0000"` when no legal move exists.
    pub bestmove: String,
    pub depth: u64,
    pub nodes: u64,
    pub elapsed: Duration,
}

/// Run a bounded search and return the chosen move.
///
/// v0.1 policy: honour an optional `searchmoves` restriction, then play the
/// first legal move in sorted UCI order. Answers immediately; time controls
/// are accepted and ignored (returning early is always legal in UCI).
pub fn search(pos: &Position, limits: &GoLimits) -> SearchResult {
    let started = Instant::now();
    let mut moves: Vec<String> = pos
        .legal_moves_sorted()
        .iter()
        .map(ToString::to_string)
        .collect();
    if !limits.searchmoves.is_empty() {
        moves.retain(|m| limits.searchmoves.contains(m));
    }
    let bestmove = moves
        .into_iter()
        .next()
        .unwrap_or_else(|| "0000".to_string());
    SearchResult {
        bestmove,
        depth: 1,
        nodes: 1,
        elapsed: started.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_go_parses_to_defaults() {
        assert_eq!(parse_go(&[]), GoLimits::default());
    }

    #[test]
    fn full_go_parses() {
        let limits = parse_go(
            &"depth 6 nodes 1000 mate 3 movetime 500 wtime 60000 btime 60000 winc 500 binc 500 movestogo 30 infinite ponder searchmoves e2e4 d2d4"
                .split_whitespace()
                .collect::<Vec<_>>(),
        );
        assert_eq!(limits.depth, Some(6));
        assert_eq!(limits.nodes, Some(1000));
        assert_eq!(limits.mate_in, Some(3));
        assert_eq!(limits.move_time_ms, Some(500));
        assert_eq!(limits.white_time_ms, Some(60000));
        assert_eq!(limits.black_time_ms, Some(60000));
        assert_eq!(limits.white_inc_ms, Some(500));
        assert_eq!(limits.black_inc_ms, Some(500));
        assert_eq!(limits.moves_to_go, Some(30));
        assert!(limits.infinite && limits.ponder);
        assert_eq!(limits.searchmoves, vec!["e2e4", "d2d4"]);
    }

    #[test]
    fn unbounded_flags() {
        assert!(parse_go(&["infinite"]).is_unbounded());
        assert!(parse_go(&["ponder"]).is_unbounded());
        assert!(!parse_go(&["depth", "4"]).is_unbounded());
    }

    #[test]
    fn unknown_tokens_are_skipped() {
        let limits = parse_go(&["depth", "4", "frobnicator", "9"]);
        assert_eq!(limits.depth, Some(4));
    }

    #[test]
    fn search_picks_first_sorted_legal_move() {
        let pos = Position::startpos();
        let expected = pos
            .legal_moves_sorted()
            .first()
            .map(ToString::to_string)
            .unwrap();
        let result = search(&pos, &GoLimits::default());
        assert_eq!(result.bestmove, expected);
    }

    #[test]
    fn search_honours_searchmoves() {
        let pos = Position::startpos();
        let limits = parse_go(&["searchmoves", "g1f3", "e2e4"]);
        let result = search(&pos, &limits);
        // Sorted: e2e4 < g1f3, so e2e4 wins among the restriction.
        assert_eq!(result.bestmove, "e2e4");
    }

    #[test]
    fn search_reports_null_move_when_mated_or_stalemated() {
        // Stalemate: black king on h8, white queen f7 + king g6 cover
        // every escape; black to move has no legal moves.
        let pos = Position::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(pos.legal_move_count(), 0);
        let result = search(&pos, &GoLimits::default());
        assert_eq!(result.bestmove, "0000");
    }
}
