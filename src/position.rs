//! Board position handling built on the `shakmaty` move generator.
//!
//! A [`Position`] owns a [`shakmaty::Chess`] state. Updates through the
//! `position` UCI command are atomic: if any move in the list is illegal the
//! whole command is rejected and the previous position is kept.

use shakmaty::fen::Fen;
use shakmaty::uci::UciMove;
use shakmaty::{
    Board, CastlingMode, Chess, Color, EnPassantMode, Move, Position as ShakmatyPosition,
};

/// Owned chess position.
#[derive(Clone, Debug)]
pub struct Position {
    inner: Chess,
}

impl Default for Position {
    fn default() -> Self {
        Self::startpos()
    }
}

impl Position {
    /// The standard initial position.
    pub fn startpos() -> Self {
        Self {
            inner: Chess::default(),
        }
    }

    /// Parse a FEN string (six fields, standard castling, e.g. from the
    /// `position fen ...` UCI command).
    pub fn from_fen(fen: &str) -> Result<Self, String> {
        let setup: Fen = fen
            .parse()
            .map_err(|e| format!("invalid FEN {fen:?}: {e}"))?;
        let inner: Chess = setup
            .into_position(CastlingMode::Standard)
            .map_err(|e| format!("invalid FEN {fen:?}: {e}"))?;
        Ok(Self { inner })
    }

    /// Render the current position as a FEN string.
    pub fn to_fen(&self) -> String {
        Fen::from_position(&self.inner, EnPassantMode::Legal).to_string()
    }

    /// Apply a sequence of UCI moves atomically. On the first illegal move an
    /// error is returned and `self` is unchanged.
    pub fn apply_uci_moves(&mut self, moves: &[String]) -> Result<(), String> {
        let mut next = self.inner.clone();
        for m in moves {
            let uci: UciMove = m
                .parse()
                .map_err(|e| format!("invalid UCI move {m:?}: {e}"))?;
            let mv = uci
                .to_move(&next)
                .map_err(|e| format!("illegal move {m:?} in position {}: {e}", self.to_fen()))?;
            next = next
                .play(mv)
                .map_err(|e| format!("illegal move {m:?}: {e}"))?;
        }
        self.inner = next;
        Ok(())
    }

    /// All legal moves, sorted by their UCI string so move choice is
    /// deterministic across runs and platforms.
    pub fn legal_moves_sorted(&self) -> Vec<UciMove> {
        let mut moves: Vec<UciMove> = self
            .inner
            .legal_moves()
            .iter()
            .map(|m| m.to_uci(CastlingMode::Standard))
            .collect();
        moves.sort_by_key(ToString::to_string);
        moves
    }

    /// Number of legal moves from the current position.
    /// Used by unit tests (production code consumes the move lists directly).
    #[allow(dead_code)]
    pub fn legal_move_count(&self) -> usize {
        self.inner.legal_moves().len()
    }

    /// Side to move.
    pub fn turn(&self) -> Color {
        self.inner.turn()
    }

    /// True when the side to move is in check.
    pub fn is_check(&self) -> bool {
        self.inner.is_check()
    }

    /// Halfmove clock (plies since last pawn move or capture).
    pub fn halfmoves(&self) -> u32 {
        self.inner.halfmoves()
    }

    /// True for checkmate, stalemate, insufficient material or the
    /// seventy-five-move rule — positions with no meaningful search.
    pub fn is_game_over(&self) -> bool {
        self.inner.is_game_over()
    }

    /// Direct board access for evaluation.
    pub fn board(&self) -> &Board {
        self.inner.board()
    }

    /// All legal moves in generation order.
    pub fn legal_moves(&self) -> Vec<Move> {
        self.inner.legal_moves().into_iter().collect()
    }

    /// The position after playing a legal move.
    pub fn play(&self, m: &Move) -> Option<Position> {
        self.inner.clone().play(*m).ok().map(|inner| Self { inner })
    }

    /// Access the underlying `shakmaty` position.
    pub fn inner(&self) -> &Chess {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startpos_has_20_legal_moves() {
        assert_eq!(Position::startpos().legal_move_count(), 20);
    }

    #[test]
    fn startpos_fen_roundtrip() {
        let pos = Position::startpos();
        assert_eq!(
            pos.to_fen(),
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"
        );
    }

    #[test]
    fn fen_parses_kiwipete_with_48_moves() {
        let pos = Position::from_fen(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        )
        .unwrap();
        assert_eq!(pos.legal_move_count(), 48);
    }

    #[test]
    fn invalid_fen_is_rejected() {
        assert!(Position::from_fen("not a fen").is_err());
        assert!(Position::from_fen("8/8/8/8/8/8/8/8 w - - 0 1").is_err());
    }

    #[test]
    fn legal_sequence_applies() {
        let mut pos = Position::startpos();
        pos.apply_uci_moves(&["e2e4".to_string(), "e7e5".to_string()])
            .unwrap();
        assert_eq!(
            pos.to_fen(),
            "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2"
        );
    }

    #[test]
    fn illegal_move_rejected_atomically() {
        let mut pos = Position::startpos();
        let before = pos.to_fen();
        // e2e5 is not a legal pawn push; the whole command must be rejected.
        let err = pos
            .apply_uci_moves(&["e2e4".to_string(), "e2e5".to_string()])
            .unwrap_err();
        assert!(err.contains("e2e5"), "unexpected error: {err}");
        assert_eq!(pos.to_fen(), before);
    }

    #[test]
    fn malformed_move_rejected_atomically() {
        let mut pos = Position::startpos();
        let before = pos.to_fen();
        assert!(pos.apply_uci_moves(&["zzz".to_string()]).is_err());
        assert_eq!(pos.to_fen(), before);
    }

    #[test]
    fn sorted_moves_are_deterministic() {
        let pos = Position::startpos();
        let a: Vec<String> = pos
            .legal_moves_sorted()
            .iter()
            .map(ToString::to_string)
            .collect();
        let b: Vec<String> = pos
            .legal_moves_sorted()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(a, b);
        let mut c = a.clone();
        c.sort();
        assert_eq!(a, c);
    }
}
