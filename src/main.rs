use clap::Parser;

use rscm::*;

fn main() {
    let cli = cli::Cli::parse();
    cli::run(cli).unwrap();
}
