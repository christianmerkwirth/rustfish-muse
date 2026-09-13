//! Static evaluation: material plus tapered piece-square tables.
//!
//! Scores are centipawns from White's point of view. Middlegame and endgame
//! tables are interpolated by game phase (piece count), so kings shelter
//! early and centralise late. Tables are written rank 1 first; black pieces
//! mirror vertically.

use shakmaty::{Board, Color, Piece, Role, Square};

pub const PAWN_VALUE: i32 = 100;
pub const KNIGHT_VALUE: i32 = 320;
pub const BISHOP_VALUE: i32 = 330;
pub const ROOK_VALUE: i32 = 500;
pub const QUEEN_VALUE: i32 = 900;

const MG: usize = 0;
const EG: usize = 1;

type Tables = [[[i32; 64]; 2]; 7];

/// Piece-square tables indexed `[role as usize][phase][square]`.
/// Square 0 is a1, square 63 is h8, always from White's perspective.
#[rustfmt::skip]
const PST: Tables = [
    [[0; 64]; 2], // placeholder for Role index 0 (unused)
    // Pawn
    [[
          0,   0,   0,   0,   0,   0,   0,   0,
          5,  10,  10, -20, -20,  10,  10,   5,
          5,  -5, -10,   0,   0, -10,  -5,   5,
          0,   0,   0,  20,  20,   0,   0,   0,
          5,   5,  10,  25,  25,  10,   5,   5,
         10,  10,  20,  30,  30,  20,  10,  10,
         50,  50,  50,  50,  50,  50,  50,  50,
          0,   0,   0,   0,   0,   0,   0,   0,
    ], [
          0,   0,   0,   0,   0,   0,   0,   0,
          5,   5,   5,   5,   5,   5,   5,   5,
         10,  10,  12,  12,  12,  12,  10,  10,
         15,  15,  18,  25,  25,  18,  15,  15,
         20,  20,  25,  35,  35,  25,  20,  20,
         30,  30,  35,  45,  45,  35,  30,  30,
         60,  60,  60,  60,  60,  60,  60,  60,
          0,   0,   0,   0,   0,   0,   0,   0,
    ]],
    // Knight
    [[
        -50, -40, -30, -30, -30, -30, -40, -50,
        -40, -20,   0,   0,   0,   0, -20, -40,
        -30,   0,  10,  15,  15,  10,   0, -30,
        -30,   5,  15,  20,  20,  15,   5, -30,
        -30,   0,  15,  20,  20,  15,   0, -30,
        -30,   5,  10,  15,  15,  10,   5, -30,
        -40, -20,   0,   5,   5,   0, -20, -40,
        -50, -40, -30, -30, -30, -30, -40, -50,
    ], [
        -50, -40, -30, -30, -30, -30, -40, -50,
        -40, -20,   0,   0,   0,   0, -20, -40,
        -30,   0,  10,  15,  15,  10,   0, -30,
        -30,   5,  15,  20,  20,  15,   5, -30,
        -30,   0,  15,  20,  20,  15,   0, -30,
        -30,   5,  10,  15,  15,  10,   5, -30,
        -40, -20,   0,   5,   5,   0, -20, -40,
        -50, -40, -30, -30, -30, -30, -40, -50,
    ]],
    // Bishop
    [[
        -20, -10, -10, -10, -10, -10, -10, -20,
        -10,   0,   0,   0,   0,   0,   0, -10,
        -10,   0,   5,  10,  10,   5,   0, -10,
        -10,   5,   5,  10,  10,   5,   5, -10,
        -10,   0,  10,  10,  10,  10,   0, -10,
        -10,  10,  10,  10,  10,  10,  10, -10,
        -10,   5,   0,   0,   0,   0,   5, -10,
        -20, -10, -10, -10, -10, -10, -10, -20,
    ], [
        -20, -10, -10, -10, -10, -10, -10, -20,
        -10,   0,   0,   0,   0,   0,   0, -10,
        -10,   0,   5,  10,  10,   5,   0, -10,
        -10,   5,   5,  10,  10,   5,   5, -10,
        -10,   0,  10,  10,  10,  10,   0, -10,
        -10,  10,  10,  10,  10,  10,  10, -10,
        -10,   5,   0,   0,   0,   0,   5, -10,
        -20, -10, -10, -10, -10, -10, -10, -20,
    ]],
    // Rook
    [[
          0,   0,   0,   0,   0,   0,   0,   0,
          5,  10,  10,  10,  10,  10,  10,   5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
          0,   0,   0,   5,   5,   0,   0,   0,
    ], [
          0,   0,   0,   0,   0,   0,   0,   0,
          5,  10,  10,  10,  10,  10,  10,   5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
          0,   0,   0,   5,   5,   0,   0,   0,
    ]],
    // Queen
    [[
        -20, -10, -10,  -5,  -5, -10, -10, -20,
        -10,   0,   0,   0,   0,   0,   0, -10,
        -10,   0,   5,   5,   5,   5,   0, -10,
         -5,   0,   5,   5,   5,   5,   0,  -5,
          0,   0,   5,   5,   5,   5,   0,  -5,
        -10,   5,   5,   5,   5,   5,   0, -10,
        -10,   0,   5,   0,   0,   0,   0, -10,
        -20, -10, -10,  -5,  -5, -10, -10, -20,
    ], [
        -20, -10, -10,  -5,  -5, -10, -10, -20,
        -10,   0,   0,   0,   0,   0,   0, -10,
        -10,   0,   5,   5,   5,   5,   0, -10,
         -5,   0,   5,   5,   5,   5,   0,  -5,
          0,   0,   5,   5,   5,   5,   0,  -5,
        -10,   5,   5,   5,   5,   5,   0, -10,
        -10,   0,   5,   0,   0,   0,   0, -10,
        -20, -10, -10,  -5,  -5, -10, -10, -20,
    ]],
    // King middlegame: shelter behind pawns, off open files.
    [[
        -30, -40, -40, -50, -50, -40, -40, -30,
        -30, -40, -40, -50, -50, -40, -40, -30,
        -30, -40, -40, -50, -50, -40, -40, -30,
        -30, -40, -40, -50, -50, -40, -40, -30,
        -20, -30, -30, -40, -40, -30, -30, -20,
        -10, -20, -20, -20, -20, -20, -20, -10,
         20,  20,   0,   0,   0,   0,  20,  20,
         20,  30,  10,   0,   0,  10,  30,  20,
    ],
    // King endgame: centralise.
    [
        -50, -40, -30, -20, -20, -30, -40, -50,
        -30, -20, -10,   0,   0, -10, -20, -30,
        -30, -10,  20,  30,  30,  20, -10, -30,
        -30, -10,  30,  40,  40,  30, -10, -30,
        -30, -10,  30,  40,  40,  30, -10, -30,
        -30, -10,  20,  30,  30,  20, -10, -30,
        -30, -30,   0,   0,   0,   0, -30, -30,
        -50, -30, -30, -30, -30, -30, -30, -50,
    ]],
];

