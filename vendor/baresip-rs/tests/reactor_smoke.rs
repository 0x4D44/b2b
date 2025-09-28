// Always run: this test only uses libre loop and mqueue.

use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[test]
fn reactor_executes_job_and_shuts_down() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let hit = Arc::new(AtomicUsize::new(0));
    let h2 = hit.clone();
    reactor.execute(move || { h2.fetch_add(1, Ordering::SeqCst); }).expect("execute");
    // Request shutdown
    reactor.shutdown().expect("shutdown");
    assert_eq!(hit.load(Ordering::SeqCst), 1);
}
