// Runs by default; requires tokio which is now a default feature.

use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[tokio::test(flavor = "current_thread")] 
async fn execute_async_runs() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let hit = Arc::new(AtomicUsize::new(0));
    let h2 = hit.clone();
    reactor.execute_async(move || { h2.fetch_add(1, Ordering::SeqCst); }).await.expect("exec");
    reactor.shutdown().expect("shutdown");
    assert_eq!(hit.load(Ordering::SeqCst), 1);
}
