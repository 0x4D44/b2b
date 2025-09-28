#![deny(warnings)]
//! Safe(ish) high-level Rust API over `libre` and `libbaresip`.
//!
//! This is a skeletal crate to establish workspace structure. The
//! implementation will grow to include lifecycle, reactor, events,
//! and typed wrappers around Accounts, UserAgents and Calls.

pub mod error;
pub mod reactor;
pub mod events;
pub mod ffi;

pub use reactor::{BaresipContext, Reactor};
