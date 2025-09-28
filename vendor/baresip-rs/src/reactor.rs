//! Reactor and lifecycle scaffolding.

use crate::error::{ctry, Error, Result};
use crate::ffi;
use std::{ffi::CString, os::raw::c_void, ptr, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, thread};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub init_core: bool,
    pub init_ua: bool,
}

#[derive(Debug)]
pub struct BaresipContext {
    core_inited: bool,
    ua_inited: bool,
}

impl BaresipContext {
    pub fn new() -> Result<Self> {
        Self::new_with_options(Options::default())
    }

    pub fn new_with_options(opts: Options) -> Result<Self> {
        ctry(unsafe { ffi::libre_init() })?;
        let mut ctx = Self { core_inited: false, ua_inited: false };
        if opts.init_core {
            ctx.init_core()?;
        }
        if opts.init_ua {
            ctx.init_ua()?;
        }
        Ok(ctx)
    }

    pub fn init_core(&mut self) -> Result<()> {
        if self.core_inited { return Ok(()); }
        // Allow callers to supply a preloaded config (modules, sip_listen, etc.)
        if let Ok(conf) = std::env::var("BRS_CONF_BUF") {
            let bytes = conf.as_bytes();
            ctry(unsafe { ffi::conf_configure_buf(bytes.as_ptr(), bytes.len()) })?;
        } else {
            let buf = b"\n";
            ctry(unsafe { ffi::conf_configure_buf(buf.as_ptr(), buf.len()) })?;
        }
        let cfg = unsafe { ffi::conf_config() };
        if cfg.is_null() { return Err(Error::CErr(-1)); }
        ctry(unsafe { ffi::baresip_init(cfg) })?;
        // Load modules from the active configuration (e.g., g711, l16, g722)
        let _ = unsafe { ffi::conf_modules() };
        self.core_inited = true;
        Ok(())
    }

    pub fn init_ua(&mut self) -> Result<()> {
        if !self.core_inited { self.init_core()?; }
        if self.ua_inited { return Ok(()); }
        let sw = CString::new("baresip-rs").unwrap();
        // Disable TLS to avoid requiring OpenSSL linkage in minimal builds.
        ctry(unsafe { ffi::ua_init(sw.as_ptr(), true, true, false) })?;
        self.ua_inited = true;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Reactor {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    running: AtomicBool,
    mq: std::sync::atomic::AtomicPtr<ffi::mqueue>,
    th: Mutex<Option<thread::JoinHandle<()>>>,
    done_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Reactor {
    pub fn start(_ctx: &BaresipContext) -> Result<Self> {
        let inner = Arc::new(Inner {
            running: AtomicBool::new(true),
            mq: std::sync::atomic::AtomicPtr::new(ptr::null_mut()),
            th: Mutex::new(None),
            done_rx: Mutex::new(None),
        });
        let inner_clone = inner.clone();

        // Barrier to wait until mqueue is created
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let th = thread::Builder::new()
            .name("baresip-reactor".into())
            .spawn(move || {
                unsafe {
                    ffi::re_thread_enter();
                    let mut mq: *mut ffi::mqueue = ptr::null_mut();
                    let rc = ffi::mqueue_alloc(&mut mq as *mut _, Some(mqueue_handler), std::ptr::null_mut());
                    if rc != 0 { let _ = tx.send(Err(())); return; }
                    inner_clone.mq.store(mq, Ordering::SeqCst);
                    let _ = tx.send(Ok(()));
                    // From here on, re_main will pick up mqueue events.
                    // Run the RE main loop until cancelled
                    let _ = ffi::re_main(None);

                    // Cleanup state; RE loop ended
                    inner_clone.running.store(false, Ordering::SeqCst);
                    let _ = done_tx.send(());
                    ffi::re_thread_leave();
                }
            })
            .map_err(|_| Error::Spawn)?;

        // wait for mqueue
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(())) => return Err(Error::Spawn),
            Err(_) => return Err(Error::Spawn),
        }
        // Ensure re_main has actually started by queueing a barrier and waiting for it.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let inner_for_barrier = inner.clone();
        {
            let reactor_tmp = Self { inner: inner_for_barrier };
            reactor_tmp.execute(move || { let _ = started_tx.send(()); })?;
        }
        started_rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| Error::Timeout)?;
        *inner.th.lock().unwrap() = Some(th);
        *inner.done_rx.lock().unwrap() = Some(done_rx);
        Ok(Self { inner })
    }

