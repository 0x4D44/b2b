# baresip (Rust)

Safe(ish) Rust wrapper for [`libre` (aka `re`)] and `libbaresip`. It provides a
Reactor that owns the C event loop, safe helpers to execute work on the RE
thread, and an events bridge (sync and Tokio adapters).

- Default link mode: pkg-config finds system `libre` and `libbaresip`.
- Optional vendoring: use the Git repo with `third_party/src` present or set
  `RE_SRC_DIR`/`BARESIP_SRC_DIR` for `baresip-sys` (not enabled on crates.io).

## Quick Start (sync)
```rust
use baresip::{BaresipContext, Reactor};

fn main() -> anyhow::Result<()> {
    // Initializes libre (re)
    let ctx = BaresipContext::new()?;
    let reactor = Reactor::start(&ctx)?;

    // Subscribe to events (unbounded channel)
    let (_bridge, rx) = reactor.register_events();

    // Run some work on the RE thread
    reactor.execute(|| {
        // call into C here safely from the RE thread
    })?;

    // Consume one event (if any)
    if let Ok(ev) = rx.try_recv() {
        println!("event: {:?}", ev);
    }

    reactor.shutdown()?;
    Ok(())
}
```

## Quick Start (Tokio)
```rust
use baresip::{BaresipContext, Reactor};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let ctx = BaresipContext::new()?;
    let reactor = Reactor::start(&ctx)?;

    let (_bridge, mut rx) = reactor.register_events_tokio(64);

    // Schedule async-friendly work
    reactor.execute_async(|| {
        // RE-thread work
    }).await?;

    if let Some(ev) = rx.recv().await {
        println!("event: {:?}", ev);
    }

    reactor.shutdown()?;
    Ok(())
}
```

## Features
- `tokio` (default): async adapter and `execute_async`.

## Platform notes
- Linux only (glibc/musl). Requires `libre` and `libbaresip` (install via your package manager) or use vendored builds from the workspace.

## License
Dual-licensed under MIT or BSD-3-Clause.
