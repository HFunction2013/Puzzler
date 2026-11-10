//! Reading `.pzle` files.
//!
//! A `.pzle` file is ordinary YAML. The `puzzle:` section may be written either
//! as a plain mapping or as the list-of-single-key-mappings style that the
//! original specification uses:
//!
//! ```yaml
//! puzzle:
//!   - name: 12piece
//!   - piece_count: 12
//!   - board_shape: "rectangle{20,3}"
//! ```
//!
//! Both forms are normalized into one mapping before being interpreted, so the
//! two styles can be mixed freely.

use serde_yaml::{Mapping, Value};

use crate::geom;
use crate::model::{Board, Piece, Puzzle};

pub struct Parsed {
    pub puzzle: Puzzle,
    pub warnings: Vec<String>,
}

pub fn parse_str(text: &str) -> Result<Parsed, String> {
    let root: Value =
        serde_yaml::from_str(text).map_err(|e| format!("invalid YAML: {e}"))?;
    let root_map = root
        .as_mapping()
        .ok_or_else(|| "the document must be a YAML mapping".to_string())?;

    let mut warnings = Vec::new();

    let puzzle_value = if let Some(v) = find(root_map, "puzzle") {
        v
    } else if find(root_map, "pieces").is_some() {
        // Be forgiving: a bare puzzle mapping without the `puzzle:` wrapper.
        &root
    } else {
        return Err("missing the top-level `puzzle:` section".to_string());
    };

    let map = normalize_map(puzzle_value, "puzzle")?;
    warn_unknown_keys(
        &map,
        &[
            "name",
            "piece_count",
            "all_use",
            "board_shape",
            "board",
            "can_flip",
            "can_rotate",
            "flip",
            "rotate",
            "pieces",
        ],
        "puzzle",
        &mut warnings,
    );

    let name = match find(&map, "name") {
        Some(v) => as_string(v, "puzzle.name")?,
        None => "puzzle".to_string(),
    };

    let board_shape = get_any(&map, &["board_shape", "board"])
        .ok_or_else(|| "puzzle: missing `board_shape:`".to_string())?;
    let board_text = shape_text(board_shape, "puzzle.board_shape")?;
    let board = parse_board_shape(&board_text)?;

    let all_use = match find(&map, "all_use") {
        Some(v) => as_bool(v, "puzzle.all_use")?,
        None => true,
    };
    let can_flip = match get_any(&map, &["can_flip", "flip"]) {
        Some(v) => as_bool(v, "puzzle.can_flip")?,
        None => false,
    };
    let can_rotate = match get_any(&map, &["can_rotate", "rotate"]) {
        Some(v) => as_bool(v, "puzzle.can_rotate")?,
        None => false,
    };
    let piece_count_declared = match find(&map, "piece_count") {
        Some(v) => Some(as_usize(v, "puzzle.piece_count")?),
        None => None,
    };

    let pieces_value = find(&map, "pieces")
        .ok_or_else(|| "puzzle: missing the `pieces:` list".to_string())?;
    let piece_entries = pieces_value
        .as_sequence()
        .ok_or_else(|| "puzzle.pieces: a list of pieces is required".to_string())?;

    let mut pieces = Vec::with_capacity(piece_entries.len());
    for (index, entry) in piece_entries.iter().enumerate() {
        let ctx = format!("piece #{}", index + 1);
        let entry_map = normalize_map(entry, &ctx)?;
        warn_unknown_keys(
            &entry_map,
            &["id", "shape", "count", "can_flip", "can_rotate", "flip", "rotate"],
            &ctx,
            &mut warnings,
        );

        let id = match find(&entry_map, "id") {
            Some(v) => as_string(v, &format!("{ctx}.id"))?,
            None => (index + 1).to_string(),
        };

        let shape_value = find(&entry_map, "shape")
            .ok_or_else(|| format!("{ctx}: missing `shape:`"))?;
        let shape_text_value = shape_text(shape_value, &format!("{ctx}.shape"))?;
        let shape = geom::parse_shape_text(&shape_text_value, &format!("piece {id}"))?;

        let count = match find(&entry_map, "count") {
            Some(v) => {
                let c = as_usize(v, &format!("{ctx}.count"))?;
                if c == 0 || c > u16::MAX as usize {
                    return Err(format!(
                        "{ctx}: count must be between 1 and {}",
                        u16::MAX
                    ));
                }
                c as u16
            }
            None => 1,
        };

        let piece_can_flip = match get_any(&entry_map, &["can_flip", "flip"]) {
            Some(v) => Some(as_bool(v, &format!("{ctx}.can_flip"))?),
            None => None,
        };
        let piece_can_rotate = match get_any(&entry_map, &["can_rotate", "rotate"]) {
            Some(v) => Some(as_bool(v, &format!("{ctx}.can_rotate"))?),
            None => None,
        };

        // Per-piece options take precedence over the puzzle-wide defaults.
        let effective_flip = piece_can_flip.unwrap_or(can_flip);
        let effective_rotate = piece_can_rotate.unwrap_or(can_rotate);
        let orientations = geom::orientations(&shape, effective_rotate, effective_flip);

        pieces.push(Piece {
            id,
            shape,
            count,
            can_rotate: effective_rotate,
            can_flip: effective_flip,
            orientations,
        });
    }

    let puzzle = Puzzle {
        name,
        board,
        pieces,
        all_use,
        can_rotate,
        can_flip,
        piece_count_declared,
    };
    let warnings_from_validation = puzzle.validate()?;
    warnings.extend(warnings_from_validation);

    Ok(Parsed { puzzle, warnings })
}

