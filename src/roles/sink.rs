use crate::{cli::Cli, logging};
use anyhow::{Context, Result};
use std::{io::Write, process::{Child, Command, Stdio}};
use std::sync::atomic::{AtomicU64, Ordering};

static RX_SAMPLES: AtomicU64 = AtomicU64::new(0);
use crate::{sip::UaHandle, sip_shim};

pub fn run(args: &Cli) -> Result<()> {
    let tag = logging::role_tag("sink");
    logging::println_tag(&tag, "init UA + aplay (skeleton)");

    // Preload baresip configuration for sink: modules + listen + call_accept
    let sip = args.sip_bind.as_deref().unwrap_or("0.0.0.0:5062");
    let conf = format!(
        "module\t\tg711\n\
         module\t\tl16\n\
         sip_listen\t{}\n\
         call_accept\tyes\n",
        sip
    );
    unsafe { std::env::set_var("BRS_CONF_BUF", conf); }

    // Start baresip reactor (UA/core) and shim init
    let ua = UaHandle::init().context("init UA")?;
    let (_bridge, rx) = ua.reactor.register_events();
    {
        let codecs = crate::sip_shim::codecs_csv();
        logging::println_tag(&tag, &format!("codecs: {}", codecs));
    }
    {
        let tag = logging::role_tag("sink");
        std::thread::spawn(move || {
            while let Ok(ev) = rx.recv() {
                logging::println_tag(&tag, &format!("bevent kind={:?} text={:?}", ev.kind(), ev.text));
            }
        });
    }
    sip_shim::sink_init(sip)?;
    // Clear preloaded config var to avoid affecting other roles
    unsafe { std::env::remove_var("BRS_CONF_BUF"); }

    // Start aplay and keep stdin open for future PCM writes.
    let mut aplay = spawn_aplay(&args.aplay_cmd).context("spawn aplay")?;

    logging::ready_line("sink", sip, &args.codec, args.ptime_ms);

    // Wire shim PCM callback to aplay stdin.
    if let Some(stdin) = aplay.stdin.take() {
        use std::sync::{Arc, Mutex};
        let writer = Arc::new(Mutex::new(stdin));
        let writer_cb = writer.clone();
        extern "C" fn on_pcm(samples: *const i16, ns: usize, user: *mut std::os::raw::c_void) {
            if samples.is_null() || ns == 0 || user.is_null() { return; }
            let slice = unsafe { std::slice::from_raw_parts(samples, ns) };
            let writer = unsafe { &*(user as *const std::sync::Mutex<std::process::ChildStdin>) };
            if let Ok(mut w) = writer.lock() {
                // Convert i16 samples to bytes
                let ptr = slice.as_ptr() as *const u8;
                let bytes = unsafe { std::slice::from_raw_parts(ptr, ns * 2) };
                let _ = w.write_all(bytes);
                let _ = w.flush();
            }
            RX_SAMPLES.fetch_add(ns as u64, Ordering::Relaxed);
        }
        let user_ptr = Arc::into_raw(writer_cb) as *mut _;
        sip_shim::sink_set_pcm_callback(on_pcm, user_ptr)?;
    }

    // Periodic metrics log
    {
        let tag = logging::role_tag("sink");
        std::thread::spawn(move || {
            let mut last = 0u64;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let now = RX_SAMPLES.load(Ordering::Relaxed);
                let delta = now.saturating_sub(last);
                last = now;
                let frames = delta / 160; // 20ms @ 8kHz mono
                logging::println_tag(&tag, &format!("rx_samples={} (+{}), rx_frames+{}", now, delta, frames));
            }
        });
    }

    wait_for_ctrl_c();
    logging::println_tag(&tag, "Ctrl+C received; shutting down");

    logging::println_tag(&tag, "shutdown: UA + aplay");
    // Best-effort shutdown
    let _ = sip_shim::sink_shutdown();
    let _ = ua.reactor.shutdown();
    let _ = aplay.kill();
    Ok(())
}

fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || { let _ = tx.send(()); });
    let _ = rx.recv();
}

fn spawn_aplay(cmdline: &str) -> Result<Child> {
    // Use a shell to interpret the provided command string.
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(cmdline).stdin(Stdio::piped()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let child = cmd.spawn().with_context(|| format!("spawning aplay: {cmdline}"))?;
    Ok(child)
}

// no non-sip fallback anymore
