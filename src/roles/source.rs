use crate::{cli::Cli, logging, media, sip::UaHandle, sip_shim};
use anyhow::{Context, Result};
use std::{sync::{Arc, Mutex}, thread, time::Duration};
use std::sync::atomic::{AtomicU64, Ordering};

static TX_SAMPLES: AtomicU64 = AtomicU64::new(0);

pub fn run(args: &Cli) -> Result<()> {
    let tag = logging::role_tag("source");
    logging::println_tag(&tag, "starting (mp3 decode + prebuffer skeleton)");

    // Initialize UA/reactor and start outbound call using shim ausrc.
    // Ensure PCMU (g711) module is loaded before UA init via preloaded config
    unsafe { std::env::set_var("BRS_CONF_BUF", "module\tg711\nmodule\tl16\n"); }
    let ua = UaHandle::init().context("init UA")?;
    // Register a channel dedicated to readiness wait
    let (_bridge_ready, rx_ready) = ua.reactor.register_events();
    {
        let codecs = crate::sip_shim::codecs_csv();
        let tag = logging::role_tag("source");
        logging::println_tag(&tag, &format!("codecs: {}", codecs));
    }

    // Start filtered event logger BEFORE dialing so we see early milestones.
    let (_bridge_log, rx_log) = ua.reactor.register_events();
    {
        let tag = logging::role_tag("source");
        std::thread::spawn(move || {
            while let Ok(ev) = rx_log.recv() {
                let kind = format!("{:?}", ev.kind());
                let text = ev.text.clone().unwrap_or_default();
                if kind.contains("CALL_LOCAL_SDP") { logging::println_tag(&tag, "SDP: Sent offer"); continue; }
                if kind.contains("CALL_REMOTE_SDP") { logging::println_tag(&tag, "SDP: Received answer"); continue; }
                if kind.contains("CALL_PROGRESS") || text.contains("180 Ringing") { logging::println_tag(&tag, "SIP: Ringing (180)"); continue; }
                if kind.contains("CALL_RTPESTAB") { logging::println_tag(&tag, "RTP: Flow established"); continue; }
                if kind.contains("CALL_ESTABLISHED") { logging::println_tag(&tag, "SIP: Call established"); continue; }
                if kind.contains("CALL_CLOSED") { logging::println_tag(&tag, &format!("SIP: Call closed ({})", text)); continue; }
            }
        });
    }

    let target = args.target.as_deref().unwrap_or("sip:127.0.0.1:0");
    sip_shim::source_start(target, 8000, 1, args.ptime_ms)?;
    unsafe { std::env::remove_var("BRS_CONF_BUF"); }

    // Wait for call to be established or closed before declaring READY
    let tag_ready = logging::role_tag("source");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10_000);
    let mut established = false;
    while std::time::Instant::now() < deadline {
        if let Ok(ev) = rx_ready.recv_timeout(std::time::Duration::from_millis(200)) {
            let txt = ev.text.clone().unwrap_or_default();
            let kind = format!("{:?}", ev.kind());
            if txt.contains("CALL_ESTABLISHED") || kind.contains("CALL_ESTABLISHED") {
                established = true;
                break;
            }
            if txt.contains("CALL_CLOSED") || kind.contains("CALL_CLOSED") {
                logging::println_tag(&tag_ready, &format!("call setup failed: {}", txt));
                // Non-zero exit to signal orchestrator
                return Err(anyhow::anyhow!("call setup failed: {}", txt));
            }
        }
    }
    if !established {
        return Err(anyhow::anyhow!("call setup timeout waiting for ESTABLISHED"));
    }

    // Decode audio file if provided; otherwise synthesize silence frames.
    let frames: Vec<Vec<i16>> = if let Some(path) = &args.audio_file {
        let pcm = media::decode_mp3_to_pcm_8k(path).with_context(|| format!("decode {}", path.display()))?;
        media::split_into_20ms_frames(&pcm.data, pcm.sample_rate)
    } else {
        vec![vec![0i16; 160]; 50 * 5] // 5s of silence @ 8kHz mono, 20ms frames
    };

    let prebuffer_frames = (args.prebuffer_ms as usize / args.ptime_ms as usize).max(1);

    let queue: Arc<Mutex<Vec<Vec<i16>>>> = Arc::new(Mutex::new(Vec::new()));
    let q_prod = queue.clone();
    thread::spawn(move || {
        loop {
            for f in &frames { if let Ok(mut q) = q_prod.lock() { q.push(f.clone()); } }
        }
    });

    // Wait until prebuffer is ready
    loop {
        if let Ok(q) = queue.lock() {
            if q.len() >= prebuffer_frames { break; }
        }
        thread::sleep(Duration::from_millis(10));
    }

    logging::ready_line("source", target, &args.codec, args.ptime_ms);

    // Send cadence: push frames to shim at ptime.
    loop {
        if let Ok(mut q) = queue.lock() {
            if !q.is_empty() {
                if let Some(frame) = q.get(0) {
                    let _ = sip_shim::source_push_pcm(frame);
                    TX_SAMPLES.fetch_add(frame.len() as u64, Ordering::Relaxed);
                }
                q.remove(0);
            }
        }
        thread::sleep(Duration::from_millis(args.ptime_ms as u64));
        // break only on ctrl-c
        if ctrlc_tripped() {
            logging::println_tag(&tag, "Ctrl+C received; shutting down");
            break;
        }
    }

    // Periodic metrics log
    {
        let tag = logging::role_tag("source");
        std::thread::spawn(move || {
            let mut last = 0u64;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let now = TX_SAMPLES.load(Ordering::Relaxed);
                let delta = now.saturating_sub(last);
                last = now;
                let frames = delta / 160;
                logging::println_tag(&tag, &format!("tx_samples={} (+{}), tx_frames+{}", now, delta, frames));
            }
        });
    }

    logging::println_tag(&tag, "shutdown");
    Ok(())
}

fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || { let _ = tx.send(()); });
    let _ = rx.recv();
}

fn ctrlc_tripped() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static HIT: AtomicBool = AtomicBool::new(false);
    let _ = ctrlc::set_handler(|| { HIT.store(true, Ordering::SeqCst); });
    HIT.load(Ordering::SeqCst)
}
