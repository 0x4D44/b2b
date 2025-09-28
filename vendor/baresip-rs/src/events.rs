use crate::ffi;
use std::{ffi::CStr, os::raw::c_void, sync::mpsc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub code: i32,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Registering,
    RegisterOk,
    RegisterFail,
    Unregistering,
    FallbackOk,
    FallbackFail,
    Create,
    Shutdown,
    Exit,
    Module,
    Custom,
    Other(String),
}

impl Event {
    pub fn kind(&self) -> EventKind {
        // SAFETY: bevent_str expects a valid enum; passing code as c_int is fine for mapping purposes.
        let s_ptr = unsafe { ffi::bevent_str(self.code) };
        let s = text_from_ptr(s_ptr);
        match s.as_deref() {
            Some("REGISTERING") => EventKind::Registering,
            Some("REGISTER_OK") => EventKind::RegisterOk,
            Some("REGISTER_FAIL") => EventKind::RegisterFail,
            Some("UNREGISTERING") => EventKind::Unregistering,
            Some("FALLBACK_OK") => EventKind::FallbackOk,
            Some("FALLBACK_FAIL") => EventKind::FallbackFail,
            Some("CREATE") => EventKind::Create,
            Some("SHUTDOWN") => EventKind::Shutdown,
            Some("EXIT") => EventKind::Exit,
            Some("MODULE") => EventKind::Module,
            Some("CUSTOM") => EventKind::Custom,
            Some(other) => EventKind::Other(other.to_string()),
            None => EventKind::Other("?".into()),
        }
    }
}

#[derive(Debug)]
pub struct EventBridge {
    // Pointer to heap-allocated handler context (see HandlerCtx below)
    ctx_ptr: *mut HandlerCtx,
    // Extra allocation we own that backs ctx.chan (so we can drop it on unregister)
    chan_box_ptr: *mut c_void,
    reactor: crate::reactor::Reactor,
}

#[repr(C)]
struct HandlerCtx {
    // Delivery function (decides how to forward to the concrete channel type)
    deliver: unsafe fn(*mut c_void, Event),
    // Opaque pointer to concrete channel sender
    chan: *mut c_void,
}

impl EventBridge {
    pub(crate) fn register(reactor: &crate::reactor::Reactor) -> (Self, mpsc::Receiver<Event>) {
        Self::register_with_capacity(reactor, 0)
    }

    pub(crate) fn register_with_capacity(reactor: &crate::reactor::Reactor, cap: usize) -> (Self, mpsc::Receiver<Event>) {
        // Choose unbounded or bounded
        if cap == 0 {
            let (tx, rx) = mpsc::channel();
            let chan_box_ptr = Box::into_raw(Box::new(tx)) as *mut c_void;
            let ctx = HandlerCtx { deliver: deliver_std, chan: chan_box_ptr };
            let ctx_ptr = Box::into_raw(Box::new(ctx));
            let ctx_usize = ctx_ptr as usize;
            let r2 = reactor.clone();
            let (done_tx, done_rx) = mpsc::channel();
            let _ = reactor.execute(move || unsafe {
                let _ = ffi::bevent_register(Some(bevent_handler), ctx_usize as *mut c_void);
                let _ = done_tx.send(());
            });
            let _ = done_rx.recv_timeout(std::time::Duration::from_millis(200));
            (Self { ctx_ptr, chan_box_ptr, reactor: r2 }, rx)
        } else {
            let (tx, rx) = mpsc::sync_channel(cap);
            let chan_box_ptr = Box::into_raw(Box::new(tx)) as *mut c_void;
            let ctx = HandlerCtx { deliver: deliver_sync, chan: chan_box_ptr };
            let ctx_ptr = Box::into_raw(Box::new(ctx));
            let ctx_usize = ctx_ptr as usize;
            let r2 = reactor.clone();
            let (done_tx, done_rx) = mpsc::channel();
            let _ = reactor.execute(move || unsafe {
                let _ = ffi::bevent_register(Some(bevent_handler), ctx_usize as *mut c_void);
                let _ = done_tx.send(());
            });
            let _ = done_rx.recv_timeout(std::time::Duration::from_millis(200));
            (Self { ctx_ptr, chan_box_ptr, reactor: r2 }, rx)
        }
    }

