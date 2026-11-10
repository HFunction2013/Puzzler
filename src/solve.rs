//! The search engine.
//!
//! The puzzle is an exact-cover problem: every playable board cell has to be
//! covered by exactly one piece. Because the board is dense and pieces are
//! small, the classic "always extend the smallest uncovered cell" rule
//! (Algorithm X with the first uncovered column) beats a generic DLX
//! implementation: it needs no column headers, no dancing links and no row
//! bookkeeping.
//!
//! Concretely:
//!
//! * each board cell is a bit in a `BitSet`;
//! * each `(piece, orientation, position)` becomes one pre-computed mask;
//! * placements are bucketed by their lowest cell, since a placement that
//!   covers the first empty cell can never touch an earlier (already filled)
//!   cell, so its lowest cell must be exactly that cell;
//! * the search fills the first empty cell and only tries placements from that
//!   bucket, pruning with a cell-count invariant and a region-size check.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::bitset::BitSet;
use crate::geom;
use crate::model::Puzzle;

#[derive(Clone, Debug)]
pub struct PlacementInfo {
    /// Index into `Puzzle::pieces`.
    pub piece: usize,
    /// Board cell indices, ascending.
    pub cells: Vec<u32>,
    pub rotation: u8,
    pub mirrored: bool,
}

#[derive(Clone, Debug)]
pub struct Solution {
    /// Indices into `SolveResult::placements`.
    pub placements: Vec<usize>,
    /// How many raw layouts collapse into this one under board symmetries.
    pub multiplicity: u64,
}

