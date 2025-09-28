use baresip::{reactor::Options, BaresipContext, Reactor};
use std::time::Duration;
use std::ffi::CString;

#[test]
fn bounded_channel_does_not_block_and_drops_when_full() {
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let (bridge, rx) = reactor.register_events_with_capacity(4);

    // Emit many events quickly from RE thread
    reactor.execute(|| unsafe {
        use baresip::ffi;
        let module = CString::new("test").unwrap();
        let ev = CString::new("flood").unwrap();
        let fmt = CString::new("%s").unwrap();
        let payload = CString::new("x").unwrap();
        for _ in 0..100 {
            let _ = ffi::module_event(module.as_ptr(), ev.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), fmt.as_ptr(), payload.as_ptr());
        }
    }).expect("emit");

    // We expect to receive some events (>=1), but not necessarily all 100.
    let mut count = 0;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(200) {
        if let Ok(_e) = rx.recv_timeout(Duration::from_millis(5)) { count += 1; }
    }
    assert!(count >= 1, "expected at least one event, got {count}");
    drop(bridge);
    reactor.shutdown().expect("shutdown");
}
