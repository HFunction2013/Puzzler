//! Polyomino geometry: shape parsing, rotations/reflections, board symmetries.

use std::collections::HashSet;

use crate::model::Board;

/// A normalized polyomino: cells are `(row, col)` with the top-left corner at
/// `(0, 0)`, sorted row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    pub cells: Vec<(u16, u16)>,
    pub width: u16,
    pub height: u16,
}

impl Shape {
    pub fn area(&self) -> usize {
        self.cells.len()
    }

    /// Renders the shape back to text rows (`#` = filled, `.` = empty).
    pub fn to_rows(&self) -> Vec<String> {
        let mut rows = vec![vec!['.'; self.width as usize]; self.height as usize];
        for &(r, c) in &self.cells {
            rows[r as usize][c as usize] = '#';
        }
        rows.into_iter()
            .map(|row| row.into_iter().collect())
            .collect()
    }
}

/// Normalizes a set of cells expressed in an arbitrary integer frame.
fn normalize_i32(cells: &[(i32, i32)]) -> (Vec<(u16, u16)>, u16, u16) {
    let min_r = cells.iter().map(|c| c.0).min().unwrap();
    let min_c = cells.iter().map(|c| c.1).min().unwrap();
    let mut out: Vec<(u16, u16)> = cells
        .iter()
        .map(|&(r, c)| ((r - min_r) as u16, (c - min_c) as u16))
        .collect();
    out.sort_unstable();
    let width = out.iter().map(|c| c.1).max().unwrap() + 1;
    let height = out.iter().map(|c| c.0).max().unwrap() + 1;
    (out, width, height)
}

/// Parses a `"###\n.#."`-style shape description.
pub fn parse_shape_text(text: &str, what: &str) -> Result<Shape, String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized
        .split('\n')
        .map(|s| s.trim_end().to_string())
        .collect();

    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    if lines.is_empty() {
        return Err(format!("{what}: the shape is empty"));
    }

    // Strip the common indentation that YAML block scalars keep.
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);

    let mut cells = Vec::new();
    for (r, line) in lines.iter().enumerate() {
        let body: String = line.chars().skip(indent).collect();
        for (c, ch) in body.chars().enumerate() {
            match ch {
                '#' => cells.push((r as i32, c as i32)),
                '.' | ' ' => {}
                other => {
                    return Err(format!(
                        "{what}: unexpected character {:?} at row {}, column {} \
                         (use '#' for filled cells and '.' for empty ones)",
                        other,
                        r + 1,
                        c + 1
                    ));
                }
            }
        }
    }
    if cells.is_empty() {
        return Err(format!("{what}: the shape contains no '#' cells"));
    }

    let (cells, width, height) = normalize_i32(&cells);
    Ok(Shape {
        cells,
        width,
        height,
    })
}

/// True when the cells form a single 4-connected component.
pub fn is_connected(cells: &[(u16, u16)]) -> bool {
    if cells.is_empty() {
        return false;
    }
    let set: HashSet<(u16, u16)> = cells.iter().copied().collect();
    let mut seen: HashSet<(u16, u16)> = HashSet::new();
    let mut stack = vec![cells[0]];
    seen.insert(cells[0]);
    while let Some((r, c)) = stack.pop() {
        let neighbours = [
            (r.wrapping_sub(1), c),
            (r.wrapping_add(1), c),
            (r, c.wrapping_sub(1)),
            (r, c.wrapping_add(1)),
        ];
        for n in neighbours {
            if set.contains(&n) && seen.insert(n) {
                stack.push(n);
            }
        }
    }
    seen.len() == set.len()
}

/// One concrete orientation of a piece.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orientation {
    pub cells: Vec<(u16, u16)>,
    pub width: u16,
    pub height: u16,
    /// Quarter turns applied clockwise.
    pub rotation: u8,
    pub mirrored: bool,
}

/// Builds the distinct orientations of a shape.
///
/// Without `allow_flip` we never reflect, and without `allow_rotate` we only
/// emit the original orientation. Shapes whose symmetry makes two
/// transformations coincide are collapsed onto the first representative.
pub fn orientations(shape: &Shape, allow_rotate: bool, allow_flip: bool) -> Vec<Orientation> {
    let turns = if allow_rotate { 4 } else { 1 };
    let mirrors = if allow_flip { 2 } else { 1 };
    let mut seen: HashSet<Vec<(u16, u16)>> = HashSet::new();
    let mut out = Vec::new();

    for mirror in 0..mirrors {
        let mirrored = mirror == 1;
        for turn in 0..turns {
            let mut cells: Vec<(i32, i32)> = shape
                .cells
                .iter()
                .map(|&(r, c)| (r as i32, c as i32))
                .collect();
            if mirrored {
                for cell in cells.iter_mut() {
                    cell.1 = -cell.1;
                }
            }
            for _ in 0..turn {
                // 90 degrees clockwise around the origin.
                cells = cells.iter().map(|&(r, c)| (c, -r)).collect();
            }
            let (normalized, width, height) = normalize_i32(&cells);
            if seen.insert(normalized.clone()) {
                out.push(Orientation {
                    cells: normalized,
                    width,
                    height,
                    rotation: turn as u8,
                    mirrored,
                });
            }
        }
    }
    out
}

