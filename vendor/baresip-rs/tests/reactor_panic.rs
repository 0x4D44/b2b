use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[test]
fn panic_in_job_does_not_kill_reactor() {
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    // This will panic inside the RE thread; handler catches unwind.
    let _ = reactor.execute(|| { panic!("job panicked"); });
    // After panic, reactor must still be able to run another job.
    let hit = Arc::new(AtomicUsize::new(0));
    let h2 = hit.clone();
    reactor.execute(move || { h2.fetch_add(1, Ordering::SeqCst); }).expect("exec");
    reactor.shutdown().expect("shutdown");
    assert_eq!(hit.load(Ordering::SeqCst), 1);
}

