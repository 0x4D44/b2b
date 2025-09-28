use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[test]
fn stress_many_execute_and_shutdown() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let hits = Arc::new(AtomicUsize::new(0));
    // Push in batches to avoid mqueue overflow; allow RE loop to drain.
    let total = 5_000usize;
    for i in 0..total {
        loop {
            let h = hits.clone();
            match reactor.execute(move || { h.fetch_add(1, Ordering::Relaxed); }) {
                Ok(()) => break,
                Err(_) => std::thread::sleep(std::time::Duration::from_micros(200)),
            }
        }
        if i % 200 == 0 { std::thread::sleep(std::time::Duration::from_millis(1)); }
    }
    reactor.shutdown().expect("shutdown");
    assert_eq!(hits.load(Ordering::Relaxed), total);
}
