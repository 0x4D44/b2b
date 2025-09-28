use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn execute_after_shutdown_from_multiple_threads_returns_closed() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    reactor.shutdown().expect("shutdown");

    let r = reactor.clone();
    let t = 8;
    let barrier = Arc::new(Barrier::new(t));
    let mut handles = Vec::new();
    for _ in 0..t {
        let b = barrier.clone();
        let r2 = r.clone();
        handles.push(thread::spawn(move || {
            b.wait();
            let res = r2.execute(|| {});
            assert!(res.is_err());
        }));
    }
    for h in handles { let _ = h.join(); }
}

