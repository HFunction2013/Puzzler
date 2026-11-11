//! JSON output (used for `--json` and `--out`).

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use serde::Serialize;

use crate::model::Puzzle;
use crate::solve::{PlacementInfo, SolveResult};

#[derive(Serialize)]
pub struct Report {
    generator: String,
    puzzle: PuzzleReport,
    stats: StatsReport,
    solutions: Vec<SolutionReport>,
}

#[derive(Serialize)]
struct PuzzleReport {
    name: String,
    piece_count: usize,
    all_use: bool,
    can_rotate: bool,
    can_flip: bool,
    board: BoardReport,
    pieces: Vec<PieceReport>,
}

#[derive(Serialize)]
struct BoardReport {
    width: usize,
    height: usize,
    cells: usize,
    origin: [usize; 2],
    shape: Vec<String>,
}

#[derive(Serialize)]
struct PieceReport {
    id: String,
    cells: usize,
    count: u16,
    can_rotate: bool,
    can_flip: bool,
    orientations: usize,
    shape: Vec<String>,
}

#[derive(Serialize)]
struct StatsReport {
    nodes: u64,
    raw_solutions: u64,
    solutions: usize,
    elapsed_ms: f64,
    truncated: bool,
    symmetry_group: usize,
}

#[derive(Serialize)]
struct SolutionReport {
    index: usize,
    symmetry_multiplicity: u64,
    grid: Vec<Vec<Option<String>>>,
    placements: Vec<PlacementReport>,
}

#[derive(Serialize)]
struct PlacementReport {
    piece: String,
    piece_index: usize,
    rotation_degrees: u16,
    flipped: bool,
    cells: Vec<[usize; 2]>,
}

pub fn build_report(puzzle: &Puzzle, result: &SolveResult) -> Report {
    let board = &puzzle.board;
    Report {
        generator: format!("puzzler {}", env!("CARGO_PKG_VERSION")),
        puzzle: PuzzleReport {
            name: puzzle.name.clone(),
            piece_count: puzzle.pieces.len(),
            all_use: puzzle.all_use,
            can_rotate: puzzle.can_rotate,
            can_flip: puzzle.can_flip,
            board: BoardReport {
                width: board.width,
                height: board.height,
                cells: board.cell_count(),
                origin: [board.origin_row, board.origin_col],
                shape: board.rows.clone(),
            },
            pieces: puzzle
                .pieces
                .iter()
                .map(|piece| PieceReport {
                    id: piece.id.clone(),
                    cells: piece.area(),
                    count: piece.count,
                    can_rotate: piece.can_rotate,
                    can_flip: piece.can_flip,
                    orientations: piece.orientations.len(),
                    shape: piece.shape.to_rows(),
                })
                .collect(),
        },
        stats: StatsReport {
            nodes: result.stats.nodes,
            raw_solutions: result.stats.raw_solutions,
            solutions: result.solutions.len(),
            elapsed_ms: result.stats.elapsed.as_secs_f64() * 1000.0,
            truncated: result.stats.truncated,
            symmetry_group: result.symmetry_group,
        },
        solutions: result
            .solutions
            .iter()
            .enumerate()
            .map(|(i, solution)| {
                let mut grid = vec![vec![None; board.width]; board.height];
                for &pid in &solution.placements {
                    let placement = &result.placements[pid];
                    let id = puzzle.pieces[placement.piece].id.clone();
                    for &cell in &placement.cells {
                        let (r, c) = (cell as usize / board.width, cell as usize % board.width);
                        grid[r][c] = Some(id.clone());
                    }
                }
                SolutionReport {
                    index: i + 1,
                    symmetry_multiplicity: solution.multiplicity,
                    grid,
                    placements: solution
                        .placements
                        .iter()
                        .map(|&pid| {
                            let placement: &PlacementInfo = &result.placements[pid];
                            PlacementReport {
                                piece: puzzle.pieces[placement.piece].id.clone(),
                                piece_index: placement.piece,
                                rotation_degrees: placement.rotation as u16 * 90,
                                flipped: placement.mirrored,
                                cells: placement
                                    .cells
                                    .iter()
                                    .map(|&cell| {
                                        [
                                            cell as usize % board.width,
                                            cell as usize / board.width,
                                        ]
                                    })
                                    .collect(),
                            }
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

pub fn write_to_file(path: &Path, puzzle: &Puzzle, result: &SolveResult, compress: bool) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    let report = build_report(puzzle, result);
    if compress {
        serde_json::to_writer(&mut writer, &report)?;
    } else {
        serde_json::to_writer_pretty(&mut writer, &report)?;
    }
    writer.write_all(b"\n")?;
    writer.flush()
}