#[derive(Clone, Debug)]
pub struct SolveOptions {
    pub max_solutions: Option<usize>,
    pub node_limit: Option<u64>,
    pub time_limit: Option<Duration>,
    /// Collapse solutions that differ only by a symmetry of the board.
    pub dedupe: bool,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            max_solutions: None,
            node_limit: None,
            time_limit: None,
            dedupe: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SolveStats {
    pub nodes: u64,
    pub raw_solutions: u64,
    pub elapsed: Duration,
    pub truncated: bool,
}

pub struct SolveResult {
    pub solutions: Vec<Solution>,
    pub placements: Vec<PlacementInfo>,
    pub stats: SolveStats,
    pub symmetry_group: usize,
}

struct Placement {
    piece: u16,
    mask: BitSet,
    cells: Vec<u32>,
    rotation: u8,
    mirrored: bool,
}

pub fn solve(puzzle: &Puzzle, opts: &SolveOptions) -> SolveResult {
    let started = Instant::now();
    let width = puzzle.board.width;
    let height = puzzle.board.height;
    let n = width * height;

    let mut board = BitSet::new(n);
    for (i, &playable) in puzzle.board.cells.iter().enumerate() {
        if playable {
            board.set(i);
        }
    }

    // --- build every placement ------------------------------------------------
    let mut placements: Vec<Placement> = Vec::new();
    let mut covering: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut seen_masks: HashSet<Vec<u64>> = HashSet::new();

    for (piece_index, piece) in puzzle.pieces.iter().enumerate() {
        seen_masks.clear();
        for orientation in &piece.orientations {
            let (ow, oh) = (orientation.width as usize, orientation.height as usize);
            if ow > width || oh > height {
                continue;
            }
            for dr in 0..=(height - oh) {
                for dc in 0..=(width - ow) {
                    let mut mask = BitSet::new(n);
                    let mut fits = true;
                    for &(r, c) in &orientation.cells {
                        let idx = (dr + r as usize) * width + (dc + c as usize);
                        if !board.get(idx) {
                            fits = false;
                            break;
                        }
                        mask.set(idx);
                    }
                    if !fits {
                        continue;
                    }
                    // Different orientations can describe the same placement;
                    // keep only the first representative.
                    if !seen_masks.insert(mask.words().to_vec()) {
                        continue;
                    }
                    let cells = mask.indices();
                    let pid = placements.len() as u32;
                    for &cell in &cells {
                        covering[cell as usize].push(pid);
                    }
                    placements.push(Placement {
                        piece: piece_index as u16,
                        mask,
                        cells,
                        rotation: orientation.rotation,
                        mirrored: orientation.mirrored,
                    });
                }
            }
        }
    }

    let areas: Vec<u32> = puzzle.pieces.iter().map(|p| p.area() as u32).collect();

    // Pieces with the same shape are physically interchangeable, so solutions
    // are compared on shape classes rather than on the user's piece ids.
    let mut shape_classes: HashMap<Vec<(u16, u16)>, u16> = HashMap::new();
    let mut class_of: Vec<u16> = Vec::with_capacity(puzzle.pieces.len());
    for piece in &puzzle.pieces {
        let next = shape_classes.len() as u16;
        let class = *shape_classes.entry(piece.shape.cells.clone()).or_insert(next);
        class_of.push(class);
    }

    let counts: Vec<u16> = puzzle.pieces.iter().map(|p| p.count).collect();
    let remaining_cells: u32 = counts
        .iter()
        .zip(areas.iter())
        .map(|(&c, &a)| c as u32 * a)
        .sum();
    let symmetries = geom::board_symmetries(&puzzle.board);

    let mut search = Search {
        n,
        board,
        occupied: BitSet::new(n),
        placements,
        covering,
        counts,
        areas,
        class_of,
        remaining_cells,
        all_use: puzzle.all_use,
        syms: symmetries,
        stack: Vec::with_capacity(puzzle.pieces.len() * 2),
        labels: vec![0; n],
        classes: HashMap::new(),
        solutions: Vec::new(),
        raw_solutions: 0,
        nodes: 0,
        stop: false,
        opts,
        started,
        dp: Vec::new(),
    };

    // If a playable cell is not covered by any placement at all, no solution
    // can exist; failing here skips the search entirely.
    let mut covered = BitSet::new(n);
    for placement in &search.placements {
        covered.or_assign(&placement.mask);
    }
    let premature = (0..n).any(|cell| search.board.get(cell) && !covered.get(cell));
    if !premature {
        search.run();
    }

    let placements_info: Vec<PlacementInfo> = search
        .placements
        .iter()
        .map(|p| PlacementInfo {
            piece: p.piece as usize,
            cells: p.cells.clone(),
            rotation: p.rotation,
            mirrored: p.mirrored,
        })
        .collect();

    let stats = SolveStats {
        nodes: search.nodes,
        raw_solutions: search.raw_solutions,
        elapsed: started.elapsed(),
        truncated: search.stop,
    };

    SolveResult {
        solutions: search.solutions,
        placements: placements_info,
        stats,
        symmetry_group: search.syms.len(),
    }
}

struct Search<'a> {
    n: usize,
    board: BitSet,
    occupied: BitSet,
    placements: Vec<Placement>,
    /// cell -> every placement that covers that cell (for MRV selection)
    covering: Vec<Vec<u32>>,
    counts: Vec<u16>,
    areas: Vec<u32>,
    class_of: Vec<u16>,
    remaining_cells: u32,
    all_use: bool,
    syms: Vec<Vec<u32>>,
    stack: Vec<u32>,
    labels: Vec<u16>,
    classes: HashMap<Vec<u16>, usize>,
    solutions: Vec<Solution>,
    raw_solutions: u64,
    nodes: u64,
    stop: bool,
    opts: &'a SolveOptions,
    started: Instant,
    dp: Vec<u64>,
}

impl<'a> Search<'a> {
    fn run(&mut self) {
        self.search();
    }

    #[inline]
    fn is_stopped(&mut self) -> bool {
        if self.stop {
            return true;
        }
        if let Some(limit) = self.opts.node_limit {
            if self.nodes >= limit {
                self.stop = true;
                return true;
            }
        }
        if self.nodes % 4096 == 0 {
            if let Some(limit) = self.opts.time_limit {
                if self.started.elapsed() >= limit {
                    self.stop = true;
                    return true;
                }
            }
        }
        false
    }

