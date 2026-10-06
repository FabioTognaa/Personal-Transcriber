use clap::Parser;
use live_transcript::cli::{self, Cli};

fn main() {
    let cli = Cli::parse();
    live_transcript::logging::init(cli.verbose);

    if let Err(error) = cli::execute(cli.command, cli.sessions_dir) {
        tracing::error!(%error);
        std::process::exit(1);
    }
}
