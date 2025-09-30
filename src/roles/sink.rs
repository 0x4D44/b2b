use crate::{
    cli::{BufferMode, Cli, JbufType},
    logging,
};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};

static RX_SAMPLES: AtomicU64 = AtomicU64::new(0);
static FIRST_PCM: AtomicBool = AtomicBool::new(false);
static SINK_PCM_USER: AtomicPtr<std::os::raw::c_void> = AtomicPtr::new(std::ptr::null_mut());
use crate::{sip::UaHandle, sip_shim};

pub fn run(args: &Cli) -> Result<()> {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let tag = logging::role_tag("sink");
    logging::println_tag(&tag, "init UA + aplay (skeleton)");

    // Preload baresip configuration for sink: modules + listen + call_accept
    let sip = args.sip_bind.as_deref().unwrap_or("0.0.0.0:5062");
    let buf_mode = match args.sink_buffer_mode {
        BufferMode::Fixed => "fixed",
        BufferMode::Adaptive => "adaptive",
    };
    let jtype = match args.sink_jbuf_type {
        JbufType::Off => "off",
        JbufType::Fixed => "fixed",
        JbufType::Adaptive => "adaptive",
    };
    let conf = format!(
        "module\t\tg711\n\
         module\t\tl16\n\
         sip_listen\t{sip}\n\
         call_accept\tyes\n\
         audio_buffer\t{bmin}-{bmax}\n\
         audio_buffer_mode\t{bmode}\n\
         audio_jitter_buffer_type\t{jtype}\n\
         audio_jitter_buffer_ms\t{jmin}-{jmax}\n",
        sip = sip,
        bmin = args.sink_buffer_min_ms,
        bmax = args.sink_buffer_max_ms,
        bmode = buf_mode,
        jtype = jtype,
        jmin = args.sink_jbuf_min_ms,
        jmax = args.sink_jbuf_max_ms
    );
    unsafe {
        std::env::set_var("BRS_CONF_BUF", conf);
    }

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
                logging::println_tag(
                    &tag,
                    &format!("bevent kind={:?} text={:?}", ev.kind(), ev.text),
                );
            }
        });
    }
    sip_shim::sink_init(sip)?;
    // Clear preloaded config var to avoid affecting other roles
    unsafe {
        std::env::remove_var("BRS_CONF_BUF");
    }

    // Start aplay and keep stdin open for future PCM writes.
    let mut aplay = spawn_aplay(&args.aplay_cmd).context("spawn aplay")?;

    logging::ready_line("sink", sip, &args.codec, args.ptime_ms);

    // Wire shim PCM callback to a bounded channel and a dedicated writer thread.
    if let Some(stdin) = aplay.stdin.take() {
        use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
        let (tx, rx): (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) = sync_channel(256);

        std::thread::spawn(move || {
            let mut w = stdin;
            while let Ok(buf) = rx.recv() {
                let _ = w.write_all(&buf);
            }
        });

        extern "C" fn on_pcm(samples: *const i16, ns: usize, user: *mut std::os::raw::c_void) {
            if samples.is_null() || ns == 0 || user.is_null() {
                return;
            }
            let slice = unsafe { std::slice::from_raw_parts(samples, ns) };
            let tx = unsafe { &*(user as *const std::sync::mpsc::SyncSender<Vec<u8>>) };
            let ptr = slice.as_ptr() as *const u8;
            let bytes = unsafe { std::slice::from_raw_parts(ptr, ns * 2) };
            let mut v = Vec::with_capacity(ns * 2);
            v.extend_from_slice(bytes);
            match tx.try_send(v) {
                Ok(_) => {}
                Err(TrySendError::Full(_)) => { /* drop to avoid blocking */ }
                Err(TrySendError::Disconnected(_)) => return,
            }
            RX_SAMPLES.fetch_add(ns as u64, Ordering::Relaxed);
            if !FIRST_PCM.swap(true, Ordering::SeqCst) {
                let tag = crate::logging::role_tag("sink");
                crate::logging::println_tag(
                    &tag,
                    &format!("MEDIA: first PCM decoded ({} samples)", ns),
                );
            }
        }
        let user_ptr = Box::into_raw(Box::new(tx)) as *mut _;
        SINK_PCM_USER.store(user_ptr, Ordering::Release);
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
                logging::println_tag(
                    &tag,
                    &format!("rx_samples={} (+{}), rx_frames+{}", now, delta, frames),
                );
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

    // Free the boxed SyncSender to prevent memory leak
    let user_ptr = SINK_PCM_USER.swap(std::ptr::null_mut(), Ordering::Acquire);
    if !user_ptr.is_null() {
        unsafe {
            use std::sync::mpsc::SyncSender;
            let _boxed = Box::from_raw(user_ptr as *mut SyncSender<Vec<u8>>);
            // Drop occurs here automatically, closing the channel
        }
    }

    Ok(())
}

fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || {
        let _ = tx.send(());
    });
    let _ = rx.recv();
}

fn spawn_aplay(cmdline: &str) -> Result<Child> {
    // Use a shell to interpret the provided command string.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(cmdline)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let child = cmd
        .spawn()
        .with_context(|| format!("spawning aplay: {cmdline}"))?;
    Ok(child)
}

// no non-sip fallback anymore
