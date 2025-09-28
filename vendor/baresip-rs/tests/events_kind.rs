use baresip::{reactor::Options, BaresipContext, Reactor};
use baresip::events::EventKind;
use std::ffi::CString;
use std::time::Duration;

#[test]
#[ignore]
fn module_event_maps_to_kind_module() {
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let (_bridge, rx) = reactor.register_events();
    reactor.execute(|| unsafe {
        use baresip::ffi;
        let module = CString::new("test").unwrap();
        let ev = CString::new("unit").unwrap();
        let fmt = CString::new("%s").unwrap();
        let payload = CString::new("x").unwrap();
        let _ = ffi::module_event(module.as_ptr(), ev.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), fmt.as_ptr(), payload.as_ptr());
    }).expect("emit");
    let ev = rx.recv_timeout(Duration::from_secs(2)).expect("recv");
    assert_eq!(ev.kind(), EventKind::Module);
    reactor.shutdown().expect("shutdown");
}

#[test]
fn create_event_maps_to_kind_create() {
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let (_bridge, rx) = reactor.register_events();
    // Initialize UA stack and allocate a UA inside RE thread to trigger CREATE
    reactor.execute(|| unsafe {
        use baresip::ffi;
        let sw = CString::new("ut").unwrap();
        let _ = ffi::ua_init(sw.as_ptr(), true, true, true);
        let mut ua: *mut std::os::raw::c_void = std::ptr::null_mut();
        let aor = CString::new("Alice <sip:alice@127.0.0.1>;regint=0").unwrap();
        let _ = ffi::ua_alloc(&mut ua as *mut _, aor.as_ptr());
        if !ua.is_null() { let _ = ffi::ua_destroy(ua); }
        ffi::ua_close();
    }).expect("ua_init/ua_alloc");

    // Find a CREATE event in the next few messages
    let mut found = false;
    for _ in 0..5 {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(500)) {
            if ev.kind() == EventKind::Create { found = true; break; }
        }
    }
    assert!(found, "expected CREATE event");
    reactor.shutdown().expect("shutdown");
}
