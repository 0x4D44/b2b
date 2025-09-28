use baresip::{reactor::Options, BaresipContext, Reactor};
use std::time::{Duration, Instant};

#[test]
fn unregister_returns_quickly_even_if_re_thread_busy() {
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let (bridge, _rx) = reactor.register_events();
    // Block RE thread for ~1s
    reactor.execute(|| {
        let until = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < until { std::thread::sleep(Duration::from_millis(10)); }
    }).expect("block");

    let start = Instant::now();
    bridge.unregister();
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(600), "unregister took too long: {:?}", elapsed);

    reactor.shutdown().expect("shutdown");
}