    fn search(&mut self) {
        self.nodes += 1;
        if self.is_stopped() {
            return;
        }

        let empty = self.empty_count();
        if empty == 0 {
            self.record();
            return;
        }
        if self.remaining_cells < empty {
            return;
        }
        if self.all_use && self.remaining_cells != empty {
            return;
        }
        if !self.all_use && !self.fillable_sum(empty) {
            return;
        }

        // Choose the most constrained empty cell: the one with the fewest
        // feasible placements covering it. This is the minimum-remaining-values
        // heuristic and prunes the search tree dramatically.
        let chosen = self.choose_cell();
        let Some(anchor) = chosen else {
            return;
        };

        // Collect the feasible placements covering the chosen cell. The list
        // is small (at most a few dozen placements per cell), so a local Vec
        // is cheaper than fighting the borrow checker across recursion.
        let mut candidates: Vec<u32> = Vec::new();
        for &pid in &self.covering[anchor] {
            let piece = self.placements[pid as usize].piece as usize;
            if self.counts[piece] == 0 {
                continue;
            }
            if self.occupied.intersects(&self.placements[pid as usize].mask) {
                continue;
            }
            candidates.push(pid);
        }
        // Bigger pieces first: they constrain the rest of the board the most.
        candidates.sort_by(|&a, &b| {
            let pa = &self.placements[a as usize];
            let pb = &self.placements[b as usize];
            self.areas[pb.piece as usize]
                .cmp(&self.areas[pa.piece as usize])
                .then(pa.piece.cmp(&pb.piece))
                .then(pa.cells.cmp(&pb.cells))
        });

        for &pid in &candidates {
            if self.is_stopped() {
                return;
            }
            let piece = self.placements[pid as usize].piece as usize;
            if self.counts[piece] == 0 {
                continue;
            }
            if self.occupied.intersects(&self.placements[pid as usize].mask) {
                continue;
            }

            self.occupied.or_assign(&self.placements[pid as usize].mask);
            self.counts[piece] -= 1;
            self.remaining_cells -= self.areas[piece];
            self.stack.push(pid);

            self.search();

            self.stack.pop();
            self.remaining_cells += self.areas[piece];
            self.counts[piece] += 1;
            self.occupied.xor_assign(&self.placements[pid as usize].mask);
        }
    }

    /// Picks the empty playable cell with the fewest feasible placements
    /// covering it. Returns `None` when there are no empty cells (the caller
    /// handles that) or when some empty cell has zero candidates, in which
    /// case the branch is dead.
    fn choose_cell(&self) -> Option<usize> {
        let mut best_cell: Option<usize> = None;
        let mut best_count = usize::MAX;

        for cell in 0..self.n {
            if !self.board.get(cell) || self.occupied.get(cell) {
                continue;
            }
            let mut count = 0usize;
            for &pid in &self.covering[cell] {
                let piece = self.placements[pid as usize].piece as usize;
                if self.counts[piece] == 0 {
                    continue;
                }
                if self.occupied.intersects(&self.placements[pid as usize].mask) {
                    continue;
                }
                count += 1;
                if count >= best_count {
                    break;
                }
            }
            if count == 0 {
                return None;
            }
            if count < best_count {
                best_count = count;
                best_cell = Some(cell);
            }
        }
        best_cell
    }

    /// Number of playable cells that are still empty.
    #[inline]
    fn empty_count(&self) -> u32 {
        let mut total = 0;
        for (b, o) in self.board.words().iter().zip(self.occupied.words().iter()) {
            total += (b & !o).count_ones();
        }
        total
    }

    /// When unused pieces are allowed, the empty cell count has to be reachable
    /// as a subset sum of the remaining pieces.
    fn fillable_sum(&mut self, target: u32) -> bool {
        let words = target as usize / 64 + 1;
        if self.dp.len() < words {
            self.dp.resize(words, 0);
        }
        for word in self.dp.iter_mut() {
            *word = 0;
        }
        self.dp[0] = 1;

        for (piece, &count) in self.counts.iter().enumerate() {
            let area = self.areas[piece] as usize;
            if area == 0 {
                continue;
            }
            for _ in 0..count {
                shift_left_or(&mut self.dp, area);
            }
        }

        let target = target as usize;
        self.dp[target / 64] & (1u64 << (target % 64)) != 0
    }

