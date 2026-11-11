# Puzzler

A fast exact-cover solver for polyomino / jigsaw puzzles defined in a tiny
YAML dialect (`.pzle`). It enumerates every distinct tiling of a board,
merges solutions that differ only by a symmetry of the board, and prints
them as colour-coded blocks or as a JSON report.

```
puzzler puzzle.pzle
```

## Highlights

- **Exact-cover search with MRV (minimum-remaining-values) heuristic.** At
  every node it picks the empty cell with the fewest feasible placements, which
  prunes the search tree by orders of magnitude compared with the naive
  "always fill the first empty cell" order.
- **Symmetry-aware.** Rotations and reflections of pieces are generated once
  and deduplicated; duplicate board symmetries (rotations / mirrorings of the
  whole rectangle) are folded onto a canonical representative so that
  physically identical tilings are reported once.
- **Colour output.** Each piece gets a random golden-angle colour that
  contrasts with the terminal background (detected from `$COLORFGBG`). With
  fewer than ten piece types the cells are numbered `1..9`; otherwise they
  are drawn as `#`, matching the shape syntax.
- **JSON output** (`--out file.json` / `--json`) for scripting.
- **Polyomino-grade speed.** The classic 12-pentomino `6×10` rectangle —
  which has 2 339 distinct tilings — is solved in roughly 20 seconds on a
  single core.

## Install

```sh
cargo install --path .
# or
cargo build --release
./target/release/puzzler puzzle.pzle
```

## Usage

```
puzzler <PUZZLE.pzle> [OPTIONS]

ARGS:
    <PUZZLE.pzle>    Puzzle definition (YAML). Use `-` to read from stdin.

OPTIONS:
    -o, --out <FILE>        Write the solutions as JSON to FILE
        --json              Print JSON to stdout instead of coloured boards
        --limit <N>         Stop after N distinct solutions
        --nodes <N>         Stop after N search nodes
        --time <SECONDS>    Stop after SECONDS of search time
        --no-dedupe         Keep solutions that only differ by rotation/mirror
        --color <WHEN>      auto | always | never        [default: auto]
        --no-color          Same as --color never
        --background <B>    auto | dark | light          [default: auto]
        --validate          Parse and validate the puzzle, do not solve it
    -q, --quiet             Print only the boards, without header or statistics
        --seed <N>          Colour seed (default: derived from the clock)
    -h, --help              Print this help
    -V, --version           Print the version
```

Exit status: `0` at least one solution, `1` bad input / invalid puzzle,
`2` no solution.

## The `.pzle` format

A `.pzle` file is ordinary YAML. The `puzzle:` section may be written either
as a mapping or as the list-of-single-key-mappings style from the original
sketch; both are normalised.

```yaml
puzzle:
  name: my_puzzle
  piece_count: 12          # informational: number of *types* of piece
  all_use: true            # false = allow unused pieces (subset tiling)
  board_shape: "rectangle{20,3}"
  # or an ASCII grid:
  # board_shape: "###\n#.#\n###"
  can_rotate: true         # global defaults
  can_flip: false
  pieces:
    - id: a
      shape: "###\n.#."
      count: 1             # how many copies of this type (default 1)
      can_rotate: true     # per-piece override (highest priority)
      can_flip: false
```

### `board_shape`

Two forms are accepted:

- `rectangle{W,H}` — a fully playable `W × H` rectangle (also `rect{W,H}`,
  `WxH`, `W*H`).
- An ASCII grid where `#` is a playable cell and `.` (or space) is blocked.
  Surrounding empty rows/columns are trimmed.

### `pieces`

Each piece has:

| key           | meaning                                                                 |
|---------------|--------------------------------------------------------------------------|
| `id`          | optional label (defaults to the 1-based index)                            |
| `shape`       | a `#`/`.` string, or a list of row strings; `#` = filled, `.` = empty  |
| `count`       | number of copies of this type (default 1)                                |
| `can_rotate`  | per-piece override of the global default                                 |
| `can_flip`    | per-piece override                                                       |

### Options precedence

Per-piece `can_rotate` / `can_flip` win over the puzzle-wide defaults, which
themselves default to `false`. `count` is the number of physical copies;
`piece_count` (when present) is checked against the number of piece *types*
and only used as a sanity warning.

### Symmetry handling

- Pieces are de-duplicated under their allowed rotations/reflections before
  placement enumeration.
- Two raw layouts that are related by a symmetry of the *board* (identity,
  rotation by 180°, mirror across the horizontal/vertical axis when the box
  is not square, etc.) are merged into a single canonical solution. Use
  `--no-dedupe` to keep every raw layout.

## Examples

```sh
# The canonical 12-pentomino tiling problems (2 and 2339 distinct solutions).
puzzler examples/pentomino_3x20.pzle
puzzler examples/pentomino_6x10.pzle

# Stop after the first solution, print as JSON.
puzzler examples/pentomino_6x10.pzle --limit 1 --json

# Write a full report to disk.
puzzler examples/pentomino_6x10.pzle --out solutions.json
```

| file | demonstrates |
|---|---|
| `pentomino_3x20.pzle` | full set of 12 pentominoes, `can_rotate` + `can_flip` |
| `pentomino_6x10.pzle` | same pieces on a denser rectangle (2,339 solutions) |
| `ring_l_tetrominoes.pzle` | non-rectangular board with a hole (5×5 ring), `count: 4` |
| `four_dominoes.pzle` | repeated piece type via `count: 4` |
| `chiral_l_pair.pzle` | chiral piece with `can_flip: true` (8 orientations) |
| `no_rotate_row.pzle` | `can_rotate: false`, pieces keep their written orientation |
| `per_piece_override.pzle` | per-piece `can_rotate` overriding the global default |
| `subset_tiling.pzle` | `all_use: false` (extra pieces, only a subset is used) |
| `t_board.pzle` | irregular ASCII board (T-shape), mixed piece sizes |

## How it works

1. **Parse & validate.** The YAML is normalised, the board is trimmed to
   its bounding box, every piece orientation is generated and de-duplicated,
   and the area / connectivity / `all_use` invariants are checked.
2. **Pre-compute placements.** For every `(piece, orientation, position)`
   that fits the board, a bitmask of covered cells is built. Each placement
   is attached to every cell it covers (the `covering` lists used by MRV).
3. **Backtracking search.** At each node:
   - prune on the remaining-cell count (and a subset-sum DP when
     `all_use: false`);
   - pick the empty playable cell with the fewest feasible placements
     (MRV); if any cell has zero candidates, the branch is dead;
   - try the feasible placements covering that cell, biggest pieces first;
   - on a complete board, compute the canonical label grid over all board
     symmetries and either insert it or bump the multiplicity of an existing
     equivalent solution.

Bitboards are `Vec<u64>`; occupancy is pushed/popped with XOR so a branch
costs only a few word operations.

## License

MIT — see [LICENSE](LICENSE).