/// The eight symmetries of the square, as transformations of a `w x h` box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symmetry {
    Identity,
    Rot90,
    Rot180,
    Rot270,
    MirrorHorizontal,
    MirrorVertical,
    Transpose,
    AntiTranspose,
}

impl Symmetry {
    pub const ALL: [Symmetry; 8] = [
        Symmetry::Identity,
        Symmetry::Rot90,
        Symmetry::Rot180,
        Symmetry::Rot270,
        Symmetry::MirrorHorizontal,
        Symmetry::MirrorVertical,
        Symmetry::Transpose,
        Symmetry::AntiTranspose,
    ];

    /// Quarter turns and transposes only map a box onto itself when it is square.
    pub fn needs_square_box(self) -> bool {
        matches!(
            self,
            Symmetry::Rot90 | Symmetry::Rot270 | Symmetry::Transpose | Symmetry::AntiTranspose
        )
    }

    pub fn apply(self, r: usize, c: usize, w: usize, h: usize) -> (usize, usize) {
        match self {
            Symmetry::Identity => (r, c),
            Symmetry::Rot90 => (c, h - 1 - r),
            Symmetry::Rot180 => (h - 1 - r, w - 1 - c),
            Symmetry::Rot270 => (w - 1 - c, r),
            Symmetry::MirrorHorizontal => (r, w - 1 - c),
            Symmetry::MirrorVertical => (h - 1 - r, c),
            Symmetry::Transpose => (c, r),
            Symmetry::AntiTranspose => (h - 1 - c, w - 1 - r),
        }
    }
}

/// All symmetries that map the board onto itself, expressed as cell
/// permutations (`perm[i]` is where cell `i` moves). The identity comes first
/// and duplicate permutations are collapsed.
pub fn board_symmetries(board: &Board) -> Vec<Vec<u32>> {
    let (w, h) = (board.width, board.height);
    let mut perms: Vec<Vec<u32>> = Vec::new();
    let mut seen: HashSet<Vec<u32>> = HashSet::new();

    for sym in Symmetry::ALL {
        if sym.needs_square_box() && w != h {
            continue;
        }
        let perm: Vec<u32> = (0..w * h)
            .map(|i| {
                let (r, c) = (i / w, i % w);
                let (nr, nc) = sym.apply(r, c, w, h);
                (nr * w + nc) as u32
            })
            .collect();
        let maps_onto_itself = (0..w * h).all(|i| !board.cells[i] || board.cells[perm[i] as usize]);
        if maps_onto_itself && seen.insert(perm.clone()) {
            perms.push(perm);
        }
    }
    perms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(text: &str) -> Shape {
        parse_shape_text(text, "shape").unwrap()
    }

    #[test]
    fn parses_block_scalar_indentation() {
        let s = shape("    ###\n    .#.");
        assert_eq!(s.cells, vec![(0, 0), (0, 1), (0, 2), (1, 1)]);
        assert_eq!((s.width, s.height), (3, 2));
    }

    #[test]
    fn trims_leading_empty_columns() {
        let s = shape(".##\n.##");
        assert_eq!((s.width, s.height), (2, 2));
    }

    #[test]
    fn dedupes_symmetric_orientations() {
        assert_eq!(orientations(&shape("##\n##"), true, true).len(), 1);
        assert_eq!(orientations(&shape("##"), true, false).len(), 2);
        assert_eq!(orientations(&shape("##"), true, true).len(), 2);
        // The L-tromino is achiral, so reflections are rotations of it.
        let tri = shape("###\n.#.");
        assert_eq!(orientations(&tri, false, false).len(), 1);
        assert_eq!(orientations(&tri, true, false).len(), 4);
        assert_eq!(orientations(&tri, true, true).len(), 4);
        // The L-tetromino is chiral and has eight distinct orientations.
        let tetra = shape("###\n#..");
        assert_eq!(orientations(&tetra, false, false).len(), 1);
        assert_eq!(orientations(&tetra, true, false).len(), 4);
        assert_eq!(orientations(&tetra, true, true).len(), 8);
    }

    #[test]
    fn connectivity() {
        assert!(is_connected(&[(0, 0), (0, 1), (1, 1)]));
        assert!(!is_connected(&[(0, 0), (0, 2)]));
    }
}