    fn record(&mut self) {
        self.raw_solutions += 1;

        for label in self.labels.iter_mut() {
            *label = 0;
        }
        for &pid in &self.stack {
            let placement = &self.placements[pid as usize];
            let class = self.class_of[placement.piece as usize];
            for &cell in &placement.cells {
                self.labels[cell as usize] = class + 1;
            }
        }

        // Canonical form: the lexicographically smallest label grid over all
        // symmetries of the board.
        let key: Vec<u16> = if self.syms.len() > 1 {
            let mut best: Option<Vec<u16>> = None;
            for perm in &self.syms {
                let mut candidate = vec![0u16; self.n];
                for (cell, &target) in perm.iter().enumerate() {
                    candidate[target as usize] = self.labels[cell];
                }
                if best.as_ref().is_none_or(|b| *b > candidate) {
                    best = Some(candidate);
                }
            }
            best.unwrap()
        } else {
            self.labels.clone()
        };

        let placements: Vec<usize> = self.stack.iter().map(|&p| p as usize).collect();

        if self.opts.dedupe {
            if let Some(&index) = self.classes.get(&key) {
                self.solutions[index].multiplicity += 1;
            } else {
                let index = self.solutions.len();
                self.classes.insert(key, index);
                self.solutions.push(Solution {
                    placements,
                    multiplicity: 1,
                });
            }
        } else {
            self.solutions.push(Solution {
                placements,
                multiplicity: 1,
            });
        }

        if let Some(limit) = self.opts.max_solutions {
            if self.solutions.len() >= limit {
                self.stop = true;
            }
        }
    }
}

