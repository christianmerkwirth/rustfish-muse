//! Transposition table: power-of-two Zobrist hash map with depth-preferred
//! replacement, per-search generations, and mate-score ply adjustment.

use std::mem::size_of;

use shakmaty::Move;

use crate::search::MATE;

/// Score bound stored with an entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bound {
    Exact,
    Lower,
    Upper,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    key: u64,
    best: Option<Move>,
    score: i32,
    depth: i32,
    bound: Bound,
    generation: u8,
}

impl Entry {
    fn empty() -> Self {
        Self {
            key: 0,
            best: None,
            score: 0,
            depth: -1,
            bound: Bound::Exact,
            generation: 0,
        }
    }
}

/// A probe hit with the score already adjusted for the caller's ply.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub score: i32,
    pub depth: i32,
    pub bound: Bound,
    pub best: Option<Move>,
}

pub struct Table {
    entries: Vec<Entry>,
    mask: usize,
    generation: u8,
}

impl Table {
    /// Allocate roughly `size_mb` megabytes (rounded down to a power of two
    /// slots so indexing is a mask).
    pub fn new(size_mb: u32) -> Self {
        let bytes = size_mb.max(1) as usize * 1024 * 1024;
        let capacity = bytes / size_of::<Entry>();
        // Largest power of two that fits the budget.
        let mut slots = 1usize;
        while slots * 2 <= capacity {
            slots *= 2;
        }
        let mut table = Self {
            entries: vec![Entry::empty(); slots],
            mask: slots - 1,
            generation: 1,
        };
        table.clear();
        table
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Start a new search generation (wrapping; 0 stays reserved for empty).
    pub fn new_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
    }

    pub fn clear(&mut self) {
        self.entries.fill(Entry::empty());
    }

    pub fn resize(&mut self, size_mb: u32) {
        *self = Self::new(size_mb);
    }

    fn index(&self, key: u64) -> usize {
        (key as usize) & self.mask
    }

    /// Mate scores are stored relative to the position's distance from the
    /// root so a mate-in-3 found at one ply stays a mate-in-3 everywhere.
    fn store_score(score: i32, ply: usize) -> i32 {
        if score >= MATE - 1000 {
            score + ply as i32
        } else if score <= -(MATE - 1000) {
            score - ply as i32
        } else {
            score
        }
    }

    fn probe_score(score: i32, ply: usize) -> i32 {
        if score >= MATE - 1000 {
            score - ply as i32
        } else if score <= -(MATE - 1000) {
            score + ply as i32
        } else {
            score
        }
    }

    pub fn probe(&self, key: u64, ply: usize) -> Option<Hit> {
        let e = &self.entries[self.index(key)];
        if e.key != key || e.depth < 0 {
            return None;
        }
        Some(Hit {
            score: Self::probe_score(e.score, ply),
            depth: e.depth,
            bound: e.bound,
            best: e.best,
        })
    }

    /// Depth-preferred replacement: keep deeper entries and the current
    /// generation, overwrite the rest.
    pub fn store(
        &mut self,
        key: u64,
        best: Option<Move>,
        score: i32,
        depth: i32,
        bound: Bound,
        ply: usize,
    ) {
        let idx = self.index(key);
        let e = &mut self.entries[idx];
        if e.key != key || e.depth <= depth || e.generation != self.generation {
            *e = Entry {
                key,
                best,
                score: Self::store_score(score, ply),
                depth,
                bound,
                generation: self.generation,
            };
        }
    }

    /// Permill (0-1000) of sampled slots used by the current generation.
    pub fn hashfull(&self) -> u32 {
        let sample = self.len().min(1024);
        if sample == 0 {
            return 0;
        }
        let step = self.len() / sample;
        let used = (0..sample)
            .filter(|i| self.entries[i * step].generation == self.generation)
            .count();
        (used * 1000 / sample) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Position;

    fn key_of(fen: &str) -> u64 {
        Position::from_fen(fen).unwrap().key()
    }

    const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

    #[test]
    fn store_probe_roundtrip() {
        let mut tt = Table::new(1);
        let key = key_of(START);
        assert!(tt.probe(key, 0).is_none());
        tt.store(key, None, 123, 4, Bound::Exact, 0);
        let hit = tt.probe(key, 0).unwrap();
        assert_eq!((hit.score, hit.depth, hit.bound), (123, 4, Bound::Exact));
    }

    #[test]
    fn mate_scores_adjust_for_ply() {
        let mut tt = Table::new(1);
        let key = key_of(START);
        // Mate in 3 plies from the root (ply 0).
        tt.store(key, None, MATE - 3, 6, Bound::Exact, 0);
        // Five plies deeper the same mate is five plies closer.
        let hit = tt.probe(key, 5).unwrap();
        assert_eq!(hit.score, MATE - 3 - 5);
        // Mated scores mirror.
        tt.store(key, None, -(MATE - 7), 6, Bound::Exact, 2);
        let hit = tt.probe(key, 6).unwrap();
        assert_eq!(hit.score, -(MATE - 7) + (6 - 2));
    }

    #[test]
    fn replacement_prefers_deeper_entries() {
        let mut tt = Table::new(1);
        let key = key_of(START);
        tt.store(key, None, 10, 5, Bound::Exact, 0);
        tt.store(key, None, 20, 3, Bound::Exact, 0);
        assert_eq!(tt.probe(key, 0).unwrap().score, 10);
        tt.store(key, None, 30, 7, Bound::Exact, 0);
        assert_eq!(tt.probe(key, 0).unwrap().score, 30);
    }

    #[test]
    fn hashfull_tracks_generations() {
        let mut tt = Table::new(1);
        assert_eq!(tt.hashfull(), 0);
        // One entry in ~40k slots rarely lands in the 1024 sample, so fill
        // enough to guarantee sampled hits.
        for k in 1..3001u64 {
            tt.store(k, None, 1, 1, Bound::Exact, 0);
        }
        assert!(tt.hashfull() > 0);
        tt.clear();
        assert_eq!(tt.hashfull(), 0);
        tt.new_generation();
        for k in 1..3001u64 {
            tt.store(k, None, 1, 1, Bound::Exact, 0);
        }
        assert!(tt.hashfull() > 0);
    }

    #[test]
    fn resize_changes_capacity() {
        let mut tt = Table::new(1);
        let small = tt.len();
        tt.resize(16);
        assert!(tt.len() > small);
        assert_eq!(tt.hashfull(), 0);
    }
}
