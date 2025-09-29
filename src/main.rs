mod cli;
mod logging;
mod media;
mod orchestrator;
mod roles;
mod sip;
mod sip_shim;
mod util;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let args = cli::Cli::parse();
    logging::init(&args);

    match args.role {
        cli::RoleKind::Orchestrator => orchestrator::run(&args),
        cli::RoleKind::Source => roles::source::run(&args),
        cli::RoleKind::Sink => roles::sink::run(&args),
        cli::RoleKind::Mixer => roles::mixer::run(&args),
    }
}