    pub fn execute<F>(&self, f: F) -> Result<()> where F: FnOnce() + Send + 'static {
        let holder = Box::new(ClosureHolder { f: Some(Box::new(f)) });
        let raw: *mut ClosureHolder = Box::into_raw(holder);
        let mq = self.inner.mq.load(Ordering::SeqCst);
        if mq.is_null() { unsafe { drop(Box::from_raw(raw)); } return Err(Error::Closed); }
        let rc = unsafe { ffi::mqueue_push(mq, 0, raw as *mut c_void) };
        if rc != 0 { unsafe { drop(Box::from_raw(raw)); } return Err(Error::CErr(rc)); }
        Ok(())
    }

    #[cfg(feature = "tokio")]
    pub async fn execute_async<F>(&self, f: F) -> Result<()> 
    where F: FnOnce() + Send + 'static {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.execute(move || {
            f();
            let _ = tx.send(());
        })?;
        let _ = rx.await;
        Ok(())
    }

    pub fn register_events(&self) -> (crate::events::EventBridge, std::sync::mpsc::Receiver<crate::events::Event>) {
        crate::events::EventBridge::register(self)
    }

    pub fn register_events_with_capacity(&self, cap: usize) -> (crate::events::EventBridge, std::sync::mpsc::Receiver<crate::events::Event>) {
        crate::events::EventBridge::register_with_capacity(self, cap)
    }

    #[cfg(feature = "tokio")]
    pub fn register_events_tokio(&self, cap: usize) -> (crate::events::EventBridge, tokio::sync::mpsc::Receiver<crate::events::Event>) {
        crate::events::EventBridge::register_tokio(self, cap)
    }

    fn shutdown_timeout_ms() -> u64 {
        std::env::var("BRS_SHUTDOWN_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000)
    }

    pub fn shutdown(&self) -> Result<()> {
        // Signal cancellation from inside the RE thread to avoid readiness races
        let (sig_tx, sig_rx) = std::sync::mpsc::channel();
        let _ = self.execute(move || {
            unsafe { ffi::re_cancel(); }
            let _ = sig_tx.send(());
        });
        let _ = sig_rx.recv_timeout(std::time::Duration::from_millis(200));
        if let Some(rx) = self.inner.done_rx.lock().unwrap().take() {
            // wait for configured timeout
            let timeout = std::time::Duration::from_millis(Self::shutdown_timeout_ms());
            if rx.recv_timeout(timeout).is_err() {
                return Err(Error::Timeout);
            }
        }
        if let Some(th) = self.inner.th.lock().unwrap().take() { let _ = th.join(); }
        // mark mqueue as closed for future execute() calls
        self.inner.mq.store(std::ptr::null_mut(), Ordering::SeqCst);
        // Close UA/core and libre if initialized
        unsafe { ffi::ua_close(); }
        unsafe { ffi::baresip_close(); }
        unsafe { ffi::libre_close(); }
        Ok(())
    }
}

#[repr(C)]
struct ClosureHolder { f: Option<Box<dyn FnOnce() + Send + 'static>> }

unsafe extern "C" fn mqueue_handler(_id: i32, data: *mut c_void, _arg: *mut c_void) {
    // Reconstruct closure
    if !data.is_null() {
        unsafe {
            let holder: Box<ClosureHolder> = Box::from_raw(data as *mut ClosureHolder);
            let run = holder.f;
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(f) = run { f(); }
            }));
        }
    }
    // no-op: arg unused
}
