# baresip-sys

Raw FFI bindings and link logic for `libre` (aka `re`) and `libbaresip`.

By default this crate links against system-installed libraries discovered via
`pkg-config`. Ensure `libre.pc` and `libbaresip.pc` are available. On Debian/Ubuntu
install `libre-dev` and `libbaresip-dev`; on Arch Linux install `re` and `baresip`.

Vendoring static C sources is not enabled in the crates.io package. If you require
vendored builds, use the Git repo version and enable the `vendored` feature or
provide `RE_SRC_DIR` and `BARESIP_SRC_DIR` environment variables pointing to the
C sources before building.

This crate exposes raw C items. Prefer using the safe wrapper in the `baresip`
crate for application development.

## Platform support
- Linux only.

## License
Dual-licensed under MIT or BSD-3-Clause.
