# Repository Guidelines

## Project Structure & Module Organization
- `src/` — main Rust binary (`src/main.rs`). Keep new modules small and focused.
- `vendor/` — patched crates used via `[patch.crates-io]` (e.g., `vendor/baresip-rs`, `vendor/baresip-rs-sys`). Don’t rename folders.
- `third_party/src/` — vendored C sources (`re`, `baresip`). Update via scripts, not manual edits.
- `scripts/` — helper scripts for vendoring/builds.
- `.cargo/config.toml` — exports `RE_SRC_DIR` and `BARESIP_SRC_DIR` for builds.
- `target/` — build artifacts (ignored by VCS).

## Build, Test, and Development Commands
- Build (debug): `cargo build`
- Run (debug): `cargo run`
- Release + static vendored libs: `scripts/build_static.sh`
- Vendor/refresh C sources: `scripts/vendor_baresip.sh`
- Format: `cargo fmt --all`
- Lint (fail on warnings): `cargo clippy --all-targets -- -D warnings`
- Tests (this crate): `cargo test` (add tests as described below)
- Vendor crate tests:
  - `cd vendor/baresip-rs && cargo test`
  - `cd vendor/baresip-rs-sys && cargo test`

## Coding Style & Naming Conventions
- Rust edition: 2024; 4‑space indentation; keep lines concise.
- Naming: modules/functions `snake_case`, types/traits `UpperCamelCase`, constants `SCREAMING_SNAKE_CASE`.
- Always run `cargo fmt` and `cargo clippy` before committing.

## Testing Guidelines
- Prefer unit tests colocated in files using `#[cfg(test)] mod tests { … }`.
- For integration tests, add `tests/` with files like `tests/reactor_smoke.rs`.
- When touching vendor crates, run their tests as shown above.
- Aim to cover new public functions and failure paths; include minimal repros for race/async cases.

## Commit & Pull Request Guidelines
- Use Conventional Commits: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`.
- PRs must include: summary, rationale, test steps (exact commands), and any logs/screenshots.
- Ensure `cargo fmt`, `cargo clippy`, and relevant tests pass locally before requesting review.

## Security & Configuration Tips
- Never commit secrets. Builds rely on environment: `RE_SRC_DIR`, `BARESIP_SRC_DIR` (pre-set via `.cargo/config.toml`).
- Don’t edit `third_party/src/*` directly; refresh via `scripts/vendor_baresip.sh` and propose changes upstream or in `vendor/*` crates.

## Architecture Overview
- Binary initializes `baresip` core and `Reactor`, emits/logs events, then shuts down cleanly. See `src/main.rs` for the minimal event loop.

