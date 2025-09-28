use baresip::{reactor::Options, BaresipContext, Reactor};
use std::time::Duration;
use std::ffi::CString;

#[test]
fn emits_module_event_and_receives() {
    // Initialize baresip core so event subsystem is active
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");

    let (bridge, rx) = reactor.register_events();

    // Emit a module event from inside the RE thread
    reactor.execute(|| unsafe {
        use baresip::ffi;
        let module = CString::new("test").unwrap();
        let ev = CString::new("hello").unwrap();
        let fmt = CString::new("%s").unwrap();
        let payload = CString::new("payload").unwrap();
        let _ = ffi::module_event(module.as_ptr(), ev.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), fmt.as_ptr(), payload.as_ptr());
    }).expect("emit");

    let ev = rx.recv_timeout(Duration::from_secs(2)).expect("recv event");
    assert!(ev.code >= 0);
    assert!(ev.text.is_some());

    drop(bridge);
    reactor.shutdown().expect("shutdown");
}
