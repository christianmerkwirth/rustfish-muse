//! Negamax alpha-beta search with iterative deepening and quiescence.
//!
//! v0.2 strength: tapered piece-square evaluation, MVV-LVA capture ordering
//! plus killers/history/PV-move, capture quiescence with check evasions,
//! and clock/movetime/depth/nodes/mate limits. Single-threaded and fully
//! deterministic for fixed depth limits; time-managed searches may complete
//! different depths run to run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use shakmaty::{CastlingMode, Color, Move, Position as _, Role};

use crate::eval;
use crate::position::Position;

/// Score of a forced mate, minus plies to mate. `INF` bounds all scores.
pub const MATE: i32 = 100_000;
pub const INF: i32 = 1_000_000;

/// Maximum search ply (deepening + quiescence margin).
const MAX_PLY: usize = 128;
/// Stop-flag/time polling granularity in nodes.
const POLL_MASK: u64 = 1023;
/// Fallback thinking time for a bare `go` with no limits at all.
const DEFAULT_MOVE_TIME: Duration = Duration::from_millis(1000);

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
        if *i + 1 < args.len()
            && let Ok(v) = args[*i + 1].parse::<u64>()
        {
            *i += 2;
            return Some(v);
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

/// Cross-thread search control owned by the UCI loop.
pub struct Control {
    /// Set by `stop`/`quit`: unwind at the next poll point.
    pub stop: AtomicBool,
    /// Set by `ponderhit`: convert a ponder search into a timed one.
    pub ponderhit: AtomicBool,
}

impl Control {
    pub fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            ponderhit: AtomicBool::new(false),
        }
    }
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of one synchronous search. Depth/nodes/score/elapsed are consumed
/// by unit tests and future UCI reporters (info lines carry them today).
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct CompletedSearch {
    /// Best move in UCI notation, or `"0000"` when no legal move exists.
    pub bestmove: String,
    /// Ponder move (best reply from the principal variation), if known.
    pub ponder: Option<String>,
    /// Deepest fully completed iteration.
    pub depth: u32,
    pub nodes: u64,
    /// Score from the side-to-move's perspective, in centipawns.
    pub score: i32,
    pub elapsed: Duration,
}

/// Bulk-count leaf nodes: perft validation of move generation.
/// Exercised by unit tests; doubles as a movegen benchmark harness.
#[allow(dead_code)]
pub fn perft(pos: &Position, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = pos.inner().legal_moves();
    if depth == 1 {
        return moves.len() as u64;
    }
    let mut total = 0;
    for m in moves {
        if let Some(next) = pos.play(&m) {
            total += perft(&next, depth - 1);
        }
    }
    total
}

fn piece_value(role: Role) -> i32 {
    match role {
        Role::Pawn => 100,
        Role::Knight => 320,
        Role::Bishop => 330,
        Role::Rook => 500,
        Role::Queen => 900,
        Role::King => 10_000,
    }
}

/// MVV-LVA capture score with promotion and castling bonuses.
fn order_score(m: &Move, turn: Color, board: &shakmaty::Board) -> i32 {
    let mut score = 0;
    if m.is_capture() {
        let victim = m.capture().unwrap_or(Role::Pawn);
        // En passant reports no captured role; it is always a pawn.
        let victim_value = if m.is_en_passant() {
            100
        } else {
            piece_value(victim)
        };
        let attacker = board.role_at(m.from().unwrap_or(m.to()));
        let attacker_value = attacker.map(piece_value).unwrap_or(100);
        score += 1_000_000 + victim_value * 16 - attacker_value;
    }
    if m.is_promotion() {
        score += 900_000 + m.promotion().map(piece_value).unwrap_or(0);
    }
    if m.is_castle() {
        score += 50_000;
    }
    let _ = turn;
    score
}

struct Search<'a> {
    control: &'a Control,
    emit: &'a dyn Fn(&str),
    nodes: u64,
    seldepth: u32,
    deadline: Option<Instant>,
    node_limit: Option<u64>,
    max_depth: u32,
    mate_in: Option<u64>,
    stopped: bool,
    completed_depth: u32,
    killers: [[Option<Move>; 2]; MAX_PLY],
    history: [[[u64; 64]; 6]; 2],
    pv_len: [usize; MAX_PLY],
    pv: [[Option<Move>; MAX_PLY]; MAX_PLY],
}

