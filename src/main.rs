use clap::Parser;
use personal_transcriber::cli::{self, Cli};

fn main() {
    let cli = Cli::parse();
    personal_transcriber::logging::init(cli.verbose);

    if let Err(error) = cli::execute(cli.command, cli.sessions_dir) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
