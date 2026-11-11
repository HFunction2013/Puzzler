//! Command-line interface.

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::json;
use crate::model::Puzzle;
use crate::parse;
use crate::render::{self, Background, Theme};
use crate::solve::{self, SolveOptions, SolveResult};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const SHORT_HELP: &str = "usage: puzzler <PUZZLE.pzle> [options]   (--help for details)";

const HELP: &str = "\
puzzler - a fast exact-cover solver for polyomino and jigsaw puzzles

USAGE:
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

EXIT STATUS:
    0   at least one solution was found
    1   invalid arguments, unreadable file or invalid puzzle
    2   the puzzle has no solution

EXAMPLES:
    puzzler examples/two_dominoes.pzle
    puzzler examples/pentomino_3x20.pzle --limit 3
    puzzler puzzle.pzle --out solutions.json
";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

struct Args {
    input: PathBuf,
    out: Option<PathBuf>,
    json: bool,
    compress: bool,
    limit: Option<usize>,
    nodes: Option<u64>,
    time: Option<Duration>,
    color: ColorMode,
    background: Option<Background>,
    dedupe: bool,
    validate: bool,
    quiet: bool,
    seed: Option<u64>,
    help: bool,
    version: bool,
}

impl Args {
    fn parse(argv: Vec<String>) -> Result<Self, String> {
        let mut args = Args {
            input: PathBuf::new(),
            out: None,
            json: false,
            compress: false,
            limit: None,
            nodes: None,
            time: None,
            color: ColorMode::Auto,
            background: None,
            dedupe: true,
            validate: false,
            quiet: false,
            seed: None,
            help: false,
            version: false,
        };
        let mut input: Option<PathBuf> = None;
        let mut iter = argv.into_iter();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-h" | "--help" => args.help = true,
                "-V" | "--version" => args.version = true,
                "-o" | "--out" => {
                    let value = iter.next().ok_or("`--out` needs a file path")?;
                    args.out = Some(PathBuf::from(value));
                }
                "--json" => args.json = true,
                "--compress" => args.compress = true,
                "--limit" => {
                    let value = iter.next().ok_or("`--limit` needs a number")?;
                    let parsed = value
                        .parse::<usize>()
                        .map_err(|_| format!("`--limit` expects a positive integer, got {value:?}"))?;
                    if parsed == 0 {
                        return Err("`--limit` must be at least 1".to_string());
                    }
                    args.limit = Some(parsed);
                }
                "--nodes" => {
                    let value = iter.next().ok_or("`--nodes` needs a number")?;
                    let parsed = value
                        .parse::<u64>()
                        .map_err(|_| format!("`--nodes` expects a positive integer, got {value:?}"))?;
                    if parsed == 0 {
                        return Err("`--nodes` must be at least 1".to_string());
                    }
                    args.nodes = Some(parsed);
                }
                "--time" => {
                    let value = iter.next().ok_or("`--time` needs a number of seconds")?;
                    let parsed: f64 = value
                        .parse()
                        .map_err(|_| format!("`--time` expects a number of seconds, got {value:?}"))?;
                    if !parsed.is_finite() || parsed <= 0.0 {
                        return Err("`--time` must be greater than zero".to_string());
                    }
                    args.time = Some(Duration::from_secs_f64(parsed));
                }
                "--color" => {
                    let value = iter.next().ok_or("`--color` needs a value")?;
                    args.color = match value.as_str() {
                        "auto" => ColorMode::Auto,
                        "always" => ColorMode::Always,
                        "never" => ColorMode::Never,
                        other => {
                            return Err(format!(
                                "`--color` expects auto, always or never, got {other:?}"
                            ));
                        }
                    };
                }
                "--no-color" => args.color = ColorMode::Never,
                "--background" => {
                    let value = iter.next().ok_or("`--background` needs a value")?;
                    args.background = Some(match value.as_str() {
                        "auto" => render::detect_background(),
                        "dark" => Background::Dark,
                        "light" => Background::Light,
                        other => {
                            return Err(format!(
                                "`--background` expects auto, dark or light, got {other:?}"
                            ));
                        }
                    });
                }
                "--no-dedupe" => args.dedupe = false,
                "--validate" => args.validate = true,
                "-q" | "--quiet" => args.quiet = true,
                "--seed" => {
                    let value = iter.next().ok_or("`--seed` needs a number")?;
                    args.seed = Some(
                        value
                            .parse::<u64>()
                            .map_err(|_| format!("`--seed` expects an integer, got {value:?}"))?,
                    );
                }
                other if other.starts_with('-') && other != "-" => {
                    return Err(format!("unknown option `{other}`"));
                }
                other => {
                    if input.is_some() {
                        return Err(format!("unexpected extra argument `{other}`"));
                    }
                    input = Some(PathBuf::from(other));
                }
            }
        }

        match input {
            Some(path) => args.input = path,
            None if args.help || args.version => args.input = PathBuf::from("-"),
            None => return Err("missing the puzzle file".to_string()),
        };
        Ok(args)
    }
}

pub fn run() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match Args::parse(argv) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{SHORT_HELP}");
            return 1;
        }
    };

    if args.help {
        print!("{HELP}");
        return 0;
    }
    if args.version {
        println!("puzzler {VERSION}");
        return 0;
    }
    execute(args)
}

