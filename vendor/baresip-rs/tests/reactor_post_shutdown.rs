use baresip::{BaresipContext, Reactor};

#[test]
fn execute_after_shutdown_errors() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    reactor.shutdown().expect("shutdown");
    let res = reactor.execute(|| {});
    assert!(res.is_err());
}

