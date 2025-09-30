# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`b2b` is a SIP/RTP audio pipeline tool built on Rust and baresip. It implements four roles—**Orchestrator**, **Source**, **Sink**, and **Mixer**—that work together to create local audio pipelines using SIP signaling and RTP media transport.

- **Source**: Decodes MP3, resamples to 8 kHz, encodes to PCMU, and sends RTP
- **Sink**: Accepts SIP INVITE, decodes PCMU, and pipes PCM to `aplay`
- **Mixer**: Dual-homed bridge that mixes incoming audio with DTMF tones
- **Orchestrator**: Parent process that spawns child roles, aggregates logs, and manages lifecycle

The system defaults to localhost loopback with PCMU codec at 8 kHz, designed for Linux with ALSA playback.

## Build Commands

```bash
# Standard debug build
cargo build

# Release build (uses vendored static libre/baresip)
cargo build --release
# or
./scripts/build_release.sh

# Run tests
cargo test

# Check code
cargo check
```

The build uses vendored `re` (libre) and `baresip` sources in `third_party/src/`. The `.cargo/config.toml` sets environment variables (`RE_SRC_DIR`, `BARESIP_SRC_DIR`, `BARESIP_MODULES`) that the `baresip-rs-sys` build script consumes.

## Running Roles

### Individual Roles (Manual)
```bash
# Sink (SIP server that plays audio)
cargo run -- --role sink --sip-bind 127.0.0.1:5062

# Mixer (bridges Source → Sink with DTMF injection)
cargo run -- --role mixer --sip-bind 127.0.0.1:5063 --target sip:127.0.0.1:5062 --dtmf-seq "123#"

# Source (SIP client that sends audio)
cargo run -- --role source --target sip:127.0.0.1:5063 --audio-file ./sample.mp3
```

### Orchestrator (Automated)
```bash
# Run a plan file (spawns all roles automatically)
cargo run -- --role orchestrator --plan ./plans/example.plan.toml

# Dry-run to see what commands will be executed
cargo run -- --role orchestrator --plan ./plans/example.plan.toml --dry-run
```

Plan files (`.toml`) define topology with per-role ports, targets, and parameters. See `plans/example.plan.toml` for reference.

## Architecture

### Module Structure

- `src/main.rs`: Entry point, role dispatch
- `src/cli.rs`: CLI argument parsing (clap-based)
- `src/orchestrator.rs`: Child process management, log aggregation, readiness coordination, graceful shutdown
- `src/roles/`: Role implementations
  - `source.rs`: MP3 decode → resample → PCMU encode → RTP send
  - `sink.rs`: RTP receive → PCMU decode → PCM → aplay stdin
  - `mixer.rs`: Dual SIP legs, mixes incoming PCM with DTMF tones
- `src/sip.rs`: Thin wrapper over `baresip-rs` (UA initialization, reactor)
- `src/media.rs`: Audio utilities (MP3 decode via symphonia, resampling, DTMF generation, mixing)
- `src/sip_shim.rs`: FFI bindings to `c/sip_shim.c`
- `src/logging.rs`: Text and JSON log formatters, color tagging
- `src/util.rs`: Subprocess helpers, signal utilities
- `c/sip_shim.c`: C integration layer for baresip audio filters and custom audio source

### Data Flow

1. **Orchestrator spawns children** in order:
   - Sink first (waits for READY)
   - Mixer next (dials Sink, waits for READY)
   - Source last (dials Mixer)

2. **Readiness Protocol**: Each child emits a `READY role=<name> ...` line on stdout when initialized. Orchestrator waits for all expected roles before proceeding.

3. **Audio Path**:
   - Source: MP3 → PCM (i16) → resample to 8 kHz mono → aubuf → C shim pulls frames → PCMU encode → RTP
   - Mixer: RTP in → PCMU decode → PCM → mix with DTMF → PCMU encode → RTP out
   - Sink: RTP → PCMU decode → PCM → aplay stdin

4. **Shutdown**: Orchestrator sends SIGTERM to children, waits `--grace-ms`, escalates to SIGKILL if needed.

### Concurrency

- Each role spawns baresip's reactor thread for SIP/RTP event loop
- Source: decode/resample runs in separate thread filling an audio buffer (`aubuf`)
- Sink: RTP callbacks enqueue PCM to a small queue flushed by writer thread to aplay
- Mixer: Separate threads for inbound PCM, DTMF generation, and mixing
- Orchestrator: Supervisor thread monitors child lifecycle; per-child reader threads for stdout/stderr

## Key External Dependencies

- **baresip-rs** / **baresip-rs-sys**: Rust bindings to baresip SIP stack (vendored in `vendor/`)
- **symphonia**: MP3 decoding
- **clap**: CLI argument parsing
- **anyhow** / **thiserror**: Error handling
- **owo-colors**: Terminal color output

Vendored sources:
- `third_party/src/re/`: libre (SIP/RTP core)
- `third_party/src/baresip/`: baresip (SIP user agent)

## Development Notes

### C Integration

The `c/sip_shim.c` provides a bridge between Rust and baresip's C API for:
- Custom audio source (`b2b_ausrc_register`) that pulls PCM from Rust-managed `aubuf`
- Audio filter tap for capturing decoded PCM in Sink
- Metrics collection for packets/intervals

Rust calls C via FFI bindings in `src/sip_shim.rs`.

### Plan Files

Plan files (TOML) define multi-role topologies. Structure:
```toml
[topology]
source.sip_target = "sip:127.0.0.1:5063"
source.audio_file = "./sample.mp3"

mixer.sip_bind = "127.0.0.1:5063"
mixer.sip_target = "sip:127.0.0.1:5062"
mixer.dtmf_seq = "123#"

sink.sip_bind = "127.0.0.1:5062"
```

Placeholders `YOUR_HOST_IP` or `YOUR_IP` are auto-replaced with detected IPv4.

### Logging

- **Text mode** (default): Human-readable with color tags per role
- **JSON mode** (`--log-format json`): Structured logs for tooling
- Orchestrator tags child output: `[SOURCE]`, `[MIXER]`, `[SINK]`

Enable SIP trace: `B2B_SIP_TRACE=1 cargo run ...`

Raw child output (no timestamp/tag): `B2B_CHILD_RAW=1` (automatically set by Orchestrator)

### Codec and Audio

- Default codec: PCMU (G.711 μ-law) at 8 kHz mono
- Ptime: 20 ms (160 samples per frame at 8 kHz)
- Resample: Linear interpolation (v1); future: `rubato` for higher quality
- DTMF: Dual-tone sine waves, configurable sequence and period

### Error Handling

Exit codes:
- 0: Success
- 64: Invalid arguments
- 65: SIP setup/connection error
- 66: Audio I/O error
- 67: Readiness/shutdown timeout
- 70: Panic

## Design Documents

For detailed design and implementation plans:
- `designs/2025.09.28 - HLD.md`: High-level design document
- `designs/2025.09.28 - Reqs.md`: Requirements specification

## Testing

Unit tests focus on:
- MP3 decode correctness
- Resampler accuracy (impulse/step response)
- DTMF tone frequency validation
- Mixer saturation/clipping behavior

Integration tests verify:
- Role startup on loopback
- READY protocol contract
- RTP timestamp monotonicity
- Audio continuity over extended runs

Run tests: `cargo test`