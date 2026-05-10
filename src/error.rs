//! Crate-wide error and result types.
//!
//! We use `anyhow::Result` for application-level fallibility and rely on
//! `tracing` for structured diagnostics. Module-internal errors that need
//! to be matched on are kept as plain enums in their respective modules
//! (e.g. `checker::CheckError`).

pub use anyhow::{anyhow, bail, Context, Error, Result};
