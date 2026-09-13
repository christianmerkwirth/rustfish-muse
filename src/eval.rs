//! Static evaluation: material plus tapered piece-square tables.
//!
//! Scores are centipawns from White's point of view. Middlegame and endgame
//! tables are interpolated by game phase (piece count), so kings shelter
//! early and centralise late. Tables are written rank 1 first; black pieces
//! mirror vertically.

use shakmaty::{Bitboard, Board, Color, File, Piece, Rank, Role, Square};

pub const TEMPO_BONUS: i32 = 12;

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

/// Mobility weight (mg, eg) per reachable square for each role.
fn mobility_weight(role: Role) -> (i32, i32) {
    match role {
        Role::Knight => (4, 2),
        Role::Bishop => (5, 3),
        Role::Rook => (2, 4),
        Role::Queen => (1, 2),
        _ => (0, 0),
    }
}

/// King-attacker weight (mg) by attacking role; endgame uses a eighth of this.
fn attacker_weight(role: Role) -> i32 {
    match role {
        Role::Pawn => 10,
        Role::Knight | Role::Bishop => 20,
        Role::Rook => 30,
        Role::Queen => 45,
        Role::King => 0,
    }
}

const SHELTER_MG: i32 = 8;
const SHELTER_EG: i32 = 2;
const STORM_MG: i32 = -12;
const STORM_EG: i32 = -4;
const OPEN_FILE_MG: i32 = -15;
const OPEN_FILE_EG: i32 = -4;

pub const BISHOP_PAIR_MG: i32 = 30;
pub const BISHOP_PAIR_EG: i32 = 50;
pub const ROOK_OPEN_MG: i32 = 20;
pub const ROOK_OPEN_EG: i32 = 12;
pub const ROOK_SEMI_MG: i32 = 10;
pub const ROOK_SEMI_EG: i32 = 6;
pub const ISOLATED_MG: i32 = -12;
pub const ISOLATED_EG: i32 = -8;
pub const DOUBLED_MG: i32 = -14;
pub const DOUBLED_EG: i32 = -10;

/// Passed-pawn bonus by advancement (0 = home rank), tapered mg/eg.
const PASSED_MG: [i32; 8] = [0, 0, 5, 12, 25, 50, 90, 0];
const PASSED_EG: [i32; 8] = [0, 0, 10, 25, 50, 100, 160, 0];

fn color_index(color: Color) -> usize {
    match color {
        Color::White => 0,
        Color::Black => 1,
    }
}

/// Pawn shelter/storm/open-file/attacker score for one king, White POV sign
/// applied by the caller via `sign`.
fn king_safety(board: &Board, color: Color, king: Square) -> (i32, i32) {
    let mut mg = 0;
    let mut eg = 0;
    let kf = king.file() as i32;
    let kr = king.rank() as i32;
    let ahead = match color {
        Color::White => kr + 1,
        Color::Black => kr - 1,
    };
    let own_pawns = board.by_piece(Piece {
        color,
        role: Role::Pawn,
    });
    let foe_pawns = board.by_piece(Piece {
        color: color.other(),
        role: Role::Pawn,
    });
    for df in -1..=1 {
        let f = kf + df;
        if !(0..=7).contains(&f) {
            continue;
        }
        let file = File::new(f as u32);
        // No own pawn anywhere on a neighbouring file: king is exposed.
        if (own_pawns & file).is_empty() {
            mg += OPEN_FILE_MG;
            eg += OPEN_FILE_EG;
        }
        // Pawn shield one rank ahead of the king.
        if (0..8).contains(&ahead) {
            let shield = Square::from_coords(file, Rank::new(ahead as u32));
            if own_pawns.contains(shield) {
                mg += SHELTER_MG;
                eg += SHELTER_EG;
            } else if foe_pawns.contains(shield) {
                mg += STORM_MG;
                eg += STORM_EG;
            }
        }
    }
    // Enemy pieces bearing down on the king zone.
    let mut zone = Bitboard::EMPTY;
    for dr in -1..=1 {
        for df in -1..=1 {
            let (f, r) = (kf + df, kr + dr);
            if (0..8).contains(&f) && (0..8).contains(&r) {
                zone.add(Square::from_coords(
                    File::new(f as u32),
                    Rank::new(r as u32),
                ));
            }
        }
    }
    for role in [
        Role::Pawn,
        Role::Knight,
        Role::Bishop,
        Role::Rook,
        Role::Queen,
    ] {
        let mut attackers = board.by_piece(Piece {
            color: color.other(),
            role,
        });
        while let Some(sq) = attackers.pop_front() {
            if board.attacks_from(sq).intersects(zone) {
                let w = attacker_weight(role);
                mg -= w;
                eg -= w / 8;
            }
        }
    }
    (mg, eg)
}

