mod bitset;
mod cli;
mod geom;
mod json;
mod model;
mod parse;
mod render;
mod solve;

fn main() {
    std::process::exit(cli::run());
}
