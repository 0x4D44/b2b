// Always run: this test uses vendored static libs and does not need network/audio.

// Force linking with the baresip-sys crate so its build.rs link flags apply.
use baresip_sys as _;

unsafe extern "C" {
    // libre (re)
    fn libre_init() -> ::std::os::raw::c_int;
    fn libre_close();
    // basic threading API
    fn re_thread_enter();
    fn re_thread_leave();
}

#[test]
fn boots_and_shuts_down() {
    unsafe {
        // Initialize and briefly enter/leave RE thread to verify symbols link.
        assert_eq!(libre_init(), 0, "libre_init failed");
        re_thread_enter();
        re_thread_leave();
        libre_close();
    }
}
