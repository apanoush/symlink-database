use clap::Parser;

use symlink_database::cli::Cli;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = cli.run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
