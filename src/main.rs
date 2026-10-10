use clap::Parser;

use symlink_database::cli::{Cli, Commands};

mod tui_run;

fn main() {
    let cli = Cli::parse();

    // `tui` needs terminal I/O, which stays in the binary; every other command
    // is orchestrated by the library.
    let result: Result<(), String> = match &cli.command {
        Commands::Tui => tui_run::run().map_err(|e| e.to_string()),
        _ => cli.run().map_err(|e| e.to_string()),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
