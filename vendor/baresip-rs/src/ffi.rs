use std::os::raw::{c_int, c_void};

#[allow(non_camel_case_types)]
pub enum mqueue {}

pub type ReSignalH = Option<unsafe extern "C" fn(sig: c_int)>;
pub type MqueueH = Option<unsafe extern "C" fn(id: c_int, data: *mut c_void, arg: *mut c_void)>;
pub type BeventH = Option<unsafe extern "C" fn(ev: c_int, event: *mut c_void, arg: *mut c_void)>;

// Force static linkage when used together with `baresip-rs-sys` built
// with `static-link` feature.
#[link(name = "baresip", kind = "static")] // link via baresip-rs-sys
#[link(name = "re", kind = "static")]      // ensure libre symbols resolve
unsafe extern "C" {
    // libre core
    pub fn libre_init() -> c_int;
    pub fn libre_close();
    pub fn re_main(signalh: ReSignalH) -> c_int;
    pub fn re_cancel();
    pub fn re_thread_enter();
    pub fn re_thread_leave();

    // mqueue
    pub fn mqueue_alloc(mqp: *mut *mut mqueue, h: MqueueH, arg: *mut c_void) -> c_int;
    pub fn mqueue_push(mq: *mut mqueue, id: c_int, data: *mut c_void) -> c_int;

    // baresip core + configuration
    pub fn conf_config() -> *mut c_void;
    pub fn conf_configure_buf(buf: *const u8, sz: usize) -> c_int;
    pub fn conf_modules() -> c_int;
    pub fn baresip_init(cfg: *mut c_void) -> c_int;
    pub fn baresip_close();
    pub fn ua_init(software: *const i8, udp: bool, tcp: bool, tls: bool) -> c_int;
    pub fn ua_close();

    // events api
    pub fn bevent_register(eh: BeventH, arg: *mut c_void) -> c_int;
    pub fn bevent_unregister(eh: BeventH);
    pub fn bevent_get_value(event: *const c_void) -> c_int;
    pub fn bevent_get_text(event: *const c_void) -> *const i8;
    pub fn bevent_str(ev: c_int) -> *const i8;
    pub fn module_event(module: *const i8, event: *const i8,
                        ua: *mut c_void, call: *mut c_void,
                        fmt: *const i8, ...) -> c_int;

    // UA alloc/destroy
    pub fn ua_alloc(uap: *mut *mut c_void, aor: *const i8) -> c_int;
    pub fn ua_destroy(ua: *mut c_void) -> u32;

    // SIP trace
    pub fn uag_enable_sip_trace(enable: bool);
}
