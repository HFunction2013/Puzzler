//! The search engine.
//!
//! The puzzle is an exact-cover problem: every playable board cell has to be
//! covered by exactly one piece. The search uses the minimum-remaining-values
//! heuristic: at every node it picks the empty cell with the fewest feasible
//! placements. The root-level branches are split across worker threads.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::bitset::BitSet;
use crate::geom;
use crate::model::Puzzle;

#[derive(Clone, Debug)]
pub struct PlacementInfo {
    /// Index into the shared placements table.
    pub piece: usize,
    /// Board cell indices, ascending.
    pub cells: Vec<u32>,
    pub rotation: u8,
    pub mirrored: bool,
}

#[derive(Clone, Debug)]
pub struct Solution {
    /// Indices into the placements table.
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
    rotation: u8,
    mirrored: bool,
}

/// Read-only data shared across worker threads.
struct Shared {
    n: usize,
    board: BitSet,
    placements: Vec<Placement>,
    covering: Vec<Vec<u32>>,
    counts_init: Vec<u16>,
    areas: Vec<u32>,
    class_of: Vec<u16>,
    remaining_cells_init: u32,
    all_use: bool,
    syms: Vec<Vec<u32>>,
    opts: SolveOptions,
}

/// Per-thread mutable search state.
struct Worker {
    occupied: BitSet,
    counts: Vec<u16>,
    remaining_cells: u32,
    stack: Vec<u32>,
    labels: Vec<u16>,
    classes: HashMap<Vec<u16>, usize>,
    solutions: Vec<Solution>,
    raw_solutions: u64,
    nodes: u64,
    stop: bool,
    dp: Vec<u64>,
}

impl Worker {
    fn new(shared: &Shared) -> Self {
        Self {
            occupied: BitSet::new(shared.n),
            counts: shared.counts_init.clone(),
            remaining_cells: shared.remaining_cells_init,
            stack: Vec::with_capacity(shared.areas.len() * 2),
            labels: vec![0; shared.n],
            classes: HashMap::new(),
            solutions: Vec::new(),
            raw_solutions: 0,
            nodes: 0,
            stop: false,
            dp: Vec::new(),
        }
    }

    #[inline]
    fn is_stopped(&mut self, shared: &Shared, started: Instant, global_stop: &AtomicBool) -> bool {
        if self.stop || global_stop.load(Ordering::Relaxed) {
            return true;
        }
        if let Some(limit) = shared.opts.node_limit {
            if self.nodes >= limit {
                self.stop = true;
                return true;
            }
        }
        if self.nodes % 4096 == 0 {
            if let Some(limit) = shared.opts.time_limit {
                if started.elapsed() >= limit {
                    self.stop = true;
                    global_stop.store(true, Ordering::Relaxed);
                    return true;
                }
            }
        }
        false
    }

    fn search(&mut self, shared: &Shared, started: Instant, global_stop: &AtomicBool) {
        self.nodes += 1;
        if self.is_stopped(shared, started, global_stop) {
            return;
        }

        let empty = self.empty_count(shared);
        if empty == 0 {
            self.record(shared);
            return;
        }
        if self.remaining_cells < empty {
            return;
        }
        if shared.all_use && self.remaining_cells != empty {
            return;
        }
        if !shared.all_use && !self.fillable_sum(shared, empty) {
            return;
        }

        let chosen = self.choose_cell(shared);
        let Some(anchor) = chosen else {
            return;
        };

        let mut candidates: Vec<u32> = Vec::new();
        for &pid in &shared.covering[anchor] {
            let piece = shared.placements[pid as usize].piece as usize;
            if self.counts[piece] == 0 {
                continue;
            }
            if self.occupied.intersects(&shared.placements[pid as usize].mask) {
                continue;
            }
            candidates.push(pid);
        }
        candidates.sort_by(|&a, &b| {
            let pa = &shared.placements[a as usize];
            let pb = &shared.placements[b as usize];
            shared.areas[pb.piece as usize]
                .cmp(&shared.areas[pa.piece as usize])
                .then(pa.piece.cmp(&pb.piece))
                .then(pa.mask.words().cmp(pb.mask.words()))
        });

        for &pid in &candidates {
            if self.is_stopped(shared, started, global_stop) {
                return;
            }
            let piece = shared.placements[pid as usize].piece as usize;
            if self.counts[piece] == 0 {
                continue;
            }
            if self.occupied.intersects(&shared.placements[pid as usize].mask) {
                continue;
            }

            self.occupied.or_assign(&shared.placements[pid as usize].mask);
            self.counts[piece] -= 1;
            self.remaining_cells -= shared.areas[piece];
            self.stack.push(pid);

            self.search(shared, started, global_stop);

            self.stack.pop();
            self.remaining_cells += shared.areas[piece];
            self.counts[piece] += 1;
            self.occupied.xor_assign(&shared.placements[pid as usize].mask);
        }
    }

