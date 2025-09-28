use baresip::{reactor::Options, BaresipContext, Reactor};
use std::ffi::CString;

#[tokio::test(flavor = "current_thread")] 
async fn tokio_adapter_receives() {
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let (_bridge, mut rx) = reactor.register_events_tokio(8);

    reactor.execute(|| unsafe {
        use baresip::ffi;
        let module = CString::new("test").unwrap();
        let ev = CString::new("tokio").unwrap();
        let fmt = CString::new("%s").unwrap();
        let payload = CString::new("ok").unwrap();
        let _ = ffi::module_event(module.as_ptr(), ev.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), fmt.as_ptr(), payload.as_ptr());
    }).expect("emit");

    let ev = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await.expect("timeout");
    assert!(ev.is_some());
    reactor.shutdown().expect("shutdown");
}

