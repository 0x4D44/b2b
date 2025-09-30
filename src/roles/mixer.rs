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
    let sink_label = target.strip_prefix("sip:").unwrap_or(target).to_string();

    // Preload config before UA init: codec modules + sip_listen so inbound leg actually listens
    let preload = format!(
        "module\t\tg711\n\
         module\t\tl16\n\
         sip_listen\t{}\n\
         call_accept\tyes\n\
         audio_player\tb2b_mix,inbound\n",
        sip
    );
    unsafe {
        std::env::set_var("BRS_CONF_BUF", preload);
    }
    // Initialize baresip core/UA first (shared reactor/event bus)
    let ua = UaHandle::init().context("init UA")?;
    let (_bridge_ev, rx_ev) = ua.reactor.register_events();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    // Background: clear, leg-labelled logs for sink vs source
    {
        let tag = logging::role_tag("mixer");
        let sink_label = sink_label.clone();
        let ready_tx = ready_tx.clone();
        std::thread::spawn(move || {
            let mut sink_reported = false;
            while let Ok(ev) = rx_ev.recv() {
                let kind = format!("{:?}", ev.kind());
                let text = ev.text.clone().unwrap_or_default();
                let is_sink_leg = !sink_label.is_empty() && text.contains(&sink_label);
                if kind.contains("CALL_INCOMING") {
                    logging::println_tag(&tag, &format!("source: incoming {}", text));
                } else if kind.contains("CALL_LOCAL_SDP") {
                    let side = if is_sink_leg { "sink" } else { "source" };
                    logging::println_tag(&tag, &format!("{}: sdp answer", side));
                } else if kind.contains("CALL_REMOTE_SDP") {
                    let side = if is_sink_leg { "sink" } else { "source" };
                    logging::println_tag(&tag, &format!("{}: sdp offer", side));
                } else if kind.contains("CALL_ESTABLISHED") {
                    let side = if is_sink_leg { "sink" } else { "source" };
                    logging::println_tag(&tag, &format!("{}: established {}", side, text));
                    if is_sink_leg && !sink_reported {
                        sink_reported = true;
                        let _ = ready_tx.send(Ok(()));
                    }
                } else if kind.contains("CALL_RTPESTAB") {
                    let side = if is_sink_leg { "sink" } else { "source" };
                    logging::println_tag(&tag, &format!("{}: rtp established", side));
                } else if kind.contains("CALL_CLOSED") {
                    let side = if is_sink_leg { "sink" } else { "source" };
                    logging::println_tag(&tag, &format!("{}: closed {}", side, text));
                    if is_sink_leg {
                        let _ = ready_tx.send(Err(text.clone()));
                    }
                }
            }
        });
    }
    drop(ready_tx);

    // Log compiled-in codecs and dialing intent
    {
        let codecs = sip_shim::codecs_csv();
        logging::println_tag(&tag, &format!("codecs: {}", codecs));
        logging::println_tag(
            &tag,
            &format!("connect: dialing sink target={}", sink_label),
        );
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
    let wait_start = std::time::Instant::now();
    let wait_timeout = std::time::Duration::from_secs(10);
    loop {
        let remaining = wait_timeout
            .checked_sub(wait_start.elapsed())
            .unwrap_or_default();
        if remaining.is_zero() {
            anyhow::bail!("mixer: timeout waiting for sink leg to establish");
        }
        match ready_rx.recv_timeout(remaining) {
            Ok(Ok(())) => break,
            Ok(Err(reason)) => {
                anyhow::bail!("mixer: sink leg closed before ready ({})", reason);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => anyhow::bail!("mixer: lost event channel while waiting for sink leg"),
        }
    }
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
