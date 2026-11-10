//! The puzzle model: board shape, pieces and their effective options.

use crate::geom::{self, Orientation, Shape};

/// The region that has to be covered, trimmed to its bounding box.
#[derive(Clone, Debug)]
pub struct Board {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<bool>,
    /// Rows rendered with `#` (playable) and `.` (blocked).
    pub rows: Vec<String>,
    /// Where the trimmed box sat inside the file's original grid.
    pub origin_row: usize,
    pub origin_col: usize,
}

impl Board {
    pub fn cell_count(&self) -> usize {
        self.cells.iter().filter(|&&c| c).count()
    }

    #[inline]
    pub fn index(&self, row: usize, col: usize) -> usize {
        row * self.width + col
    }

    pub fn rectangle(width: usize, height: usize) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err(format!(
                "board_shape: rectangle{{width,height}} needs positive sizes, got {{{width},{height}}}"
            ));
        }
        let rows = vec!["#".repeat(width); height];
        Ok(Self {
            width,
            height,
            cells: vec![true; width * height],
            rows,
            origin_row: 0,
            origin_col: 0,
        })
    }

    /// Parses `#` / `.` rows and trims the surrounding empty border.
    pub fn from_rows(rows: &[String]) -> Result<Self, String> {
        let mut min_r = usize::MAX;
        let mut max_r = 0usize;
        let mut min_c = usize::MAX;
        let mut max_c = 0usize;

        for (r, row) in rows.iter().enumerate() {
            for (c, ch) in row.chars().enumerate() {
                match ch {
                    '#' => {
                        min_r = min_r.min(r);
                        max_r = max_r.max(r);
                        min_c = min_c.min(c);
                        max_c = max_c.max(c);
                    }
                    '.' | ' ' => {}
                    other => {
                        return Err(format!(
                            "board_shape: unexpected character {:?} at row {}, column {} \
                             (use '#' for playable cells and '.' for blocked ones)",
                            other,
                            r + 1,
                            c + 1
                        ));
                    }
                }
            }
        }
        if min_r == usize::MAX {
            return Err("board_shape: the board contains no '#' cells".to_string());
        }

        let width = max_c - min_c + 1;
        let height = max_r - min_r + 1;
        let mut cells = vec![false; width * height];
        let mut out_rows = Vec::with_capacity(height);
        for r in 0..height {
            let src = &rows[min_r + r];
            let mut line = String::with_capacity(width);
            for c in 0..width {
                let filled = src.chars().nth(min_c + c) == Some('#');
                cells[r * width + c] = filled;
                line.push(if filled { '#' } else { '.' });
            }
            out_rows.push(line);
        }

        Ok(Self {
            width,
            height,
            cells,
            rows: out_rows,
            origin_row: min_r,
            origin_col: min_c,
        })
    }

    /// True when every playable cell is 4-connected to the rest.
    pub fn is_connected(&self) -> bool {
        let start = match self.cells.iter().position(|&c| c) {
            Some(i) => i,
            None => return false,
        };
        let count = self.cell_count();
        let mut seen = vec![false; self.cells.len()];
        let mut stack = vec![start];
        seen[start] = true;
        let mut visited = 1usize;
        while let Some(idx) = stack.pop() {
            let (r, c) = (idx / self.width, idx % self.width);
            let neighbours = [
                (r.wrapping_sub(1), c),
                (r + 1, c),
                (r, c.wrapping_sub(1)),
                (r, c.wrapping_add(1)),
            ];
            for (nr, nc) in neighbours {
                if nr < self.height && nc < self.width {
                    let n = self.index(nr, nc);
                    if self.cells[n] && !seen[n] {
                        seen[n] = true;
                        visited += 1;
                        stack.push(n);
                    }
                }
            }
        }
        visited == count
    }
}

/// One piece type, with the options that are actually in force for it.
#[derive(Clone, Debug)]
pub struct Piece {
    pub id: String,
    pub shape: Shape,
    /// How many copies of this piece type exist.
    pub count: u16,
    pub can_rotate: bool,
    pub can_flip: bool,
    pub orientations: Vec<Orientation>,
}

impl Piece {
    pub fn area(&self) -> usize {
        self.shape.area()
    }
}

#[derive(Clone, Debug)]
pub struct Puzzle {
    pub name: String,
    pub board: Board,
    pub pieces: Vec<Piece>,
    pub all_use: bool,
    pub can_rotate: bool,
    pub can_flip: bool,
    /// The `piece_count:` value from the file, if it was given.
    pub piece_count_declared: Option<usize>,
}

impl Puzzle {
    pub fn total_piece_cells(&self) -> usize {
        self.pieces
            .iter()
            .map(|p| p.area() * p.count as usize)
            .sum()
    }

    /// Validates cross-piece invariants. Returns warnings; hard failures are errors.
    pub fn validate(&self) -> Result<Vec<String>, String> {
        let mut warnings = Vec::new();
        let board_cells = self.board.cell_count();
        if board_cells == 0 {
            return Err("board_shape: the board is empty".to_string());
        }
        if self.pieces.is_empty() {
            return Err("puzzle: no pieces were defined".to_string());
        }

        for piece in &self.pieces {
            if piece.area() == 0 {
                return Err(format!("piece {}: the shape is empty", piece.id));
            }
            if !geom::is_connected(&piece.shape.cells) {
                return Err(format!(
                    "piece {}: the shape is not 4-connected, so it cannot be a physical piece",
                    piece.id
                ));
            }
            if piece.area() > board_cells {
                return Err(format!(
                    "piece {}: covers {} cells but the board only has {}",
                    piece.id,
                    piece.area(),
                    board_cells
                ));
            }
            if piece.count == 0 {
                return Err(format!("piece {}: count must be at least 1", piece.id));
            }
        }

        let total = self.total_piece_cells();
        if total < board_cells {
            return Err(format!(
                "the pieces cover {} cells but the board needs {}: no arrangement can fill it",
                total, board_cells
            ));
        }
        if self.all_use && total != board_cells {
            return Err(format!(
                "all_use is true, so every piece must be placed, but the pieces cover {} cells \
                 while the board has {} (set all_use: false to allow unused pieces)",
                total, board_cells
            ));
        }
        if let Some(declared) = self.piece_count_declared {
            if declared != self.pieces.len() {
                warnings.push(format!(
                    "piece_count is {}, but {} piece entries were defined \
                     (piece_count counts piece types, not copies)",
                    declared,
                    self.pieces.len()
                ));
            }
        }
        if !self.board.is_connected() {
            warnings.push(
                "board_shape: the board is not 4-connected; it will be treated as a single puzzle \
                 only if the pieces can span the gap"
                    .to_string(),
            );
        }
        Ok(warnings)
    }
}
