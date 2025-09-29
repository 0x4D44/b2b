use anyhow::Result;
use std::os::raw::{c_char, c_int, c_void};

unsafe extern "C" {
    fn sip_sink_init(bind_addr: *const c_char) -> c_int;
    fn sip_sink_set_pcm_callback(
        cb: extern "C" fn(*const i16, usize, *mut c_void),
        user: *mut c_void,
    ) -> c_int;
    fn sip_sink_shutdown() -> c_int;
    fn sip_source_start(target: *const c_char, srate: u32, ch: u8, ptime_ms: u32) -> c_int;
    fn sip_source_push_pcm(samples: *const i16, nsamples: usize) -> c_int;
    fn sip_source_backlog_ms() -> c_int;
    fn sip_source_tx_enable(enable: c_int) -> c_int;
    fn sip_mixer_init(
        bind_addr: *const c_char,
        target: *const c_char,
        srate: u32,
        ch: u8,
        ptime_ms: u32,
    ) -> c_int;
    fn sip_mixer_shutdown() -> c_int;
    fn sip_mixer_config(seq: *const c_char, period_ms: u32, gain_in: f32, gain_dtmf: f32) -> c_int;
    fn brs_codecs_csv() -> *const c_char;
}

pub fn sink_init(bind_addr: &str) -> Result<()> {
    let c = std::ffi::CString::new(bind_addr).unwrap();
    let rc = unsafe { sip_sink_init(c.as_ptr()) };
    if rc != 0 {
        anyhow::bail!("sip_sink_init rc={}", rc);
    }
    Ok(())
}

pub fn sink_set_pcm_callback(
    cb: extern "C" fn(*const i16, usize, *mut c_void),
    user: *mut c_void,
) -> Result<()> {
    let rc = unsafe { sip_sink_set_pcm_callback(cb, user) };
    if rc != 0 {
        anyhow::bail!("sip_sink_set_pcm_callback rc={}", rc);
    }
    Ok(())
}

pub fn sink_shutdown() -> Result<()> {
    let rc = unsafe { sip_sink_shutdown() };
    if rc != 0 {
        anyhow::bail!("sip_sink_shutdown rc={}", rc);
    }
    Ok(())
}

pub fn source_start(target: &str, srate: u32, ch: u8, ptime_ms: u32) -> Result<()> {
    let c = std::ffi::CString::new(target).unwrap();
    let rc = unsafe { sip_source_start(c.as_ptr(), srate, ch, ptime_ms) };
    if rc != 0 {
        anyhow::bail!("sip_source_start rc={}", rc);
    }
    Ok(())
}

pub fn source_push_pcm(samples: &[i16]) -> Result<()> {
    let rc = unsafe { sip_source_push_pcm(samples.as_ptr(), samples.len()) };
    if rc != 0 {
        anyhow::bail!("sip_source_push_pcm rc={}", rc);
    }
    Ok(())
}

pub fn source_backlog_ms() -> u32 {
    unsafe { sip_source_backlog_ms() as u32 }
}

pub fn source_tx_enable(on: bool) -> Result<()> {
    let rc = unsafe { sip_source_tx_enable(if on { 1 } else { 0 }) };
    if rc != 0 {
        anyhow::bail!("sip_source_tx_enable rc={}", rc);
    }
    Ok(())
}

pub fn codecs_csv() -> String {
    unsafe {
        let p = brs_codecs_csv();
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

pub fn mixer_init(bind_addr: &str, target: &str, srate: u32, ch: u8, ptime_ms: u32) -> Result<()> {
    let b = std::ffi::CString::new(bind_addr).unwrap();
    let t = std::ffi::CString::new(target).unwrap();
    let rc = unsafe { sip_mixer_init(b.as_ptr(), t.as_ptr(), srate, ch, ptime_ms) };
    if rc != 0 {
        anyhow::bail!("sip_mixer_init rc={}", rc);
    }
    Ok(())
}

pub fn mixer_shutdown() -> Result<()> {
    let rc = unsafe { sip_mixer_shutdown() };
    if rc != 0 {
        anyhow::bail!("sip_mixer_shutdown rc={}", rc);
    }
    Ok(())
}

pub fn mixer_config(seq: &str, period_ms: u32, gain_in: f32, gain_dtmf: f32) -> Result<()> {
    let c = std::ffi::CString::new(seq).unwrap_or_else(|_| std::ffi::CString::new("123#").unwrap());
    let rc = unsafe { sip_mixer_config(c.as_ptr(), period_ms, gain_in, gain_dtmf) };
    if rc != 0 {
        anyhow::bail!("sip_mixer_config rc={}", rc);
    }
    Ok(())
}
