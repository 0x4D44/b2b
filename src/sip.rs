use anyhow::{Context, Result};
use baresip::{BaresipContext, Reactor};

pub struct UaHandle {
    pub ctx: BaresipContext,
    pub reactor: Reactor,
}

impl UaHandle {
    pub fn init() -> Result<Self> {
        let opts = baresip::reactor::Options { init_core: true, init_ua: true };
        let ctx = BaresipContext::new_with_options(opts).context("baresip init")?;
        let reactor = Reactor::start(&ctx).context("reactor start")?;
        // Optional SIP trace (enable by setting B2B_SIP_TRACE=1)
        if std::env::var("B2B_SIP_TRACE").map(|v| v == "1").unwrap_or(false) {
            unsafe { baresip::ffi::uag_enable_sip_trace(true) };
        }
        Ok(Self { ctx, reactor })
    }
}
