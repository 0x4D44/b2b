use crate::{cli::Cli, logging, sip::UaHandle, sip_shim};
use anyhow::{Context, Result};

pub fn run(args: &Cli) -> Result<()> {
    let tag = logging::role_tag("mixer");
    logging::println_tag(&tag, "starting (bridge MVP)");

    let sip = args.sip_bind.as_deref().unwrap_or("0.0.0.0:5063");
    let target = args
        .target
        .as_deref()
        .context("mixer requires --target (sink)")?;

    // Preload config before UA init: codec modules + sip_listen so inbound leg actually listens
    let preload = format!(
        "module\t\tg711\n\
         module\t\tl16\n\
         sip_listen\t{}\n\
         call_accept\tyes\n",
        sip
    );
    unsafe {
        std::env::set_var("BRS_CONF_BUF", preload);
    }
    // Initialize baresip core/UA first (shared reactor/event bus)
    let ua = UaHandle::init().context("init UA")?;
    let (_bridge_ev, rx_ev) = ua.reactor.register_events();
    // Background: log key mixer events for both legs
    {
        let tag = logging::role_tag("mixer");
        std::thread::spawn(move || {
            while let Ok(ev) = rx_ev.recv() {
                let kind = format!("{:?}", ev.kind());
                let text = ev.text.clone().unwrap_or_default();
                if kind.contains("CALL_REMOTE_SDP") {
                    logging::println_tag(&tag, "bevent: CALL_REMOTE_SDP (offer)");
                } else if kind.contains("CALL_LOCAL_SDP") {
                    logging::println_tag(&tag, "bevent: CALL_LOCAL_SDP (answer)");
                } else if kind.contains("CALL_INCOMING") {
                    logging::println_tag(&tag, &format!("bevent: CALL_INCOMING ({})", text));
                } else if kind.contains("CALL_PROGRESS") {
                    logging::println_tag(&tag, "bevent: CALL_PROGRESS");
                } else if kind.contains("CALL_RTPESTAB") {
                    logging::println_tag(&tag, "bevent: CALL_RTPESTAB");
                } else if kind.contains("CALL_ESTABLISHED") {
                    logging::println_tag(&tag, &format!("bevent: CALL_ESTABLISHED ({})", text));
                } else if kind.contains("CALL_CLOSED") {
                    logging::println_tag(&tag, &format!("bevent: CALL_CLOSED ({})", text));
                }
            }
        });
    }

    // Log compiled-in codecs
    {
        let codecs = sip_shim::codecs_csv();
        logging::println_tag(&tag, &format!("codecs: {}", codecs));
    }

    // Start mixer legs: inbound catch-all and outbound dial
    sip_shim::mixer_init(sip, target, 8000, 1, args.ptime_ms)?;
    // Configure mixing: dtmf sequence and gains
    sip_shim::mixer_config(
        &args.dtmf_seq,
        args.dtmf_period_ms,
        args.mix_gain_in,
        args.mix_gain_dtmf,
    )?;
    unsafe {
        std::env::remove_var("BRS_CONF_BUF");
    }

    // Event loop: wait for outbound (to Sink) established, then READY
    // Give outbound leg up to 5s to establish
    std::thread::sleep(std::time::Duration::from_millis(500));
    logging::ready_line("mixer", sip, &args.codec, args.ptime_ms);

    // Keep process alive until Ctrl+C
    wait_for_ctrl_c();
    logging::println_tag(&tag, "Ctrl+C received; shutting down");

    let _ = sip_shim::mixer_shutdown();
    let _ = ua.reactor.shutdown();
    logging::println_tag(&tag, "shutdown");
    Ok(())
}

fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || {
        let _ = tx.send(());
    });
    let _ = rx.recv();
}