fn piece_value(role: Role) -> i32 {
    match role {
        Role::Pawn => PAWN_VALUE,
        Role::Knight => KNIGHT_VALUE,
        Role::Bishop => BISHOP_VALUE,
        Role::Rook => ROOK_VALUE,
        Role::Queen => QUEEN_VALUE,
        Role::King => 0,
    }
}

/// Square index into PST (0 = a1). Black squares mirror vertically first.
fn pst_index(sq: Square, color: Color) -> usize {
    let sq = match color {
        Color::White => sq,
        Color::Black => sq.flip_vertical(),
    };
    sq.file() as usize + sq.rank() as usize * 8
}

/// Static score in centipawns from White's perspective.
pub fn evaluate(board: &Board) -> i32 {
    let mut mg: i32 = 0;
    let mut eg: i32 = 0;
    let mut phase: i32 = 0;
    for i in 0..64 {
        let sq = Square::new(i);
        let Some(Piece { color, role }) = board.piece_at(sq) else {
            continue;
        };
        let value = piece_value(role);
        let idx = pst_index(sq, color);
        let table = &PST[role as usize];
        let contrib_mg = value + table[MG][idx];
        let contrib_eg = value + table[EG][idx];
        match color {
            Color::White => {
                mg += contrib_mg;
                eg += contrib_eg;
            }
            Color::Black => {
                mg -= contrib_mg;
                eg -= contrib_eg;
            }
        }
        phase += match role {
            Role::Knight | Role::Bishop => 1,
            Role::Rook => 2,
            Role::Queen => 4,
            _ => 0,
        };
    }
    let phase = phase.min(24);
    (mg * phase + eg * (24 - phase)) / 24
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Position;

    fn eval_fen(fen: &str) -> i32 {
        evaluate(Position::from_fen(fen).unwrap().board())
    }

    #[test]
    fn startpos_scores_zero() {
        assert_eq!(
            eval_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
            0
        );
    }

    #[test]
    fn material_counts() {
        // White up a knight (black b8 knight missing), all else standard.
        let up_knight = eval_fen("r1bqkbnr/pppppppp/8/8/8/5N2/PPPPPPPP/RNBQKB1R w KQkq - 0 1");
        assert!(
            (200..=450).contains(&up_knight),
            "extra knight should score ~320, got {up_knight}"
        );
        let down_exchange =
            eval_fen("rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2");
        assert!(
            down_exchange.abs() < 150,
            "equal pawn swap should stay near zero, got {down_exchange}"
        );
    }

    #[test]
    fn black_pieces_mirror_white() {
        // Same piece on mirrored squares must contribute mirrored scores:
        // white knight on e4 == -(black knight on e5).
        let white = eval_fen("4k3/8/8/8/4N3/8/8/4K3 w - - 0 1");
        let black = eval_fen("4k3/8/8/4n3/8/8/8/4K3 w - - 0 1");
        // e4 (white) mirrors to e5 (black) vertically, so scores must negate.
        assert_eq!(white, -black, "white {white} vs black {black}");
    }

    #[test]
    fn king_centralises_in_endgame() {
        // Bare kings: centralised white king outscores a cornered one.
        let center = eval_fen("8/8/8/3K4/8/8/6k1/8 w - - 0 1");
        let corner = eval_fen("K7/8/8/8/8/8/6k1/8 w - - 0 1");
        assert!(center > corner, "center {center} vs corner {corner}");
    }
}
