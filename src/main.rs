use clap::Parser;

use rscm::*;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(e) = cli::run(cli) {
        eprintln!("Error: {}", e);

        let msg = e.to_string();
        if msg.contains("does not exist") {
            eprintln!("\nHint: Run 'sudo rscm init' to create the store directory.");
        } else if msg.contains("Permission denied") {
            eprintln!("\nHint: This operation requires root privileges.");
            eprintln!("Run with: sudo rscm <command>");
        }

        std::process::exit(1);
    }
}
