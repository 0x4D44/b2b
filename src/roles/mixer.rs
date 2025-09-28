use crate::{cli::Cli, logging};
use anyhow::Result;

pub fn run(args: &Cli) -> Result<()> {
    let tag = logging::role_tag("mixer");
    logging::println_tag(&tag, "starting (skeleton)");
    let sip = args.sip_bind.as_deref().unwrap_or("127.0.0.1:5063");
    logging::ready_line("mixer", sip, &args.codec, args.ptime_ms);
    wait_for_ctrl_c();
    logging::println_tag(&tag, "Ctrl+C received; shutting down");
    logging::println_tag(&tag, "shutdown");
    Ok(())
}

fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || { let _ = tx.send(()); });
    let _ = rx.recv();
}
