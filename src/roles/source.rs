use crate::{cli::Cli, logging, media, sip::UaHandle, sip_shim};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{thread, time::Duration};

static TX_SAMPLES: AtomicU64 = AtomicU64::new(0);

pub fn run(args: &Cli) -> Result<()> {
    let tag = logging::role_tag("source");
    logging::println_tag(&tag, "starting (mp3 decode + prebuffer skeleton)");

    // Initialize UA/reactor and start outbound call using shim ausrc.
    // Ensure PCMU (g711) module is loaded before UA init via preloaded config
    unsafe {
        std::env::set_var("BRS_CONF_BUF", "module\tg711\nmodule\tl16\n");
    }
    let ua = UaHandle::init().context("init UA")?;
    // Single event registration; we multiplex for logging and readiness
    let (_bridge_ev, rx_ev) = ua.reactor.register_events();
    {
        let codecs = crate::sip_shim::codecs_csv();
        let tag = logging::role_tag("source");
        logging::println_tag(&tag, &format!("codecs: {}", codecs));
    }

    // Event processor thread: logs milestones and signals establishment/close
    let (sig_tx, sig_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    {
        let tag = logging::role_tag("source");
        std::thread::spawn(move || {
            while let Ok(ev) = rx_ev.recv() {
                let kind = format!("{:?}", ev.kind());
                let text = ev.text.clone().unwrap_or_default();
                if kind.contains("CALL_LOCAL_SDP") {
                    logging::println_tag(&tag, "SDP: Sent offer");
                } else if kind.contains("CALL_REMOTE_SDP") {
                    logging::println_tag(&tag, "SDP: Received answer");
                } else if kind.contains("CALL_PROGRESS") || text.contains("180 Ringing") {
                    logging::println_tag(&tag, "SIP: Ringing (180)");
                } else if kind.contains("CALL_RTPESTAB") {
                    logging::println_tag(&tag, "RTP: Flow established");
                } else if kind.contains("CALL_ESTABLISHED") {
                    logging::println_tag(&tag, "SIP: Call established");
                    let _ = sig_tx.send(Ok(()));
                } else if kind.contains("CALL_CLOSED") {
                    logging::println_tag(&tag, &format!("SIP: Call closed ({})", text));
                    let _ = sig_tx.send(Err(text));
                }
            }
        });
    }

    let target = args.target.as_deref().unwrap_or("sip:127.0.0.1:0");
    {
        let tag = logging::role_tag("source");
        logging::println_tag(&tag, &format!("Dialing target: {}", target));
    }
    sip_shim::source_start(target, 8000, 1, args.ptime_ms)?;
    unsafe {
        std::env::remove_var("BRS_CONF_BUF");
    }

    // Wait for call to be established or closed before declaring READY
    let tag_ready = logging::role_tag("source");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10_000);
    let established = loop {
        let timeout = deadline.saturating_duration_since(std::time::Instant::now());
        if timeout.is_zero() {
            break false;
        }
        match sig_rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(Ok(())) => break true,
            Ok(Err(reason)) => {
                logging::println_tag(&tag_ready, &format!("call setup failed: {}", reason));
                return Err(anyhow::anyhow!("call setup failed: {}", reason));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break false,
        }
    };
    if !established {
        return Err(anyhow::anyhow!(
            "call setup timeout waiting for ESTABLISHED"
        ));
    }

    // Decode audio file if provided; otherwise synthesize silence frames.
    let frames: Vec<Vec<i16>> = if let Some(path) = &args.audio_file {
        let pcm = media::decode_mp3_to_pcm_8k(path)
            .with_context(|| format!("decode {}", path.display()))?;
        media::split_into_20ms_frames(&pcm.data, pcm.sample_rate)
    } else {
        vec![vec![0i16; 160]; 50 * 5] // 5s of silence @ 8kHz mono, 20ms frames
    };

    let prebuffer_frames = (args.prebuffer_ms as usize / args.ptime_ms as usize).max(1);
    // Use a cyclic index instead of a producer thread to avoid busy loop
    let mut idx = 0usize;

    // Prime C-side aubuf with prebuffer frames but keep TX gated off.
    {
        let mut primed = 0usize;
        while primed < prebuffer_frames {
            let frame = &frames[idx];
            let _ = sip_shim::source_push_pcm(frame);
            idx = (idx + 1) % frames.len();
            primed += 1;
        }
    }

    // Small pre-roll to allow remote jitter/rtp to settle
    std::thread::sleep(std::time::Duration::from_millis(args.preroll_ms as u64));
    // Enable TX now that buffer is primed
    let _ = sip_shim::source_tx_enable(true);
    logging::ready_line("source", target, &args.codec, args.ptime_ms);

    // Start metrics thread before main loop
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let metrics_handle = {
        let tag = logging::role_tag("source");
        std::thread::spawn(move || {
            let mut last = 0u64;
            loop {
                match stop_rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(_) => break, // Stop signal received
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let now = TX_SAMPLES.load(Ordering::Relaxed);
                        let delta = now.saturating_sub(last);
                        last = now;
                        let frames = delta / 160;
                        logging::println_tag(
                            &tag,
                            &format!("tx_samples={} (+{}), tx_frames+{}", now, delta, frames),
                        );
                    }
                    Err(_) => break, // Channel disconnected
                }
            }
        })
    };

    // Maintain a healthy backlog in C-side aubuf; let C ausrc pace RTP
    let target_backlog_ms: u32 = (args.prebuffer_ms).min(500); // avoid excessive memory
    loop {
        // Top-up loop: push frames rapidly until backlog >= target
        let mut loops = 0;
        while sip_shim::source_backlog_ms() < target_backlog_ms {
            let frame = &frames[idx];
            let _ = sip_shim::source_push_pcm(frame);
            TX_SAMPLES.fetch_add(frame.len() as u64, Ordering::Relaxed);
            idx = (idx + 1) % frames.len();
            loops += 1;
            if loops > 50 {
                break;
            } // avoid monopolizing CPU; yield soon
        }
        // Sleep a bit; C ausrc will drain at 20ms/frame
        thread::sleep(Duration::from_millis(10));
        // break only on ctrl-c
        if ctrlc_tripped() {
            logging::println_tag(&tag, "Ctrl+C received; shutting down");
            break;
        }
    }

    // Stop metrics thread gracefully
    drop(stop_tx); // Signal thread to stop
    let _ = metrics_handle.join();

    logging::println_tag(&tag, "shutdown");
    Ok(())
}

fn ctrlc_tripped() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    static HIT: AtomicBool = AtomicBool::new(false);
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let _ = ctrlc::set_handler(|| {
            HIT.store(true, Ordering::SeqCst);
        });
    });

    HIT.load(Ordering::SeqCst)
}
