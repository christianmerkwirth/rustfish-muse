//! Negamax alpha-beta search with iterative deepening, quiescence and
//! selective pruning.
//!
//! v0.3 strength: everything from v0.2 plus a transposition table (with mate
//! adjustment), null-move pruning, late-move reductions with PVS re-search,
//! reverse futility, razoring, check extensions and repetition detection.
//! Single-threaded and fully deterministic for fixed depth limits;
//! time-managed searches may complete different depths run to run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use shakmaty::{CastlingMode, Color, Move, Position as _, Role};

use crate::eval;
use crate::position::Position;
use crate::tt::{Bound, Table};

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
        if let Some(next) = pos.play_temp(&m) {
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

/// True when `key` already occurred twice on the path (game history plus the
/// search stack): the current position completes a threefold repetition, so
/// the search scores it a draw without descending further.
fn is_repetition(game_keys: &[u64], stack: &[u64], key: u64) -> bool {
    let mut seen = 0;
    for k in game_keys.iter().chain(stack.iter()) {
        if *k == key {
            seen += 1;
            if seen >= 2 {
                return true;
            }
        }
    }
    false
}

/// Null-move pruning is unsound in zugzwang-prone positions, so it requires
/// a non-pawn piece on the board for the side to move.
fn has_non_pawn_material(pos: &Position) -> bool {
    let material = pos.board().material_side(pos.turn());
    material[Role::Knight] + material[Role::Bishop] + material[Role::Rook] + material[Role::Queen]
        > 0
}

/// First aspiration window half-width, widened only by full-window research.
pub const ASPIRATION_DELTA: i32 = 25;
/// Deepening depth from which the previous score seeds an aspiration window.
pub const ASPIRATION_MIN_DEPTH: u32 = 5;

struct Search<'a> {
    control: &'a Control,
    emit: &'a dyn Fn(&str),
    tt: &'a mut Table,
    game_keys: &'a [u64],
    stack: Vec<u64>,
    nodes: u64,
    seldepth: u32,
    aspiration_fails: u64,
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
        let white_pov = eval::evaluate(pos.board(), pos.turn());
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

        let key = pos.key();
        if let Some(hit) = self.tt.probe(key, ply) {
            match hit.bound {
                Bound::Exact => return hit.score,
                Bound::Lower => {
                    if hit.score >= beta {
                        return hit.score;
                    }
                }
                Bound::Upper => {
                    if hit.score <= alpha {
                        return hit.score;
                    }
                }
            }
        }

        let alpha_orig = alpha;
        let in_check = pos.is_check();
        if !in_check {
            let stand_pat = self.evaluate_stm(pos);
            if stand_pat >= beta {
                self.tt.store(key, None, beta, 0, Bound::Lower, ply);
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
            let Some(next) = pos.play_temp(m) else {
                continue;
            };
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
        if !self.stopped {
            let bound = if alpha <= alpha_orig {
                Bound::Upper
            } else if alpha >= beta {
                Bound::Lower
            } else {
                Bound::Exact
            };
            self.tt.store(key, None, alpha, 0, bound, ply);
        }
        alpha
    }

    /// Full-width search with repetition detection. The stack push/pop lives
    /// here so every early return in the search body stays balanced.
    /// Eight parameters is the standard negamax shape (position, depth, ply,
    /// window, PV move, null-move rights); bundling them would obscure it.
    #[allow(clippy::too_many_arguments)]
    fn negamax(
        &mut self,
        pos: &Position,
        depth: i32,
        ply: usize,
        alpha: i32,
        beta: i32,
        pv_move: Option<&Move>,
        can_null: bool,
    ) -> i32 {
        self.nodes += 1;
        self.poll();
        // Truncate this ply's PV line up front: every early return below
        // (TT cutoff, pruning, repetition, stop) must leave an empty tail,
        // or parents copy a stale line from an older branch into their PV.
        self.pv_len[ply] = ply;
        if self.stopped || ply >= MAX_PLY {
            return alpha;
        }
        if is_repetition(self.game_keys, &self.stack, pos.key()) {
            return 0;
        }
        self.stack.push(pos.key());
        let score = self.negamax_search(pos, depth, ply, alpha, beta, pv_move, can_null);
        self.stack.pop();
        score
    }

    /// Same standard shape as [`Search::negamax`]; see there for the allow.
    #[allow(clippy::too_many_arguments)]
    fn negamax_search(
        &mut self,
        pos: &Position,
        mut depth: i32,
        ply: usize,
        mut alpha: i32,
        beta: i32,
        pv_move: Option<&Move>,
        can_null: bool,
    ) -> i32 {
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
        self.seldepth = self.seldepth.max(ply as u32);

        let in_check = pos.is_check();
        let key = pos.key();

        // Transposition table probe; the stored move orders first.
        let mut tt_move: Option<Move> = None;
        if let Some(hit) = self.tt.probe(key, ply) {
            tt_move = hit.best;
            if hit.depth >= depth {
                match hit.bound {
                    Bound::Exact => return hit.score,
                    Bound::Lower => {
                        if hit.score >= beta {
                            return hit.score;
                        }
                    }
                    Bound::Upper => {
                        if hit.score <= alpha {
                            return hit.score;
                        }
                    }
                }
            }
        }

        // Static eval feeds the pruning decisions. Skipped in check: the
        // evasions play out in full.
        let eval = if !in_check {
            Some(self.evaluate_stm(pos))
        } else {
            None
        };

        // Reverse futility: clearly ahead with little depth left.
        if let Some(e) = eval
            && (1..=2).contains(&depth)
            && beta < MATE - 1000
            && e - 120 * depth >= beta
        {
            return e;
        }

        // Razoring: at the frontier, a bad eval drops straight to quiescence.
        if let Some(e) = eval
            && depth == 1
            && alpha < MATE - 1000
            && e + 250 <= alpha
        {
            return self.quiescence(pos, ply, alpha, beta);
        }

        // Null-move pruning: passing must not refute a beta cutoff.
        if can_null
            && depth >= 3
            && !in_check
            && beta < MATE - 1000
            && let Some(e) = eval
            && e >= beta
            && has_non_pawn_material(pos)
            && let Some(null_pos) = pos.null_move()
        {
            let reduction = if depth > 6 { 3 } else { 2 };
            let score = -self.negamax(
                &null_pos,
                depth - 1 - reduction,
                ply + 1,
                -beta,
                -beta + 1,
                None,
                false,
            );
            if self.stopped {
                return 0;
            }
            if score >= beta {
                return score;
            }
        }

        // Check extension: see the forced reply one ply deeper.
        if in_check {
            depth += 1;
        }

        let mut moves = pos.legal_moves();
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        let ordered = tt_move.as_ref().or(pv_move);
        self.sort_moves(&mut moves, pos, ordered, ply);

        let alpha_orig = alpha;
        let mut best = -INF;
        let mut best_move: Option<Move> = None;
        let mut idx = 0;
        for m in &moves {
            let Some(next) = pos.play_temp(m) else {
                continue;
            };
            let quiet = !m.is_capture() && !m.is_promotion();
            // Frontier futility: hopeless quiets at depth 1 (captures tried).
            if idx > 0
                && depth == 1
                && !in_check
                && quiet
                && let Some(e) = eval
                && e + 200 <= alpha
            {
                idx += 1;
                continue;
            }
            let mut score;
            if idx == 0 {
                score = -self.negamax(&next, depth - 1, ply + 1, -beta, -alpha, None, true);
            } else {
                // Late-move reduction for quiet laggards, verified by research.
                let reduction = if idx >= 3 && depth >= 3 && quiet && !in_check {
                    1 + ((depth >= 7 && idx >= 12) as i32)
                } else {
                    0
                };
                score = -self.negamax(
                    &next,
                    depth - 1 - reduction,
                    ply + 1,
                    -alpha - 1,
                    -alpha,
                    None,
                    true,
                );
                if score > alpha && reduction > 0 {
                    score =
                        -self.negamax(&next, depth - 1, ply + 1, -alpha - 1, -alpha, None, true);
                }
                if score > alpha && score < beta {
                    score = -self.negamax(&next, depth - 1, ply + 1, -beta, -alpha, None, true);
                }
            }
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
                        if quiet {
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
            idx += 1;
        }
        if !self.stopped {
            let bound = if best <= alpha_orig {
                Bound::Upper
            } else if best >= beta {
                Bound::Lower
            } else {
                Bound::Exact
            };
            self.tt.store(key, best_move, best, depth, bound, ply);
        }
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
///
/// `tt` persists across calls (it is the engine's shared table) and gets a
/// fresh generation per search; game history comes from the position.
pub fn search(
    pos: &Position,
    limits: &GoLimits,
    control: &Control,
    tt: &mut Table,
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

    tt.new_generation();
    let mut searcher = Search {
        control,
        emit,
        tt,
        game_keys: pos.history(),
        stack: Vec::with_capacity(64),
        nodes: 0,
        seldepth: 0,
        aspiration_fails: 0,
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
        // Aspiration: when the previous iteration completed, its score seeds
        // a narrow window for the first root move; a fail low/high falls
        // back to a full-window research below.
        let mut beta_root = INF;
        let mut aspiring = false;
        if depth >= ASPIRATION_MIN_DEPTH && completed_depth + 1 == depth {
            alpha = score - ASPIRATION_DELTA;
            beta_root = score + ASPIRATION_DELTA;
            aspiring = true;
        }
        let mut best_this: Option<Move> = None;
        let mut score_this = -INF;
        // Principal variation for this iteration: best root move followed by
        // the child's PV line. Saved on improvement because later siblings
        // overwrite the shared triangular table.
        let mut pv_this: Vec<Move> = Vec::new();

        // Root split: transposition-table move, then previous best, rest ordered.
        let tt_root = searcher.tt.probe(pos.key(), 0).and_then(|hit| hit.best);
        let pv_move = tt_root.or_else(|| ordered.first().copied());
        searcher.sort_moves(&mut ordered, pos, pv_move.as_ref(), 0);

        let mut idx = 0;
        for m in &ordered {
            let Some(next) = pos.play_temp(m) else {
                continue;
            };
            // Principal-variation search at the root: full window first,
            // null windows with full-window research afterwards.
            let mut s = if idx == 0 {
                -searcher.negamax(&next, depth as i32 - 1, 1, -beta_root, -alpha, None, true)
            } else {
                -searcher.negamax(&next, depth as i32 - 1, 1, -alpha - 1, -alpha, None, true)
            };
            if searcher.stopped {
                break;
            }
            if idx == 0 && aspiring && (s <= alpha || s >= beta_root) {
                // Aspiration window failed: reset to the full window and get
                // the exact score, so later siblings search valid bounds.
                searcher.aspiration_fails += 1;
                alpha = -INF;
                beta_root = INF;
                aspiring = false;
                s = -searcher.negamax(&next, depth as i32 - 1, 1, -INF, INF, None, true);
                if searcher.stopped {
                    break;
                }
            }
            // Root beta is +INF, so any improvement over alpha needs the
            // exact full-window score.
            if idx > 0 && s > alpha {
                s = -searcher.negamax(&next, depth as i32 - 1, 1, -INF, -alpha, None, true);
                if searcher.stopped {
                    break;
                }
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
            idx += 1;
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
            // Persist the root so later searches (and deeper iterations via
            // the TT move) start from knowledge.
            searcher
                .tt
                .store(pos.key(), Some(bm), score, depth as i32, Bound::Exact, 0);
            (searcher.emit)(&format!(
                "info depth {depth} seldepth {} time {} nodes {} nps {} score {} hashfull {} pv {}",
                searcher.seldepth,
                elapsed.as_millis(),
                searcher.nodes,
                nps,
                format_score(white_pov),
                searcher.tt.hashfull(),
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

    /// Run a search with a fresh 1 MB table, returning result and info lines.
    fn run_search(pos: &Position, go: &[&str]) -> (CompletedSearch, Vec<String>) {
        let limits = parse_go(go);
        let mut tt = Table::new(1);
        let lines: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let result = search(pos, &limits, &no_control(), &mut tt, &|line| {
            lines.lock().unwrap().push(line.to_string());
        });
        (result, lines.lock().unwrap().clone())
    }

    fn search_depth(fen: Option<&str>, depth: u64) -> CompletedSearch {
        let pos = match fen {
            Some(f) => Position::from_fen(f).unwrap(),
            None => Position::startpos(),
        };
        run_search(&pos, &["depth", &depth.to_string()]).0
    }

    fn mates_with(pos: &Position, bestmove: &str) -> bool {
        let bm: shakmaty::uci::UciMove = bestmove.parse().unwrap();
        pos.play(&bm.to_move(pos.inner()).unwrap())
            .map(|played| played.inner().is_checkmate())
            .unwrap_or(false)
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
        let (result, _) = run_search(&pos, &["depth", "3"]);
        assert!(
            mates_with(&pos, &result.bestmove),
            "bestmove {} does not mate",
            result.bestmove
        );
        assert!(result.score.abs() >= MATE - 1000);
    }

    #[test]
    fn search_keeps_mate_through_aspiration_windows() {
        // Depth 8 runs several aspiration iterations (from depth 5) with
        // mate scores inside narrow windows; fail low/high research must
        // still return the forced mate, deterministically.
        let pos = Position::from_fen("1r1k4/pp2n1p1/2p3B1/4Q3/1q3P2/N4RP1/1PPPr3/2RK4 b - - 8 23")
            .unwrap();
        let (first, _) = run_search(&pos, &["depth", "8"]);
        let (second, _) = run_search(&pos, &["depth", "8"]);
        assert!(
            mates_with(&pos, &first.bestmove),
            "bestmove {} does not mate",
            first.bestmove
        );
        assert!(first.score.abs() >= MATE - 1000);
        assert_eq!(first.bestmove, second.bestmove);
        assert_eq!(first.score, second.score);
    }

    #[test]
    fn search_still_finds_mate_in_2_with_pruning() {
        // Pruning (null move, futility, LMR) must not hide forced mates.
        // Verified mate-in-2 positions from the bench suite.
        for fen in [
            "1n2kbnr/1p2p2P/8/r2p4/3P1Pb1/1PP5/7P/q1B1K3 b k - 0 21",
            "1n2q1r1/rbpp1ppk/8/pBp1p1B1/4P1P1/N1P2P2/PP1KN1P1/R2Q4 w - - 2 16",
        ] {
            let pos = Position::from_fen(fen).unwrap();
            let (result, _) = run_search(&pos, &["depth", "5"]);
            let bm: shakmaty::uci::UciMove = result.bestmove.parse().unwrap();
            let after = pos.play(&bm.to_move(pos.inner()).unwrap()).unwrap();
            // Either mates at once, or every reply still allows mate on move.
            let immediate = after.inner().is_checkmate();
            let replies = after.legal_moves();
            let forces = !replies.is_empty()
                && replies.iter().all(|reply| {
                    after
                        .play(reply)
                        .map(|p2| {
                            p2.legal_moves().iter().any(|m2| {
                                p2.play(m2)
                                    .map(|p3| p3.inner().is_checkmate())
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                });
            assert!(
                immediate || forces,
                "bestmove {} in {fen} does not force mate in 2",
                result.bestmove
            );
        }
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
        let (result, _) = run_search(&pos, &["searchmoves", "g1f3", "e2e4", "depth", "2"]);
        assert!(["g1f3", "e2e4"].contains(&result.bestmove.as_str()));
    }

    #[test]
    fn search_reports_null_move_when_mated_or_stalemated() {
        let pos = Position::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(pos.legal_move_count(), 0);
        let mut tt = Table::new(1);
        let result = search(&pos, &GoLimits::default(), &no_control(), &mut tt, &silent);
        assert_eq!(result.bestmove, "0000");
    }

    #[test]
    fn repetition_detector_counts_path_occurrences() {
        // Startpos history holds the key once (the game start itself).
        let pos = Position::startpos();
        let key = pos.key();
        // Seen once before: twofold, not yet a draw.
        assert!(!is_repetition(pos.history(), &[], key));
        // Seen twice before (game + path return): threefold, draw.
        assert!(is_repetition(pos.history(), &[key], key));
        assert!(is_repetition(pos.history(), &[key, key], key));
        assert!(!is_repetition(pos.history(), &[key], 0x1234_5678));
    }

    #[test]
    fn search_handles_repeated_positions() {
        // Knights out and back twice: the start repeats, and 5.Nf3 would
        // repeat a third time, so that line must score a draw, not crash.
        let mut pos = Position::startpos();
        pos.apply_uci_moves(
            &[
                "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
            ]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        )
        .unwrap();
        let (result, _) = run_search(&pos, &["depth", "3"]);
        let legal: Vec<String> = pos
            .legal_moves_sorted()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(legal.contains(&result.bestmove));
    }

    #[test]
    fn shared_table_cuts_repeat_search_nodes() {
        // Searching the same position twice with one table must reuse work:
        // fewer nodes the second time, same best move.
        let pos = Position::startpos();
        let limits = parse_go(&["depth", "4"]);
        let mut tt = Table::new(1);
        let first = search(&pos, &limits, &no_control(), &mut tt, &silent);
        let second = search(&pos, &limits, &no_control(), &mut tt, &silent);
        assert_eq!(first.bestmove, second.bestmove);
        assert!(
            second.nodes < first.nodes,
            "no reuse: {} vs {}",
            second.nodes,
            first.nodes
        );
    }

    #[test]
    fn search_emits_info_per_completed_iteration() {
        let pos = Position::startpos();
        let (result, lines) = run_search(&pos, &["depth", "2"]);
        assert_eq!(result.depth, 2);
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
    fn search_reports_legal_pv_lines() {
        // Every info PV must be playable move by move: stale triangular-table
        // entries (e.g. after a transposition-table cutoff) once leaked
        // illegal moves into the PV and ponder fields.
        for fen in [
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "r1bqk2r/pppp1ppp/2n2n2/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
            "r1bq1rk1/pp1pppbp/2n2np1/2p5/2B1P3/2NP1N2/PPP2PPP/R1BQ1RK1 w - - 0 8",
        ] {
            let pos = Position::from_fen(fen).unwrap();
            let (_, lines) = run_search(&pos, &["depth", "8"]);
            assert!(!lines.is_empty(), "no info lines for {fen}");
            for line in &lines {
                let Some(pv) = line.split_once(" pv ").map(|(_, pv)| pv) else {
                    continue;
                };
                let mut check = pos.clone();
                for token in pv.split_whitespace() {
                    let uci: shakmaty::uci::UciMove = token.parse().unwrap();
                    let m = uci.to_move(check.inner()).expect("legal PV move");
                    assert!(
                        check.legal_moves().contains(&m),
                        "illegal PV move {token} in '{line}' ({fen})"
                    );
                    check = check.play(&m).unwrap();
                }
            }
        }
    }

    #[test]
    fn search_respects_movetime() {
        let pos = Position::startpos();
        let (result, _) = run_search(&pos, &["movetime", "50"]);
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
