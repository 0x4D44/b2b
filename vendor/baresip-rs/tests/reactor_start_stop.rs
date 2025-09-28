use baresip::{BaresipContext, Reactor};

#[test]
fn start_stop_loop_many_times() {
    for _ in 0..30 {
        let ctx = BaresipContext::new().expect("bootstrap");
        let reactor = Reactor::start(&ctx).expect("start");
        reactor.shutdown().expect("shutdown");
    }
}