/// Static score in centipawns from White's perspective for the given side to
/// move (tempo bonus goes to the mover).
pub fn evaluate(board: &Board, turn: Color) -> i32 {
    let mut mg: i32 = 0;
    let mut eg: i32 = 0;
    let mut phase: i32 = 0;
    let white_bb = board.white();
    let black_bb = board.black();
    let mut pawn_count = [[0i32; 8]; 2];
    let mut pawns: [(Color, i32, i32); 16] = [(Color::White, 0, 0); 16];
    let mut pawn_len = 0usize;
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
        // Piece mobility: reachable squares not occupied by own pieces.
        let (wm, we) = mobility_weight(role);
        if wm != 0 {
            let own = match color {
                Color::White => white_bb,
                Color::Black => black_bb,
            };
            let moves = board.attacks_from(sq).without(own).count() as i32;
            match color {
                Color::White => {
                    mg += wm * moves;
                    eg += we * moves;
                }
                Color::Black => {
                    mg -= wm * moves;
                    eg -= we * moves;
                }
            }
        }
        if role == Role::Pawn {
            let f = sq.file() as usize;
            let r = sq.rank() as usize;
            pawn_count[color_index(color)][f] += 1;
            if pawn_len < pawns.len() {
                pawns[pawn_len] = (color, f as i32, r as i32);
                pawn_len += 1;
            }
        }
    }
    // Pawn structure: isolated, doubled, passed.
    for &(color, f, r) in &pawns[..pawn_len] {
        let ci = color_index(color);
        let left = if f > 0 {
            pawn_count[ci][f as usize - 1]
        } else {
            0
        };
        let right = if f < 7 {
            pawn_count[ci][f as usize + 1]
        } else {
            0
        };
        let mut pmg = 0;
        let mut peg = 0;
        if left == 0 && right == 0 {
            pmg += ISOLATED_MG;
            peg += ISOLATED_EG;
        }
        let foe = color.other();
        // Rank-aware passed check: no foe pawn ahead on nearby files.
        let mut blocked = false;
        let mut foes = board.by_piece(Piece {
            color: foe,
            role: Role::Pawn,
        });
        while let Some(sq) = foes.pop_front() {
            let (ff, rr) = (sq.file() as i32, sq.rank() as i32);
            if (ff - f).abs() <= 1 {
                match color {
                    Color::White => {
                        if rr > r {
                            blocked = true;
                            break;
                        }
                    }
                    Color::Black => {
                        if rr < r {
                            blocked = true;
                            break;
                        }
                    }
                }
            }
        }
        if !blocked {
            let adv = match color {
                Color::White => r,
                Color::Black => 7 - r,
            }
            .clamp(0, 7) as usize;
            pmg += PASSED_MG[adv];
            peg += PASSED_EG[adv];
        }
        match color {
            Color::White => {
                mg += pmg;
                eg += peg;
            }
            Color::Black => {
                mg -= pmg;
                eg -= peg;
            }
        }
    }
    for (ci, files) in pawn_count.iter().enumerate() {
        let sign = if ci == 0 { 1 } else { -1 };
        for &count in files.iter() {
            let extra = count - 1;
            if extra > 0 {
                mg += sign * DOUBLED_MG * extra;
                eg += sign * DOUBLED_EG * extra;
            }
        }
    }
    // Bishop pair and rooks on open files.
    for color in [Color::White, Color::Black] {
        let sign = match color {
            Color::White => 1,
            Color::Black => -1,
        };
        let bishops = board
            .by_piece(Piece {
                color,
                role: Role::Bishop,
            })
            .count();
        if bishops >= 2 {
            mg += sign * BISHOP_PAIR_MG;
            eg += sign * BISHOP_PAIR_EG;
        }
        let own_pawns = board.by_piece(Piece {
            color,
            role: Role::Pawn,
        });
        let foe_pawns = board.by_piece(Piece {
            color: color.other(),
            role: Role::Pawn,
        });
        let mut rooks = board.by_piece(Piece {
            color,
            role: Role::Rook,
        });
        while let Some(sq) = rooks.pop_front() {
            let file = File::new(sq.file() as u32);
            if (own_pawns & file).is_empty() {
                if (foe_pawns & file).is_empty() {
                    mg += sign * ROOK_OPEN_MG;
                    eg += sign * ROOK_OPEN_EG;
                } else {
                    mg += sign * ROOK_SEMI_MG;
                    eg += sign * ROOK_SEMI_EG;
                }
            }
        }
        if let Some(king) = board.king_of(color) {
            let (smg, seg) = king_safety(board, color, king);
            mg += sign * smg;
            eg += sign * seg;
        }
    }
    // Tempo: side to move gets a small bonus.
    match turn {
        Color::White => {
            mg += TEMPO_BONUS;
            eg += TEMPO_BONUS;
        }
        Color::Black => {
            mg -= TEMPO_BONUS;
            eg -= TEMPO_BONUS;
        }
    }
    let phase = phase.min(24);
    (mg * phase + eg * (24 - phase)) / 24
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Position;

    fn eval_fen(fen: &str) -> i32 {
        let pos = Position::from_fen(fen).unwrap();
        evaluate(pos.board(), pos.turn())
    }

    #[test]
    fn startpos_scores_tempo_only() {
        // Symmetric material: only the side-to-move tempo bonus remains.
        assert_eq!(
            eval_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
            TEMPO_BONUS
        );
        assert_eq!(
            eval_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1"),
            -TEMPO_BONUS
        );
    }

    #[test]
    fn tempo_is_exact_side_to_move_bonus() {
        let w = eval_fen("4k3/8/8/8/8/8/8/4K2R w K - 0 1");
        let b = eval_fen("4k3/8/8/8/8/8/8/4K2R b K - 0 1");
        assert_eq!(w - b, 2 * TEMPO_BONUS);
    }

    #[test]
    fn isolated_pawn_penalised() {
        // Lone d4 pawn vs c4+d4: black blockers on d7+e7 stop both sides
        // being passed, so the diff is the isolated penalty (-8 eg) plus
        // the extra pawn's PST (c4 = 18 eg): 26 in all.
        let lone = eval_fen("4k3/3pp3/8/8/3P4/8/8/4K3 w - - 0 1");
        let supported = eval_fen("4k3/3pp3/8/8/2PP4/8/8/4K3 w - - 0 1");
        let diff = supported - lone - PAWN_VALUE;
        assert!(
            (20..=32).contains(&diff),
            "isolated penalty + c-pawn PST should be ~26, got {diff}"
        );
    }

    #[test]
    fn doubled_pawns_penalised() {
        // d4+d3 (doubled extra + both isolated) vs d4+e4 (clean); black
        // blockers on d7+e7 cancel the passed bonuses on both sides.
        let doubled = eval_fen("4k3/3pp3/8/8/3P4/3P4/8/4K3 w - - 0 1");
        let clean = eval_fen("4k3/3pp3/8/8/2PP4/8/8/4K3 w - - 0 1");
        let diff = clean - doubled;
        assert!(
            diff > 25,
            "doubled+isolated pawns should cost 25+, got {diff}"
        );
    }

    #[test]
    fn passed_pawn_rewarded() {
        // White pawn on d6, no black pawn ahead: passed bonus (eg 100).
        let passed = eval_fen("4k3/8/3P4/8/8/8/8/4K3 w - - 0 1");
        // Same pawn blocked by a black pawn on d7: no bonus.
        let blocked = eval_fen("4k3/3p4/3P4/8/8/8/8/4K3 w - - 0 1");
        // Blocked adds a black pawn: -100 material, +pawn PST, -passed bonus.
        let diff = passed - blocked - PAWN_VALUE;
        assert!(
            (60..=140).contains(&diff),
            "passed bonus on 6th should be ~100, got {diff}"
        );
    }

    #[test]
    fn bishop_pair_rewarded() {
        // White two bishops vs black knight: material +340, pair +50 (eg).
        let pair = eval_fen("4k3/8/8/3n4/8/8/8/2B1KB2 w - - 0 1");
        let material = 2 * BISHOP_VALUE - KNIGHT_VALUE;
        let diff = pair - material;
        assert!(
            (20..=80).contains(&diff),
            "bishop pair should be ~50, got {diff}"
        );
    }

    #[test]
    fn rook_open_file_rewarded() {
        // Rook on d1, d-file empty: open-file bonus (mostly eg: +12).
        // Same rook with a black pawn on d7: semi-open (+6) instead.
        let open = eval_fen("4k3/8/8/8/8/8/8/3RK3 w - - 0 1");
        let semi = eval_fen("4k3/3p4/8/8/8/8/8/3RK3 w - - 0 1");
        // Semi adds a black pawn: -100 material plus its PST.
        let diff = open - semi - PAWN_VALUE;
        assert!(
            (-5..=25).contains(&diff),
            "open-vs-semi rook bonus should be small positive, got {diff}"
        );
        assert!(
            open > ROOK_VALUE,
            "open rook should beat bare material, got {open}"
        );
    }

    #[test]
    fn king_shelter_rewarded() {
        // Kg1 + f2/g2/h2 pawns vs bare Kg1: 3x shelter (+2 eg each).
        let sheltered = eval_fen("6k1/8/8/8/8/8/5PPP/6K1 w - - 0 1");
        let bare = eval_fen("6k1/8/8/8/8/8/8/6K1 w - - 0 1");
        let diff = sheltered - bare - 3 * PAWN_VALUE;
        assert!(
            (20..=50).contains(&diff),
            "shelter+PST should be ~36, got {diff}"
        );
    }

    #[test]
    fn centralised_knight_beats_cornered() {
        // d4 knight (8 moves) vs a1 knight (2 moves): mobility + PST.
        let centre = eval_fen("4k3/8/8/8/3N4/8/8/4K3 w - - 0 1");
        let corner = eval_fen("4k3/8/8/8/8/8/8/N3K3 w - - 0 1");
        let diff = centre - corner;
        assert!(
            diff > 20,
            "central knight should clearly beat cornered, got {diff}"
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
        // Mirrored position with the move flipped too, so tempo negates as well.
        let black = eval_fen("4k3/8/8/4n3/8/8/8/4K3 b - - 0 1");
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
