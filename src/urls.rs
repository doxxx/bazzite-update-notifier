//! Tiny URL-opening helpers.
//!
//! Wraps `webbrowser::open` so callers don't need to know which crate we
//! use, and so we can centralize logging for "user clicked but the browser
//! didn't open" cases.

use tracing::{info, warn};

/// Open a URL in the user's default browser. Logs success/failure rather
/// than returning an error: a failed open here should never crash the
/// daemon, and the user has clearly *tried* to read release notes so
/// silent failure would be the worst outcome.
pub fn open(url: &str) {
    info!(%url, "opening URL in browser");
    if let Err(e) = webbrowser::open(url) {
        warn!(?e, %url, "failed to open URL");
    }
}
