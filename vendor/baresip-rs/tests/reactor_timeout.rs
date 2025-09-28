use baresip::{BaresipContext, Reactor};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

#[test]
fn shutdown_timeout_when_loop_blocked() {
    // Set aggressive timeout for this test
    unsafe { std::env::set_var("BRS_SHUTDOWN_TIMEOUT_MS", "50") };
    let ctx = BaresipContext::new().expect("bootstrap");
    let reactor = Reactor::start(&ctx).expect("start");
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    reactor.execute(move || {
        // Block the RE thread for ~600ms, exceeding configured shutdown timeout
        let until = std::time::Instant::now() + Duration::from_millis(600);
        while std::time::Instant::now() < until && !stop2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }).expect("exec");

    let res = reactor.shutdown();
    assert!(res.is_err(), "expected timeout");
    // Unblock and perform a clean shutdown on second attempt
    stop.store(true, Ordering::SeqCst);
    // Give the RE thread a moment to finish the blocking job
    std::thread::sleep(Duration::from_millis(250));
    let _ = reactor.shutdown();
}