fn execute(args: Args) -> i32 {
    let text = match read_input(&args.input) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("error: {message}");
            return 1;
        }
    };

    let parsed = match parse::parse_str(&text) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {}: {message}", display_path(&args.input));
            return 1;
        }
    };
    for warning in &parsed.warnings {
        eprintln!("warning: {warning}");
    }
    let puzzle = parsed.puzzle;

    if args.validate {
        print_validation(&puzzle);
        return 0;
    }

    let opts = SolveOptions {
        max_solutions: args.limit,
        node_limit: args.nodes,
        time_limit: args.time,
        dedupe: args.dedupe,
    };
    let result = solve::solve(&puzzle, &opts);

    if let Some(path) = &args.out {
        if let Err(error) = json::write_to_file(path, &puzzle, &result, args.compress) {
            eprintln!("error: cannot write {}: {error}", path.display());
            return 1;
        }
        eprintln!("{}", summary_line(&result, &puzzle));
        eprintln!(
            "wrote {} solution(s) to {}",
            result.solutions.len(),
            path.display()
        );
    } else if args.json {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let report = json::build_report(&puzzle, &result);
        let write_result = if args.compress {
            serde_json::to_writer(&mut out, &report)
        } else {
            serde_json::to_writer_pretty(&mut out, &report)
        };
        if let Err(error) = write_result {
            eprintln!("error: cannot write JSON: {error}");
            return 1;
        }
        let _ = writeln!(out);
        let _ = out.flush();
    } else {
        let color = match args.color {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => io::stdout().is_terminal(),
        };
        let background = args.background.unwrap_or_else(render::detect_background);
        let seed = args.seed.unwrap_or_else(random_seed);
        let theme = Theme::new(color, background, seed, puzzle.pieces.len());

        let stdout = io::stdout();
        let mut out = stdout.lock();
        match print_report(&mut out, &puzzle, &result, &theme, args.quiet) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return 0,
            Err(error) => {
                eprintln!("error: cannot write output: {error}");
                return 1;
            }
        }
    }

    if result.solutions.is_empty() {
        eprintln!("no solution: the pieces cannot cover this board");
        2
    } else {
        0
    }
}

fn print_report<W: Write>(
    out: &mut W,
    puzzle: &Puzzle,
    result: &SolveResult,
    theme: &Theme,
    quiet: bool,
) -> io::Result<()> {
    if !quiet {
        writeln!(out, "puzzler {VERSION} - {}", puzzle.name)?;
        writeln!(
            out,
            "board {}x{} ({} cells) | {} piece types ({} cells) | all_use: {} | rotate: {} | flip: {}",
            puzzle.board.width,
            puzzle.board.height,
            puzzle.board.cell_count(),
            puzzle.pieces.len(),
            puzzle.total_piece_cells(),
            yes_no(puzzle.all_use),
            yes_no(puzzle.can_rotate),
            yes_no(puzzle.can_flip)
        )?;
        writeln!(out)?;
        writeln!(out, "board:")?;
        render::render_board_map(out, puzzle)?;
        writeln!(out)?;
        render::render_legend(out, puzzle, theme)?;
        writeln!(out)?;
    }

    for (index, solution) in result.solutions.iter().enumerate() {
        let title = format!(
            "solution {}/{}  ({} layout{} under board symmetry)",
            index + 1,
            result.solutions.len(),
            solution.multiplicity,
            if solution.multiplicity == 1 { "" } else { "s" }
        );
        render::render_solution(out, &title, puzzle, &result.placements, solution, theme)?;
        if index + 1 < result.solutions.len() {
            writeln!(out)?;
        }
    }

    if !quiet {
        writeln!(out)?;
        writeln!(out, "{}", summary_line(result, puzzle))?;
    }
    Ok(())
}

fn print_validation(puzzle: &Puzzle) {
    println!("puzzle:  {}", puzzle.name);
    println!(
        "board:   {}x{} ({} cells), origin ({}, {})",
        puzzle.board.width,
        puzzle.board.height,
        puzzle.board.cell_count(),
        puzzle.board.origin_row,
        puzzle.board.origin_col
    );
    println!(
        "pieces:  {} types, {} cells in total",
        puzzle.pieces.len(),
        puzzle.total_piece_cells()
    );
    println!(
        "options: all_use={} can_rotate={} can_flip={}",
        puzzle.all_use, puzzle.can_rotate, puzzle.can_flip
    );
    println!("board shape:");
    for row in &puzzle.board.rows {
        println!("    {row}");
    }
    println!("pieces:");
    for piece in &puzzle.pieces {
        println!(
            "    #{:<8} {:>2} cells x{:<3} rotate={} flip={} orientations={}",
            piece.id,
            piece.area(),
            piece.count,
            yes_no(piece.can_rotate),
            yes_no(piece.can_flip),
            piece.orientations.len()
        );
    }
    println!("OK: the puzzle is valid");
}

fn summary_line(result: &SolveResult, puzzle: &Puzzle) -> String {
    let mut text = format!(
        "{} solution{} | {} raw layouts | {} nodes | {:.2} ms",
        result.solutions.len(),
        if result.solutions.len() == 1 { "" } else { "s" },
        group_digits(result.stats.raw_solutions),
        group_digits(result.stats.nodes),
        result.stats.elapsed.as_secs_f64() * 1000.0
    );
    if result.symmetry_group > 1 {
        text.push_str(&format!(
            " | {} board symmetries merged",
            result.symmetry_group
        ));
    }
    if result.stats.truncated {
        text.push_str(" | search truncated");
    }
    let _ = puzzle;
    text
}

fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn read_input(path: &Path) -> Result<String, String> {
    if path == Path::new("-") {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        return Ok(buffer);
    }
    fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn display_path(path: &Path) -> String {
    if path == Path::new("-") {
        "<stdin>".to_string()
    } else {
        path.display().to_string()
    }
}

fn random_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0x5EED)
}