impl<'a> Search<'a> {
    fn poll(&mut self) {
        if self.nodes & POLL_MASK != 0 {
            return;
        }
        // The external stop flag (stop/quit) is honoured only after the first
        // iteration completes, so every search yields a real result instead of
        // racing `quit` against thread startup. Limits still apply immediately.
        if self.completed_depth > 0 && self.control.stop.load(Ordering::Relaxed) {
            self.stopped = true;
            return;
        }
        if let Some(limit) = self.node_limit
            && self.nodes >= limit
        {
            self.stopped = true;
            return;
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            self.stopped = true;
        }
    }

    fn history_index(m: &Move, turn: Color) -> Option<(usize, usize)> {
        let color_idx = usize::from(turn == Color::Black);
        let piece_idx = (m.role() as usize).checked_sub(1)?;
        if piece_idx >= 6 {
            return None;
        }
        Some((color_idx, piece_idx))
    }

    fn move_score(&self, m: &Move, pos: &Position, pv_move: Option<&Move>, ply: usize) -> i64 {
        if pv_move == Some(m) {
            return i64::MAX;
        }
        let mut score = i64::from(order_score(m, pos.turn(), pos.board()));
        if !m.is_capture() && !m.is_promotion() {
            if let Some((c, p)) = Self::history_index(m, pos.turn()) {
                let to = m.to().file() as usize + m.to().rank() as usize * 8;
                score += self.history[c][p][to].min(1_000_000) as i64;
            }
            for k in self.killers[ply].iter().flatten() {
                if k == m {
                    score += 500_000;
                    break;
                }
            }
        }
        score
    }

    /// Sort moves in place, best first. Deterministic: ties keep generation
    /// order via stable sort on unique keys.
    fn sort_moves(&self, moves: &mut [Move], pos: &Position, pv_move: Option<&Move>, ply: usize) {
        let mut scored: Vec<(i64, usize)> = moves
            .iter()
            .enumerate()
            .map(|(i, m)| (self.move_score(m, pos, pv_move, ply), i))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        // Reorder through a scratch vector of clones (positions are small).
        let original = moves.to_vec();
        for (slot, (_, idx)) in scored.iter().enumerate() {
            moves[slot] = original[*idx];
        }
    }

    fn evaluate_stm(&self, pos: &Position) -> i32 {
        let white_pov = eval::evaluate(pos.board());
        match pos.turn() {
            Color::White => white_pov,
            Color::Black => -white_pov,
        }
    }

    fn quiescence(&mut self, pos: &Position, ply: usize, mut alpha: i32, beta: i32) -> i32 {
        self.nodes += 1;
        self.poll();
        if self.stopped || ply >= MAX_PLY {
            return alpha;
        }
        self.seldepth = self.seldepth.max(ply as u32);

        let in_check = pos.is_check();
        if !in_check {
            let stand_pat = self.evaluate_stm(pos);
            if stand_pat >= beta {
                return beta;
            }
            if stand_pat > alpha {
                alpha = stand_pat;
            }
        }

        let mut moves: Vec<Move> = if in_check {
            pos.legal_moves()
        } else {
            pos.legal_moves()
                .into_iter()
                .filter(|m| m.is_capture() || m.is_promotion())
                .collect()
        };
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { alpha };
        }
        self.sort_moves(&mut moves, pos, None, ply);
        for m in &moves {
            let Some(next) = pos.play(m) else { continue };
            let score = -self.quiescence(&next, ply + 1, -beta, -alpha);
            if self.stopped {
                return 0;
            }
            if score > alpha {
                alpha = score;
                if alpha >= beta {
                    break;
                }
            }
        }
        alpha
    }

    fn negamax(
        &mut self,
        pos: &Position,
        depth: i32,
        ply: usize,
        mut alpha: i32,
        beta: i32,
        pv_move: Option<&Move>,
    ) -> i32 {
        self.nodes += 1;
        self.poll();
        if self.stopped || ply >= MAX_PLY {
            return alpha;
        }
        self.seldepth = self.seldepth.max(ply as u32);

        if pos.is_game_over() || pos.halfmoves() >= 100 {
            return if pos.inner().is_checkmate() {
                -MATE + ply as i32
            } else {
                0
            };
        }
        if depth <= 0 {
            return self.quiescence(pos, ply, alpha, beta);
        }

        let mut moves = pos.legal_moves();
        if moves.is_empty() {
            return if pos.is_check() {
                -MATE + ply as i32
            } else {
                0
            };
        }
        self.sort_moves(&mut moves, pos, pv_move, ply);

        self.pv_len[ply] = ply;
        let mut best = -INF;
        let mut best_move: Option<Move> = None;
        for m in &moves {
            let Some(next) = pos.play(m) else { continue };
            let score = -self.negamax(&next, depth - 1, ply + 1, -beta, -alpha, None);
            if self.stopped {
                return 0;
            }
            if score > best {
                best = score;
                best_move = Some(*m);
                if score > alpha {
                    alpha = score;
                    // Update principal variation.
                    self.pv[ply][ply] = Some(*m);
                    let child_len = self.pv_len[ply + 1];
                    for i in (ply + 1)..child_len {
                        self.pv[ply][i] = self.pv[ply + 1][i];
                    }
                    self.pv_len[ply] = child_len;
                    if alpha >= beta {
                        // Beta cutoff: killers + history for quiet moves.
                        if !m.is_capture() && !m.is_promotion() {
                            if self.killers[ply][0].as_ref() != Some(m) {
                                self.killers[ply][1] = self.killers[ply][0];
                                self.killers[ply][0] = Some(*m);
                            }
                            if let Some((c, p)) = Self::history_index(m, pos.turn()) {
                                let to = m.to().file() as usize + m.to().rank() as usize * 8;
                                let bonus = (depth * depth) as u64;
                                self.history[c][p][to] = self.history[c][p][to]
                                    .saturating_add(bonus)
                                    .min(1_000_000_000);
                            }
                        }
                        break;
                    }
                }
            }
        }
        let _ = best_move;
        best
    }
}

