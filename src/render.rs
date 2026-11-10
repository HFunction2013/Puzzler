//! ANSI terminal rendering: one coloured block per cell, one colour per piece.

use std::io::{self, Write};

use crate::model::Puzzle;
use crate::solve::{PlacementInfo, Solution};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

/// Detects the terminal background from `$COLORFGBG` (e.g. `15;0`).
pub fn detect_background() -> Background {
    match std::env::var("COLORFGBG") {
        Ok(value) => {
            let last = value
                .split(';')
                .next_back()
                .and_then(|v| v.trim().parse::<u8>().ok())
                .unwrap_or(0);
            if last >= 7 {
                Background::Light
            } else {
                Background::Dark
            }
        }
        Err(_) => Background::Dark,
    }
}

pub struct Theme {
    pub color: bool,
    /// xterm-256 background index per piece.
    pub piece_colors: Vec<u8>,
    /// xterm-256 foreground index per piece, chosen for contrast.
    pub piece_foregrounds: Vec<u8>,
    /// xterm-256 colour for board cells that no piece covers.
    pub board_color: u8,
}

impl Theme {
    pub fn new(color: bool, background: Background, seed: u64, piece_types: usize) -> Self {
        let mut piece_colors = Vec::with_capacity(piece_types);
        let mut piece_foregrounds = Vec::with_capacity(piece_types);
        // Golden-angle hue stepping keeps neighbouring pieces far apart even
        // when the piece count is large.
        let offset = (splitmix64(seed) % 10_000) as f32 / 10_000.0;
        for index in 0..piece_types {
            let hue = (offset + index as f32 * 0.618_034) % 1.0;
            let value = match background {
                Background::Dark => 0.95,
                Background::Light => 0.72,
            };
            let rgb = hsv_to_rgb(hue, 0.82, value);
            let color = rgb_to_xterm(rgb);
            let foreground = if luminance(rgb) > 140.0 { 0 } else { 15 };
            piece_colors.push(color);
            piece_foregrounds.push(foreground);
        }
        let board_color = match background {
            Background::Dark => 244,
            Background::Light => 240,
        };
        Self {
            color,
            piece_colors,
            piece_foregrounds,
            board_color,
        }
    }

    /// A printable symbol for a piece when colours are unavailable.
    ///
    /// With fewer than ten piece types the pieces are numbered 1..9; once
    /// there are ten or more the pieces are drawn as `#`, matching the shape
    /// syntax used in `.pzle` files.
    fn symbol(&self, piece: usize) -> char {
        if self.piece_colors.len() < 10 {
            char::from_digit(piece as u32 + 1, 10).unwrap_or('#')
        } else {
            '#'
        }
    }
}

pub fn render_solution<W: Write>(
    out: &mut W,
    title: &str,
    puzzle: &Puzzle,
    placements: &[PlacementInfo],
    solution: &Solution,
    theme: &Theme,
) -> io::Result<()> {
    let width = puzzle.board.width;
    let height = puzzle.board.height;
    let mut labels = vec![0usize; width * height];
    for &pid in &solution.placements {
        let placement = &placements[pid];
        for &cell in &placement.cells {
            labels[cell as usize] = placement.piece + 1;
        }
    }

    let labels_mode = puzzle.pieces.len() < 10;

    writeln!(out, "{title}")?;
    for r in 0..height {
        write!(out, "  ")?;
        for c in 0..width {
            let idx = r * width + c;
            if !puzzle.board.cells[idx] {
                write!(out, "  ")?;
                continue;
            }
            let label = labels[idx];
            if label == 0 {
                // A board cell no piece covers (only possible in reports).
                if theme.color {
                    write!(out, "\x1b[38;5;{}m##\x1b[0m", theme.board_color)?;
                } else {
                    write!(out, "##")?;
                }
                continue;
            }
            let piece = label - 1;
            if theme.color {
                let bg = theme.piece_colors[piece];
                let fg = theme.piece_foregrounds[piece];
                if labels_mode {
                    write!(
                        out,
                        "\x1b[48;5;{bg}m\x1b[38;5;{fg}m{} \x1b[0m",
                        (piece + 1) % 10
                    )?;
                } else {
                    write!(out, "\x1b[48;5;{bg}m\x1b[38;5;{fg}m##\x1b[0m")?;
                }
            } else {
                let symbol = theme.symbol(piece);
                write!(out, "{symbol}{symbol}")?;
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

pub fn render_legend<W: Write>(out: &mut W, puzzle: &Puzzle, theme: &Theme) -> io::Result<()> {
    writeln!(out, "  pieces:")?;
    for (index, piece) in puzzle.pieces.iter().enumerate() {
        write!(out, "    ")?;
        if theme.color {
            let bg = theme.piece_colors[index];
            write!(out, "\x1b[48;5;{bg}m  \x1b[0m ")?;
        } else {
            let symbol = theme.symbol(index);
            write!(out, "{symbol}{symbol} ")?;
        }
        writeln!(
            out,
            "{:<8} {:>2} cells{:<4} rotate: {:<5} flip: {:<5} orientations: {}",
            format!("#{}", piece.id),
            piece.area(),
            if piece.count > 1 {
                format!(" x{}", piece.count)
            } else {
                String::new()
            },
            if piece.can_rotate { "yes" } else { "no" },
            if piece.can_flip { "yes" } else { "no" },
            piece.orientations.len()
        )?;
    }
    Ok(())
}

/// ASCII preview of the board region.
pub fn render_board_map<W: Write>(out: &mut W, puzzle: &Puzzle) -> io::Result<()> {
    for row in &puzzle.board.rows {
        writeln!(out, "    {row}")?;
    }
    Ok(())
}

fn splitmix64(mut state: u64) -> u64 {
    state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = (h.fract() + 1.0).fract() * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let clamp = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
    (clamp(r), clamp(g), clamp(b))
}

fn luminance(rgb: (u8, u8, u8)) -> f32 {
    0.2126 * rgb.0 as f32 + 0.7152 * rgb.1 as f32 + 0.0722 * rgb.2 as f32
}

/// Nearest xterm-256 colour (6x6x6 cube plus the grey ramp).
fn rgb_to_xterm(rgb: (u8, u8, u8)) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut best = (u32::MAX, 16u8);
    for (ri, &r) in LEVELS.iter().enumerate() {
        for (gi, &g) in LEVELS.iter().enumerate() {
            for (bi, &b) in LEVELS.iter().enumerate() {
                let index = (16 + 36 * ri + 6 * gi + bi) as u8;
                let distance = color_distance(rgb, (r, g, b));
                if distance < best.0 {
                    best = (distance, index);
                }
            }
        }
    }
    for step in 0..24u8 {
        let value = 8 + 10 * step;
        let index = 232 + step;
        let distance = color_distance(rgb, (value, value, value));
        if distance < best.0 {
            best = (distance, index);
        }
    }
    best.1
}

fn color_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_is_stable_and_distinct() {
        let theme = Theme::new(true, Background::Dark, 42, 12);
        assert_eq!(theme.piece_colors.len(), 12);
        let unique: std::collections::HashSet<_> = theme.piece_colors.iter().collect();
        assert!(unique.len() >= 10, "pieces should get distinct colours");
        let same = Theme::new(true, Background::Dark, 42, 12);
        assert_eq!(theme.piece_colors, same.piece_colors);
    }
}
