//! Library surface for `bazzite-update-notifier`.
//!
//! The `main.rs` binary re-uses these modules; making them available as
//! a library also lets integration tests in `tests/` exercise the
//! public APIs without going through the binary.

pub mod checker;
pub mod config;
pub mod error;
pub mod icons;
pub mod notifier;
pub mod resolver;
pub mod state;
pub mod tray;
pub mod urls;
