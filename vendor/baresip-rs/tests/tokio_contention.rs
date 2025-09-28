// Runs by default; tokio is now a default feature.

use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_execute_async_complete() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let hits = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let r = reactor.clone();
        let h = hits.clone();
        tasks.push(tokio::spawn(async move {
            r.execute_async(move || { h.fetch_add(1, Ordering::Relaxed); }).await.unwrap();
        }));
    }
    for t in tasks { let _ = t.await; }
    reactor.shutdown().expect("shutdown");
    assert_eq!(hits.load(Ordering::Relaxed), 100);
}