/// Accepts both `key: value` mappings and `- key: value` lists.
fn normalize_map(value: &Value, ctx: &str) -> Result<Mapping, String> {
    match value {
        Value::Mapping(map) => Ok(map.clone()),
        Value::Sequence(seq) => {
            let mut merged = Mapping::new();
            for (i, item) in seq.iter().enumerate() {
                match item {
                    Value::Mapping(map) if !map.is_empty() => {
                        for (k, v) in map {
                            merged.insert(k.clone(), v.clone());
                        }
                    }
                    other => {
                        return Err(format!(
                            "{ctx}: entry {} must be a mapping (`- key: value`), found {}",
                            i + 1,
                            type_name(other)
                        ));
                    }
                }
            }
            Ok(merged)
        }
        other => Err(format!(
            "{ctx}: expected a mapping or a list of `- key: value` entries, found {}",
            type_name(other)
        )),
    }
}

fn find<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k.as_str().map(|s| s == key).unwrap_or(false))
        .map(|(_, v)| v)
}

fn get_any<'a>(map: &'a Mapping, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| find(map, key))
}

fn warn_unknown_keys(map: &Mapping, known: &[&str], ctx: &str, warnings: &mut Vec<String>) {
    for key in map.keys() {
        if let Some(name) = key.as_str() {
            if !known.contains(&name) {
                warnings.push(format!("{ctx}: ignoring unknown key `{name}`"));
            }
        }
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a list",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}

fn as_string(value: &Value, ctx: &str) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(format!("{ctx}: expected a string, found {}", type_name(other))),
    }
}

fn as_usize(value: &Value, ctx: &str) -> Result<usize, String> {
    match value {
        Value::Number(n) => n
            .as_u64()
            .map(|v| v as usize)
            .ok_or_else(|| format!("{ctx}: expected a non-negative integer")),
        Value::String(s) => s
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("{ctx}: expected a non-negative integer, found {s:?}")),
        other => Err(format!(
            "{ctx}: expected a non-negative integer, found {}",
            type_name(other)
        )),
    }
}

fn as_bool(value: &Value, ctx: &str) -> Result<bool, String> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match n.as_u64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(format!("{ctx}: expected a boolean, found {n}")),
        },
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(true),
            "false" | "no" | "off" | "0" => Ok(false),
            _ => Err(format!("{ctx}: expected a boolean, found {s:?}")),
        },
        other => Err(format!("{ctx}: expected a boolean, found {}", type_name(other))),
    }
}

/// Shape-ish values may be a string or a list of row strings.
fn shape_text(value: &Value, ctx: &str) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Sequence(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            for (i, row) in rows.iter().enumerate() {
                out.push(as_string(row, &format!("{ctx}[{i}]"))?);
            }
            Ok(out.join("\n"))
        }
        other => Err(format!(
            "{ctx}: expected a string or a list of row strings, found {}",
            type_name(other)
        )),
    }
}

/// `rectangle{W,H}` (also `rect{W,H}` / `{WxH}`) or a `#`/`.` grid.
fn parse_board_shape(text: &str) -> Result<Board, String> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("rectangle") || lower.starts_with("rect") {
        // Recover the text inside the braces whatever prefix was used.
        let inner = match (trimmed.find('{'), trimmed.rfind('}')) {
            (Some(start), Some(end)) if end > start => trimmed[start + 1..end].trim(),
            _ => "",
        };
        if inner.is_empty() {
            return Err(
                "board_shape: expected `rectangle{width,height}`, for example `rectangle{20,3}`"
                    .to_string(),
            );
        }
        let separator = if inner.contains(',') {
            ','
        } else if inner.contains('x') || inner.contains('X') {
            'x'
        } else if inner.contains('*') {
            '*'
        } else {
            return Err(format!(
                "board_shape: `{trimmed}` is not a valid rectangle; use `rectangle{{width,height}}`"
            ));
        };
        let mut parts = inner.split(separator);
        let width = parts
            .next()
            .and_then(|p| p.trim().parse::<usize>().ok())
            .ok_or_else(|| format!("board_shape: invalid width in `{trimmed}`"))?;
        let height = parts
            .next()
            .and_then(|p| p.trim().parse::<usize>().ok())
            .ok_or_else(|| format!("board_shape: invalid height in `{trimmed}`"))?;
        if parts.next().is_some() {
            return Err(format!(
                "board_shape: `{trimmed}` has too many dimensions; expected `rectangle{{width,height}}`"
            ));
        }
        return Board::rectangle(width, height);
    }

    let rows: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|s| s.to_string())
        .collect();
    Board::from_rows(&rows)
}
