// Always run: minimal in-memory config, no modules.

// Link to the crate so build.rs link flags apply
use baresip_sys as _;

unsafe extern "C" {
    fn libre_init() -> ::std::os::raw::c_int;
    fn libre_close();

    fn conf_configure_buf(buf: *const u8, sz: usize) -> ::std::os::raw::c_int;
    fn conf_config() -> *mut ::std::os::raw::c_void;

    fn baresip_init(cfg: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int;
    fn baresip_close();
}

#[test]
fn init_core_with_empty_config() {
    unsafe {
        assert_eq!(libre_init(), 0, "libre_init failed");

        // Minimal config buffer: pass a single newline so conf_alloc_buf accepts it
        let buf = b"\n";
        assert_eq!(conf_configure_buf(buf.as_ptr(), buf.len()), 0, "conf_configure_buf failed");

        let cfg = conf_config();
        assert!(!cfg.is_null());
        assert_eq!(baresip_init(cfg), 0, "baresip_init failed");

        baresip_close();
        libre_close();
    }
}
