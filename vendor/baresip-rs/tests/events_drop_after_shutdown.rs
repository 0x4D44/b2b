use baresip::{reactor::Options, BaresipContext, Reactor};

#[test]
fn drop_bridge_after_shutdown_does_not_hang() {
    let ctx = BaresipContext::new_with_options(Options { init_core: true, init_ua: false }).expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let (bridge, _rx) = reactor.register_events();
    // Shutdown reactor first
    reactor.shutdown().expect("shutdown");
    // Then drop the bridge — should not hang
    drop(bridge);
}

