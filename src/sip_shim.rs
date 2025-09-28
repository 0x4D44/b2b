use anyhow::Result;
use std::os::raw::{c_char, c_int, c_void};

unsafe extern "C" {
    fn sip_sink_init(bind_addr: *const c_char) -> c_int;
    fn sip_sink_set_pcm_callback(cb: extern "C" fn(*const i16, usize, *mut c_void), user: *mut c_void) -> c_int;
    fn sip_sink_shutdown() -> c_int;
    fn sip_preconfigure_listen(bind_addr: *const c_char) -> c_int;
    fn sip_source_start(target: *const c_char, srate: u32, ch: u8, ptime_ms: u32) -> c_int;
    fn sip_source_push_pcm(samples: *const i16, nsamples: usize) -> c_int;
    fn sip_source_shutdown() -> c_int;
    fn brs_codecs_csv() -> *const c_char;
}

pub fn sink_init(bind_addr: &str) -> Result<()> {
    let c = std::ffi::CString::new(bind_addr).unwrap();
    let rc = unsafe { sip_sink_init(c.as_ptr()) };
    if rc != 0 { anyhow::bail!("sip_sink_init rc={}", rc); }
    Ok(())
}

pub fn sink_set_pcm_callback(cb: extern "C" fn(*const i16, usize, *mut c_void), user: *mut c_void) -> Result<()> {
    let rc = unsafe { sip_sink_set_pcm_callback(cb, user) };
    if rc != 0 { anyhow::bail!("sip_sink_set_pcm_callback rc={}", rc); }
    Ok(())
}

pub fn sink_shutdown() -> Result<()> {
    let rc = unsafe { sip_sink_shutdown() };
    if rc != 0 { anyhow::bail!("sip_sink_shutdown rc={}", rc); }
    Ok(())
}

pub fn preconfigure_listen(bind_addr: &str) -> Result<()> {
    let c = std::ffi::CString::new(bind_addr).unwrap();
    let rc = unsafe { sip_preconfigure_listen(c.as_ptr()) };
    if rc != 0 { anyhow::bail!("sip_preconfigure_listen rc={}", rc); }
    Ok(())
}

pub fn source_start(target: &str, srate: u32, ch: u8, ptime_ms: u32) -> Result<()> {
    let c = std::ffi::CString::new(target).unwrap();
    let rc = unsafe { sip_source_start(c.as_ptr(), srate, ch, ptime_ms) };
    if rc != 0 { anyhow::bail!("sip_source_start rc={}", rc); }
    Ok(())
}

pub fn source_push_pcm(samples: &[i16]) -> Result<()> {
    let rc = unsafe { sip_source_push_pcm(samples.as_ptr(), samples.len()) };
    if rc != 0 { anyhow::bail!("sip_source_push_pcm rc={}", rc); }
    Ok(())
}

pub fn source_shutdown() -> Result<()> {
    let rc = unsafe { sip_source_shutdown() };
    if rc != 0 { anyhow::bail!("sip_source_shutdown rc={}", rc); }
    Ok(())
}

pub fn codecs_csv() -> String {
    unsafe {
        let p = brs_codecs_csv();
        if p.is_null() { return String::new(); }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