    fn choose_cell(&self, shared: &Shared) -> Option<usize> {
        let mut best_cell: Option<usize> = None;
        let mut best_count = usize::MAX;

        for (wi, (bw, ow)) in shared
            .board
            .words()
            .iter()
            .zip(self.occupied.words().iter())
            .enumerate()
        {
            let mut empty_bits = bw & !ow;
            while empty_bits != 0 {
                let bit = empty_bits.trailing_zeros() as usize;
                empty_bits &= empty_bits - 1;
                let cell = wi * 64 + bit;

                let mut count = 0usize;
                for &pid in &shared.covering[cell] {
                    let piece = shared.placements[pid as usize].piece as usize;
                    if self.counts[piece] == 0 {
                        continue;
                    }
                    if self.occupied.intersects(&shared.placements[pid as usize].mask) {
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
        }
        best_cell
    }

    #[inline]
    fn empty_count(&self, shared: &Shared) -> u32 {
        let mut total = 0;
        for (b, o) in shared.board.words().iter().zip(self.occupied.words().iter()) {
            total += (b & !o).count_ones();
        }
        total
    }

    fn fillable_sum(&mut self, shared: &Shared, target: u32) -> bool {
        let words = target as usize / 64 + 1;
        if self.dp.len() < words {
            self.dp.resize(words, 0);
        }
        for word in self.dp.iter_mut() {
            *word = 0;
        }
        self.dp[0] = 1;

        for (piece, &count) in self.counts.iter().enumerate() {
            let area = shared.areas[piece] as usize;
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

    fn record(&mut self, shared: &Shared) {
        self.raw_solutions += 1;

        for label in self.labels.iter_mut() {
            *label = 0;
        }
        for &pid in &self.stack {
            let placement = &shared.placements[pid as usize];
            let class = shared.class_of[placement.piece as usize];
            for cell in placement.mask.indices() {
                self.labels[cell as usize] = class + 1;
            }
        }

        let key: Vec<u16> = if shared.syms.len() > 1 {
            let mut best: Option<Vec<u16>> = None;
            for perm in &shared.syms {
                let mut candidate = vec![0u16; shared.n];
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

        if shared.opts.dedupe {
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

        if let Some(limit) = shared.opts.max_solutions {
            if self.solutions.len() >= limit {
                self.stop = true;
            }
        }
    }
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
                        rotation: orientation.rotation,
                        mirrored: orientation.mirrored,
                    });
                }
            }
        }
    }

    let areas: Vec<u32> = puzzle.pieces.iter().map(|p| p.area() as u32).collect();

    let mut shape_classes: HashMap<Vec<(u16, u16)>, u16> = HashMap::new();
    let mut class_of: Vec<u16> = Vec::with_capacity(puzzle.pieces.len());
    for piece in &puzzle.pieces {
        let next = shape_classes.len() as u16;
        let class = *shape_classes.entry(piece.shape.cells.clone()).or_insert(next);
        class_of.push(class);
    }

    let counts_init: Vec<u16> = puzzle.pieces.iter().map(|p| p.count).collect();
    let remaining_cells_init: u32 = counts_init
        .iter()
        .zip(areas.iter())
        .map(|(&c, &a)| c as u32 * a)
        .sum();
    let symmetries = geom::board_symmetries(&puzzle.board);

    let shared = Shared {
        n,
        board,
        placements,
        covering,
        counts_init,
        areas,
        class_of,
        remaining_cells_init,
        all_use: puzzle.all_use,
        syms: symmetries,
        opts: opts.clone(),
    };

    // If a playable cell is not covered by any placement at all, no solution.
    let mut covered = BitSet::new(n);
    for p in &shared.placements {
        covered.or_assign(&p.mask);
    }
    let premature = (0..n).any(|cell| shared.board.get(cell) && !covered.get(cell));

    let mut all_solutions: Vec<Solution> = Vec::new();
    let mut total_nodes = 0u64;
    let mut total_raw = 0u64;
    let mut truncated = false;

    // Wrap shared data in Arc so worker threads can share it.
    let shared = Arc::new(shared);
    let symmetry_group = shared.syms.len();

    if !premature {
        // --- pick the root cell and enumerate root-level candidates --------
        let root_worker = Worker::new(&shared);
        let root_cell = root_worker.choose_cell(&shared);

        if let Some(anchor) = root_cell {
            let mut root_candidates: Vec<u32> = Vec::new();
            for &pid in &shared.covering[anchor] {
                let piece = shared.placements[pid as usize].piece as usize;
                if root_worker.counts[piece] == 0 {
                    continue;
                }
                if root_worker.occupied.intersects(&shared.placements[pid as usize].mask) {
                    continue;
                }
                root_candidates.push(pid);
            }
            root_candidates.sort_by(|&a, &b| {
                let pa = &shared.placements[a as usize];
                let pb = &shared.placements[b as usize];
                shared.areas[pb.piece as usize]
                    .cmp(&shared.areas[pa.piece as usize])
                    .then(pa.piece.cmp(&pb.piece))
                    .then(pa.mask.words().cmp(pb.mask.words()))
            });

            // Decide thread count.
            let num_threads = thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(root_candidates.len());

            let global_stop = Arc::new(AtomicBool::new(false));

            if num_threads <= 1 || root_candidates.len() <= 1 {
                // Sequential path.
                let mut worker = Worker::new(&shared);
                for &pid in &root_candidates {
                    let piece = shared.placements[pid as usize].piece as usize;
                    worker.occupied.or_assign(&shared.placements[pid as usize].mask);
                    worker.counts[piece] -= 1;
                    worker.remaining_cells -= shared.areas[piece];
                    worker.stack.push(pid);
                    worker.search(&shared, started, &global_stop);
                    worker.stack.pop();
                    worker.remaining_cells += shared.areas[piece];
                    worker.counts[piece] += 1;
                    worker.occupied.xor_assign(&shared.placements[pid as usize].mask);
                    if worker.stop {
                        break;
                    }
                }
                total_nodes = worker.nodes;
                total_raw = worker.raw_solutions;
                truncated = worker.stop;
                all_solutions = worker.solutions;
            } else {
                // Parallel path: split root candidates across threads.
                let chunk_size = root_candidates.len().div_ceil(num_threads);
                let chunks: Vec<Vec<u32>> = root_candidates
                    .chunks(chunk_size)
                    .map(|c| c.to_vec())
                    .collect();

                let mut handles = Vec::new();
                for chunk in chunks {
                    let shared = shared.clone();
                    let global_stop = global_stop.clone();
                    handles.push(thread::spawn(move || {
                        let mut worker = Worker::new(&shared);
                        for &pid in &chunk {
                            if global_stop.load(Ordering::Relaxed) {
                                break;
                            }
                            let piece = shared.placements[pid as usize].piece as usize;
                            worker.occupied.or_assign(&shared.placements[pid as usize].mask);
                            worker.counts[piece] -= 1;
                            worker.remaining_cells -= shared.areas[piece];
                            worker.stack.push(pid);
                            worker.search(&shared, started, &global_stop);
                            worker.stack.pop();
                            worker.remaining_cells += shared.areas[piece];
                            worker.counts[piece] += 1;
                            worker.occupied.xor_assign(&shared.placements[pid as usize].mask);
                            if worker.stop {
                                global_stop.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        worker
                    }));
                }

                let mut merged_classes: HashMap<Vec<u16>, usize> = HashMap::new();
                for handle in handles {
                    let worker = handle.join().expect("worker thread panicked");
                    total_nodes += worker.nodes;
                    total_raw += worker.raw_solutions;
                    truncated = truncated || worker.stop;
                    // Merge solutions from this worker, re-deduping across threads.
                    for sol in worker.solutions {
                        if opts.dedupe {
                            let key = canonical_key(&shared, &sol);
                            if let Some(&idx) = merged_classes.get(&key) {
                                all_solutions[idx].multiplicity += sol.multiplicity;
                            } else {
                                merged_classes.insert(key, all_solutions.len());
                                all_solutions.push(sol);
                            }
                        } else {
                            all_solutions.push(sol);
                        }
                    }
                }
            }
        }
    }

    let placements_info: Vec<PlacementInfo> = shared
        .placements
        .iter()
        .map(|p| PlacementInfo {
            piece: p.piece as usize,
            cells: p.mask.indices(),
            rotation: p.rotation,
            mirrored: p.mirrored,
        })
        .collect();

    let stats = SolveStats {
        nodes: total_nodes,
        raw_solutions: total_raw,
        elapsed: started.elapsed(),
        truncated,
    };

    SolveResult {
        solutions: all_solutions,
        placements: placements_info,
        stats,
        symmetry_group,
    }
}

/// Recomputes the canonical label grid for a solution (used when merging
/// solutions from multiple threads, since each thread has its own dedup map).
fn canonical_key(shared: &Shared, sol: &Solution) -> Vec<u16> {
    let mut labels = vec![0u16; shared.n];
    for &pid in &sol.placements {
        let placement = &shared.placements[pid];
        let class = shared.class_of[placement.piece as usize];
        for cell in placement.mask.indices() {
            labels[cell as usize] = class + 1;
        }
    }
    if shared.syms.len() > 1 {
        let mut best = labels.clone();
        for perm in &shared.syms {
            let mut candidate = vec![0u16; shared.n];
            for (cell, &target) in perm.iter().enumerate() {
                candidate[target as usize] = labels[cell];
            }
            if candidate < best {
                best = candidate;
            }
        }
        best
    } else {
        labels
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