fn shift_left_or(words: &mut [u64], shift: usize) {
    if shift == 0 {
        return;
    }
    let word_shift = shift / 64;
    let bit_shift = shift % 64;
    if word_shift >= words.len() {
        return;
    }
    for i in (0..words.len()).rev() {
        let mut value = words[i - word_shift] << bit_shift;
        if bit_shift > 0 && i > word_shift {
            value |= words[i - word_shift - 1] >> (64 - bit_shift);
        }
        words[i] |= value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn solve_text(text: &str, options: SolveOptions) -> (SolveResult, Puzzle) {
        let parsed = parse::parse_str(text).expect("puzzle should parse");
        let result = solve(&parsed.puzzle, &options);
        (result, parsed.puzzle)
    }

    fn every_cell_covered_once(puzzle: &Puzzle, result: &SolveResult, solution: &Solution) {
        let mut count = vec![0usize; puzzle.board.width * puzzle.board.height];
        for &pid in &solution.placements {
            for &cell in &result.placements[pid].cells {
                count[cell as usize] += 1;
            }
        }
        for (cell, &playable) in puzzle.board.cells.iter().enumerate() {
            if playable {
                assert_eq!(count[cell], 1, "cell {cell} must be covered exactly once");
            } else {
                assert_eq!(count[cell], 0, "blocked cell {cell} must stay empty");
            }
        }
    }

    #[test]
    fn fills_two_by_two_with_two_dominoes() {
        let text = r###"
puzzle:
  name: two dominoes
  board_shape: "rectangle{2,2}"
  can_rotate: true
  pieces:
    - id: a
      shape: "##"
      count: 2
"###;
        let (result, puzzle) = solve_text(text, SolveOptions::default());
        assert_eq!(result.stats.raw_solutions, 2, "two raw layouts");
        assert_eq!(result.solutions.len(), 1, "one layout up to rotation");
        assert_eq!(result.solutions[0].multiplicity, 2);
        every_cell_covered_once(&puzzle, &result, &result.solutions[0]);
    }

    #[test]
    fn identical_shapes_with_different_ids_merge() {
        let text = r###"
puzzle:
  board_shape: "rectangle{2,2}"
  can_rotate: true
  pieces:
    - id: a
      shape: "##"
    - id: b
      shape: "##"
"###;
        let (result, _) = solve_text(text, SolveOptions::default());
        assert_eq!(result.stats.raw_solutions, 4);
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(result.solutions[0].multiplicity, 4);
    }

    #[test]
    fn keeps_duplicates_when_dedupe_is_off() {
        let text = r###"
puzzle:
  board_shape: "rectangle{2,2}"
  can_rotate: true
  pieces:
    - id: a
      shape: "##"
      count: 2
"###;
        let options = SolveOptions {
            dedupe: false,
            ..SolveOptions::default()
        };
        let (result, _) = solve_text(text, options);
        assert_eq!(result.solutions.len(), 2);
        assert!(result.solutions.iter().all(|s| s.multiplicity == 1));
    }

    #[test]
    fn rotation_flag_is_respected() {
        // A vertical domino cannot fill a 2x1 board unless rotation is allowed.
        let text = r###"
puzzle:
  board_shape: "rectangle{2,1}"
  can_rotate: false
  pieces:
    - id: v
      shape: |
        #
        #
"###;
        let (result, _) = solve_text(text, SolveOptions::default());
        assert!(result.solutions.is_empty());

        let text = r###"
puzzle:
  board_shape: "rectangle{2,1}"
  can_rotate: true
  pieces:
    - id: v
      shape: |
        #
        #
"###;
        let (result, _) = solve_text(text, SolveOptions::default());
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn piece_option_overrides_puzzle_default() {
        // The puzzle allows rotation, but the piece itself does not.
        let text = r###"
puzzle:
  board_shape: "rectangle{1,2}"
  can_rotate: true
  pieces:
    - id: h
      can_rotate: false
      shape: "##"
"###;
        let (result, _) = solve_text(text, SolveOptions::default());
        assert!(result.solutions.is_empty());
    }

    #[test]
    fn solves_an_l_shaped_grid_board() {
        let text = r###"
puzzle:
  board_shape: |
    ##
    #.
  pieces:
    - id: ell
      shape: |
        ##
        #.
"###;
        let (result, puzzle) = solve_text(text, SolveOptions::default());
        assert_eq!(result.solutions.len(), 1);
        every_cell_covered_once(&puzzle, &result, &result.solutions[0]);
    }

    #[test]
    fn rejects_unfillable_area() {
        let text = r###"
puzzle:
  board_shape: "rectangle{3,1}"
  all_use: false
  pieces:
    - id: a
      shape: "##"
"###;
        assert!(parse::parse_str(text).is_err(), "pieces cannot fill the board");
    }

    #[test]
    fn rejects_all_use_with_surplus_pieces() {
        let text = r###"
puzzle:
  board_shape: "rectangle{2,2}"
  all_use: true
  pieces:
    - id: a
      shape: "##"
      count: 3
"###;
        assert!(parse::parse_str(text).is_err());
    }

    #[test]
    fn subset_search_when_all_use_is_false() {
        let text = r###"
puzzle:
  board_shape: "rectangle{2,2}"
  all_use: false
  can_rotate: true
  pieces:
    - id: domino
      shape: "##"
      count: 3
"###;
        let (result, puzzle) = solve_text(text, SolveOptions::default());
        assert_eq!(result.stats.raw_solutions, 2);
        assert_eq!(result.solutions.len(), 1);
        every_cell_covered_once(&puzzle, &result, &result.solutions[0]);
    }

    #[test]
    fn honors_the_solution_limit() {
        let text = r###"
puzzle:
  board_shape: "rectangle{4,3}"
  can_rotate: true
  pieces:
    - id: d
      shape: "##"
      count: 6
"###;
        let options = SolveOptions {
            max_solutions: Some(1),
            ..SolveOptions::default()
        };
        let (result, _) = solve_text(text, options);
        assert_eq!(result.solutions.len(), 1);
        assert!(result.stats.truncated);
    }
}