/// Compute the thinking deadline from clock limits, from now.
fn compute_deadline(limits: &GoLimits, turn: Color, now: Instant) -> Option<Instant> {
    if let Some(ms) = limits.move_time_ms {
        return Some(
            now + Duration::from_millis(ms.max(1)).saturating_sub(Duration::from_millis(10)),
        );
    }
    let (left, inc) = match turn {
        Color::White => (limits.white_time_ms, limits.white_inc_ms.unwrap_or(0)),
        Color::Black => (limits.black_time_ms, limits.black_inc_ms.unwrap_or(0)),
    };
    let left = left?;
    let moves = limits.moves_to_go.unwrap_or(30).max(1);
    let alloc = left / moves + inc / 2;
    let alloc = alloc.min(left.saturating_sub(20).max(5));
    Some(now + Duration::from_millis(alloc.max(5)))
}

fn format_score(score: i32) -> String {
    if score.abs() >= MATE - 1000 {
        let plies = MATE - score.abs();
        let moves = (plies + 1) / 2;
        format!("mate {}", moves * score.signum())
    } else {
        format!("cp {score}")
    }
}

/// Run a bounded or unbounded search to completion.
///
/// Emits `info ...` lines through `emit` as iterations complete. Returns the
/// best move found; on early stop the best of the last completed iteration,
/// falling back to the first legal move when nothing completed.
pub fn search(
    pos: &Position,
    limits: &GoLimits,
    control: &Control,
    emit: &dyn Fn(&str),
) -> CompletedSearch {
    let started = Instant::now();
    // Deterministic root order: sorted UCI, converted back to moves.
    let mut root_moves: Vec<Move> = pos
        .legal_moves_sorted()
        .into_iter()
        .filter_map(|u| u.to_move(pos.inner()).ok())
        .collect();
    if !limits.searchmoves.is_empty() {
        root_moves.retain(|m| {
            limits
                .searchmoves
                .contains(&m.to_uci(CastlingMode::Standard).to_string())
        });
    }
    let fallback = root_moves
        .first()
        .map(|m| m.to_uci(CastlingMode::Standard).to_string())
        .unwrap_or_else(|| "0000".to_string());
    if root_moves.is_empty() {
        return CompletedSearch {
            bestmove: "0000".to_string(),
            ponder: None,
            depth: 0,
            nodes: 0,
            score: 0,
            elapsed: started.elapsed(),
        };
    }

    // Depth bound: explicit `depth`, else unbounded deepening (time/stop end it).
    let mut max_depth = limits.depth.unwrap_or(128).min(128) as u32;
    if let Some(mate_in) = limits.mate_in
        && limits.depth.is_none()
        && limits.nodes.is_none()
        && limits.move_time_ms.is_none()
        && limits.white_time_ms.is_none()
        && limits.black_time_ms.is_none()
    {
        // A lone `mate` limit must still terminate: mate in N needs at most
        // 2N-1 plies, plus one spare iteration to confirm nothing shorter.
        max_depth = max_depth.min(mate_in as u32 * 2 + 1);
    }

    let mut searcher = Search {
        control,
        emit,
        nodes: 0,
        seldepth: 0,
        deadline: None,
        node_limit: limits.nodes,
        max_depth,
        mate_in: limits.mate_in,
        stopped: false,
        completed_depth: 0,
        killers: std::array::from_fn(|_| [None, None]),
        history: [[[0; 64]; 6]; 2],
        pv_len: [0; MAX_PLY],
        pv: std::array::from_fn(|_| std::array::from_fn(|_| None)),
    };

    // Ponder searches run without a clock until ponderhit converts them.
    if !limits.is_unbounded() || control.ponderhit.load(Ordering::Relaxed) {
        searcher.deadline = compute_deadline(limits, pos.turn(), started);
    }
    if searcher.deadline.is_none()
        && !limits.is_unbounded()
        && limits.move_time_ms.is_none()
        && limits.white_time_ms.is_none()
        && limits.black_time_ms.is_none()
        && limits.depth.is_none()
        && limits.nodes.is_none()
        && limits.mate_in.is_none()
    {
        // Bare `go` with no limits at all: think briefly rather than forever.
        searcher.deadline = Some(started + DEFAULT_MOVE_TIME);
    }

    let mut bestmove = fallback.clone();
    let mut ponder = None;
    let mut completed_depth = 0u32;
    let mut score = 0i32;

    let mut ordered = root_moves.clone();
    let mut depth = 1u32;
    while depth <= searcher.max_depth && !searcher.stopped {
        // Ponder conversion: once ponderhit arrives, run on the clock.
        if limits.ponder && searcher.deadline.is_none() && control.ponderhit.load(Ordering::Relaxed)
        {
            searcher.deadline = compute_deadline(limits, pos.turn(), Instant::now());
        }

        let mut alpha = -INF;
        let mut best_this: Option<Move> = None;
        let mut score_this = -INF;
        // Principal variation for this iteration: best root move followed by
        // the child's PV line. Saved on improvement because later siblings
        // overwrite the shared triangular table.
        let mut pv_this: Vec<Move> = Vec::new();

        // Root split: previous best (PV) move first, rest ordered.
        let pv_move = ordered.first().cloned();
        searcher.sort_moves(&mut ordered, pos, pv_move.as_ref(), 0);

        for m in &ordered {
            let Some(next) = pos.play(m) else { continue };
            let s = -searcher.negamax(&next, depth as i32 - 1, 1, -INF, -alpha, None);
            if searcher.stopped {
                break;
            }
            if s > score_this {
                score_this = s;
                best_this = Some(*m);
                alpha = alpha.max(s);
                pv_this.clear();
                pv_this.push(*m);
                for i in 1..searcher.pv_len[1] {
                    if let Some(pm) = searcher.pv[1][i] {
                        pv_this.push(pm);
                    }
                }
            }
        }
        if searcher.stopped {
            break;
        }

        if let Some(bm) = best_this {
            // Promote to front for the next iteration (PV move).
            if let Some(idx) = ordered.iter().position(|m| m == &bm) {
                let mv = ordered.remove(idx);
                ordered.insert(0, mv);
            }
            bestmove = bm.to_uci(CastlingMode::Standard).to_string();
            score = score_this;
            completed_depth = depth;
            searcher.completed_depth = depth;

            let pv_line: Vec<String> = pv_this
                .iter()
                .map(|m| m.to_uci(CastlingMode::Standard).to_string())
                .collect();
            // Ponder = best reply from the principal variation.
            ponder = pv_line.get(1).cloned();

            let white_pov = if pos.turn() == Color::White {
                score
            } else {
                -score
            };
            let elapsed = started.elapsed();
            let nps = if elapsed.as_secs_f64() > 0.0 {
                (searcher.nodes as f64 / elapsed.as_secs_f64()) as u64
            } else {
                searcher.nodes
            };
            (searcher.emit)(&format!(
                "info depth {depth} seldepth {} time {} nodes {} nps {} score {} pv {}",
                searcher.seldepth,
                elapsed.as_millis(),
                searcher.nodes,
                nps,
                format_score(white_pov),
                pv_line.join(" ")
            ));
        }

        // Mate-distance stop: a forced mate within the requested bound ends it.
        if let Some(mate_in) = searcher.mate_in
            && score.abs() >= MATE - 1000
            && (MATE - score.abs() + 1) / 2 <= mate_in as i32
        {
            break;
        }
        // Exact mate found: deeper iterations cannot improve it.
        if score.abs() >= MATE - 1000 && depth >= (MATE - score.abs()) as u32 {
            break;
        }
        depth += 1;
    }

    CompletedSearch {
        bestmove,
        ponder,
        depth: completed_depth,
        nodes: searcher.nodes,
        score,
        elapsed: started.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_control() -> Control {
        Control::new()
    }

    fn silent(_: &str) {}

    fn search_depth(fen: Option<&str>, depth: u64) -> CompletedSearch {
        let pos = match fen {
            Some(f) => Position::from_fen(f).unwrap(),
            None => Position::startpos(),
        };
        search(
            &pos,
            &parse_go(&["depth", &depth.to_string()]),
            &no_control(),
            &silent,
        )
    }

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
    fn perft_startpos() {
        let pos = Position::startpos();
        assert_eq!(perft(&pos, 1), 20);
        assert_eq!(perft(&pos, 2), 400);
        assert_eq!(perft(&pos, 3), 8_902);
        assert_eq!(perft(&pos, 4), 197_281);
    }

    #[test]
    fn perft_kiwipete() {
        let pos = Position::from_fen(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        )
        .unwrap();
        assert_eq!(perft(&pos, 1), 48);
        assert_eq!(perft(&pos, 2), 2_039);
        assert_eq!(perft(&pos, 3), 97_862);
    }

    #[test]
    fn search_finds_mate_in_1() {
        // Verified mate-in-1 from the bench suite: black to play and mate.
        let pos = Position::from_fen("1r1k4/pp2n1p1/2p3B1/4Q3/1q3P2/N4RP1/1PPPr3/2RK4 b - - 8 23")
            .unwrap();
        let result = search(&pos, &parse_go(&["depth", "3"]), &no_control(), &silent);
        // The chosen move must deliver checkmate.
        let bm: shakmaty::uci::UciMove = result.bestmove.parse().unwrap();
        let played = pos.play(&bm.to_move(pos.inner()).unwrap()).unwrap();
        assert!(
            played.inner().is_checkmate(),
            "bestmove {} does not mate",
            result.bestmove
        );
        assert!(result.score.abs() >= MATE - 1000);
    }

    #[test]
    fn search_is_deterministic_at_fixed_depth() {
        let a = search_depth(None, 3);
        let b = search_depth(None, 3);
        assert_eq!(a.bestmove, b.bestmove);
        assert_eq!(a.score, b.score);
    }

    #[test]
    fn search_honours_searchmoves() {
        let pos = Position::startpos();
        let result = search(
            &pos,
            &parse_go(&["searchmoves", "g1f3", "e2e4", "depth", "2"]),
            &no_control(),
            &silent,
        );
        assert!(["g1f3", "e2e4"].contains(&result.bestmove.as_str()));
    }

    #[test]
    fn search_reports_null_move_when_mated_or_stalemated() {
        let pos = Position::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(pos.legal_move_count(), 0);
        let result = search(&pos, &GoLimits::default(), &no_control(), &silent);
        assert_eq!(result.bestmove, "0000");
    }

    #[test]
    fn search_emits_info_per_completed_iteration() {
        let pos = Position::startpos();
        let lines: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let result = search(&pos, &parse_go(&["depth", "2"]), &no_control(), &|line| {
            lines.lock().unwrap().push(line.to_string());
        });
        assert_eq!(result.depth, 2);
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.starts_with("info depth 1")),
            "missing depth-1 info: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("info depth 2")),
            "missing depth-2 info: {lines:?}"
        );
        assert!(lines.iter().all(|l| l.contains(" pv ")), "{lines:?}");
    }

    #[test]
    fn search_respects_movetime() {
        let pos = Position::startpos();
        let result = search(&pos, &parse_go(&["movetime", "50"]), &no_control(), &silent);
        assert!(
            result.elapsed < Duration::from_secs(3),
            "{:?}",
            result.elapsed
        );
        let legal: Vec<String> = pos
            .legal_moves_sorted()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(legal.contains(&result.bestmove));
    }
}