    #[cfg(feature = "tokio")]
    pub fn register_tokio(reactor: &crate::reactor::Reactor, cap: usize) -> (Self, tokio::sync::mpsc::Receiver<Event>) {
        let (tx, rx) = tokio::sync::mpsc::channel(cap.max(1));
        let chan_box_ptr = Box::into_raw(Box::new(tx)) as *mut c_void;
        let ctx = HandlerCtx { deliver: deliver_tokio, chan: chan_box_ptr };
        let ctx_ptr = Box::into_raw(Box::new(ctx));
        let ctx_usize = ctx_ptr as usize;
        let r2 = reactor.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let _ = reactor.execute(move || unsafe {
            let _ = ffi::bevent_register(Some(bevent_handler), ctx_usize as *mut c_void);
            let _ = done_tx.send(());
        });
        let _ = done_rx.recv_timeout(std::time::Duration::from_millis(200));
        (Self { ctx_ptr, chan_box_ptr, reactor: r2 }, rx)
    }

    pub fn unregister(mut self) { self.unregister_impl(true); }
}

impl Drop for EventBridge {
    fn drop(&mut self) {
        self.unregister_impl(false);
    }
}

unsafe extern "C" fn bevent_handler(_ev: i32, event: *mut c_void, arg: *mut c_void) {
    if arg.is_null() { return; }
    let ctx = unsafe { &*(arg as usize as *mut HandlerCtx) };
    let code = unsafe { ffi::bevent_get_value(event as *const c_void) };
    let text_ptr = unsafe { ffi::bevent_get_text(event as *const c_void) };
    let text = text_from_ptr(text_ptr);
    unsafe { (ctx.deliver)(ctx.chan, Event { code, text }); }
}

// Delivery helpers (must be `unsafe fn` to match signature)
unsafe fn deliver_std(chan: *mut c_void, ev: Event) {
    let tx = unsafe { &*(chan as *mut mpsc::Sender<Event>) };
    let _ = tx.send(ev);
}

unsafe fn deliver_sync(chan: *mut c_void, ev: Event) {
    let tx = unsafe { &*(chan as *mut std::sync::mpsc::SyncSender<Event>) };
    let _ = tx.try_send(ev);
}

#[cfg(feature = "tokio")]
unsafe fn deliver_tokio(chan: *mut c_void, ev: Event) {
    let tx = unsafe { &*(chan as *mut tokio::sync::mpsc::Sender<Event>) };
    let _ = tx.try_send(ev);
}

impl EventBridge {
    fn unregister_impl(&mut self, wait: bool) {
        let (done_tx, done_rx) = mpsc::channel();
        let scheduled = self.reactor.execute(move || unsafe {
            ffi::bevent_unregister(Some(bevent_handler));
            let _ = done_tx.send(());
        }).is_ok();
        if scheduled && wait {
            let _ = done_rx.recv_timeout(std::time::Duration::from_millis(500));
        } else if !scheduled {
            unsafe { ffi::bevent_unregister(Some(bevent_handler)); }
        }
        unsafe {
            if !self.chan_box_ptr.is_null() { drop(Box::from_raw(self.chan_box_ptr)); self.chan_box_ptr = std::ptr::null_mut(); }
            if !self.ctx_ptr.is_null() { drop(Box::from_raw(self.ctx_ptr)); self.ctx_ptr = std::ptr::null_mut(); }
        }
    }
}

#[inline]
pub fn text_from_ptr(ptr: *const i8) -> Option<String> {
    if ptr.is_null() { return None; }
    // SAFETY: caller guarantees ptr points to a valid NUL-terminated C string
    unsafe { CStr::from_ptr(ptr) }.to_str().ok().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliver_std_sends_event() {
        let (tx, rx) = mpsc::channel();
        let boxed = Box::new(tx);
        let ptr = Box::into_raw(boxed) as *mut c_void;
        unsafe { super::deliver_std(ptr, Event { code: 1, text: Some("ok".into()) }); }
        let ev: Event = rx.recv().unwrap();
        assert_eq!(ev.code, 1);
        assert_eq!(ev.text.as_deref(), Some("ok"));
    }

    #[test]
    fn deliver_sync_does_not_block_when_full() {
        let (tx, rx) = mpsc::sync_channel(1);
        // Fill the channel
        tx.try_send(Event { code: 0, text: None }).unwrap();
        let boxed = Box::new(tx);
        let ptr = Box::into_raw(boxed) as *mut c_void;
        // This should not block even though the channel is full
        unsafe { super::deliver_sync(ptr, Event { code: 2, text: None }); }
        // Drain one item to ensure channel was indeed filled before
        let _ = rx.try_recv();
    }
}
