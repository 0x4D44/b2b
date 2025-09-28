#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

//! Low-level FFI bindings to `libre` (aka `re`) and `libbaresip`.
//!
//! Safety: All items in this crate are raw C FFI. Prefer using the safe
//! wrappers in the top-level `baresip` crate for application development.
//!
//! Binding Generation:
//! - By default, this crate expects pre-generated bindings at
//!   `src/bindings.rs` (currently a placeholder). Enable the `bindgen`
//!   feature to generate bindings at build time. That requires `libclang`.
//!
//! Linking Strategy:
//! - Default features enable `vendored` and `static-link`, causing build.rs to
//!   build static `libre` and `libbaresip` via CMake and link them.

#[cfg(feature = "bindgen")]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(not(feature = "bindgen"))]
mod bindings {
    // Placeholder module so the crate compiles without libclang.
    // Replace by enabling the `bindgen` feature or by adding a
    // generated `src/bindings.rs` file in the repository.
}

#[cfg(not(feature = "bindgen"))]
#[allow(unused_imports)]
pub use bindings::*;
