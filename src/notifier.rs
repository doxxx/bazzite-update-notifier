//! Option A — desktop toast notifications via libnotify (D-Bus).
//!
//! Uses `notify-rust` with the `d` (D-Bus) backend pinned so we never
//! silently fall through to a no-op shim on a non-Linux build.
//!
//! ## Icon strategy
//!
//! `notify-rust`'s public 4.x API doesn't expose an `Image::from_rgba`
//! constructor. The two paths it does support are:
//!
//! 1. `Notification::icon("name")` — looked up via the freedesktop icon
//!    theme (works after `install.sh` places the icon under
//!    `~/.local/share/icons/hicolor/64x64/apps/`).
//! 2. `Hint::ImagePath("/abs/path.png")` — works regardless of the icon
//!    theme; we use this as a fallback by extracting the embedded PNG
//!    to `$XDG_CACHE_HOME/bazzite-update-notifier/icon.png` once.
//!
//! We always set both: the theme name takes precedence on servers that
//! resolve it, and the path-based hint guarantees something visible
//! even on a freshly-built daemon that hasn't run `install.sh` yet.
//!
//! ## DE caveats
//!
//! - **GNOME shell** does not render notification action buttons inline;
//!   only the body-click default action is exposed without expanding into
//!   the Notification Center. We compensate by routing `default` to the
//!   user-preferred URL (GitHub by default per `behavior.toast_default_action`)
//!   and by also surfacing both URLs in the tray menu when `mode = "both"`.
//! - **KDE Plasma** renders all four actions as buttons.

use std::path::PathBuf;

use notify_rust::{Hint, Notification, Timeout, Urgency};
use once_cell::sync::OnceCell;
use tracing::{debug, warn};

use crate::checker::Deployment;
use crate::error::{Context, Result};
use crate::icons;
use crate::resolver::ReleaseLinks;
use crate::urls;

/// Which URL the toast body click ("default" action) should open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    GitHub,
    Discourse,
}

impl DefaultAction {
    pub fn from_config_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "discourse" => DefaultAction::Discourse,
            // "github" or anything unknown → GitHub (the project's stated
            // preference and the more useful default per the plan).
            _ => DefaultAction::GitHub,
        }
    }
}

/// Outcome of an emitted toast — what the user did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastResult {
    /// User clicked the body / a "notes" action. We opened a URL.
    Opened,
    /// User explicitly dismissed (Dismiss button or close).
    Dismissed,
    /// Toast was emitted but we have no further information (timed out
    /// before the user did anything; some servers don't surface this).
    NoAction,
}

/// Lazily extract the embedded PNG to a cache file and return its path.
/// We do this only once per process; if the extraction fails we just
/// log and return `None` so the toast still emits without an image hint.
fn cached_icon_path() -> Option<PathBuf> {
    static CACHED: OnceCell<Option<PathBuf>> = OnceCell::new();
    CACHED
        .get_or_init(|| match extract_icon_to_cache() {
            Ok(p) => Some(p),
            Err(e) => {
                warn!(?e, "couldn't materialize toast icon to cache");
                None
            }
        })
        .clone()
}

fn extract_icon_to_cache() -> Result<PathBuf> {
    let cache = dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
        .context("no cache dir available")?
        .join("bazzite-update-notifier");
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;
    let path = cache.join("icon.png");
    // Idempotent: write only if missing or stale (size mismatch).
    let needs_write = match std::fs::metadata(&path) {
        Ok(m) => m.len() as usize != icons::ICON_PNG.len(),
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&path, icons::ICON_PNG)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(path)
}

/// Emit a toast for the given pending update and links. Blocks (async)
/// until the notification daemon reports the result, which on most
/// implementations means until the user acts or it times out.
///
/// `desktop_entry` should match the `.desktop` filename without extension
/// (`bazzite-update-notifier`) so KDE/GNOME group our notifications.
pub async fn toast(
    pending: &Deployment,
    links: &ReleaseLinks,
    default_action: DefaultAction,
    desktop_entry: &str,
) -> Result<ToastResult> {
    let summary = "Bazzite update available";
    let body = match &links.headline {
        Some(headline) => format!(
            "Version {} is ready to install.\n{}",
            pending.version, headline
        ),
        None => format!("Version {} is ready to install.", pending.version),
    };

    let icon_path = cached_icon_path();
    let github_url = links.github_url.clone();
    let discourse_url = links.discourse_url.clone();
    let desktop_entry = desktop_entry.to_string();

    debug!(channel = ?links.channel, github = %github_url, discourse = %discourse_url,
           "showing update toast");

    // notify-rust's wait_for_action API is blocking. We offload the whole
    // emit-and-wait sequence to a blocking task so we don't stall the
    // tokio runtime for slow notification daemons.
    let result = tokio::task::spawn_blocking(move || {
        let mut n = Notification::new();
        n.summary(summary)
            .body(&body)
            .appname("Bazzite Update Notifier")
            .icon(&desktop_entry)
            .urgency(Urgency::Normal)
            .timeout(Timeout::Default)
            .hint(Hint::Category("system".to_string()))
            .hint(Hint::DesktopEntry(desktop_entry.clone()));

        if let Some(p) = &icon_path {
            // ImagePath ensures the icon shows even when the theme name
            // hasn't been registered yet (i.e. before install.sh runs).
            n.hint(Hint::ImagePath(p.to_string_lossy().into_owned()));
        }

        // Action ids/labels per the plan.
        n.action("default", "Open release notes");
        n.action("notes-github", "Release Notes (GitHub)");
        n.action("notes-discourse", "What's New (Discourse)");
        n.action("dismiss", "Dismiss");

        let handle = n
            .show()
            .map_err(|e| crate::error::anyhow!("show notification: {e}"))?;

        // Dispatch the user's choice. `wait_for_action` invokes the closure
        // exactly once with the action id (or "__closed" if the
        // notification was closed without a click).
        let mut outcome = ToastResult::NoAction;
        handle.wait_for_action(|action| match action {
            "default" => {
                let url = match default_action {
                    DefaultAction::GitHub => &github_url,
                    DefaultAction::Discourse => &discourse_url,
                };
                urls::open(url);
                outcome = ToastResult::Opened;
            }
            "notes-github" => {
                urls::open(&github_url);
                outcome = ToastResult::Opened;
            }
            "notes-discourse" => {
                urls::open(&discourse_url);
                outcome = ToastResult::Opened;
            }
            "dismiss" => {
                outcome = ToastResult::Dismissed;
            }
            "__closed" => {
                // Server closed the notification without a click. Treat as
                // no-action; we'll re-toast on the next *new* checksum but
                // not on the same one (state machine handles that).
            }
            other => {
                warn!(action = other, "unknown notification action");
            }
        });
        Ok::<ToastResult, crate::error::Error>(outcome)
    })
    .await
    .context("notification task panicked or was cancelled")??;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_action_parsing() {
        assert_eq!(
            DefaultAction::from_config_str("github"),
            DefaultAction::GitHub
        );
        assert_eq!(
            DefaultAction::from_config_str("discourse"),
            DefaultAction::Discourse
        );
        assert_eq!(
            DefaultAction::from_config_str("DISCOURSE"),
            DefaultAction::Discourse
        );
        // Unknown defaults to GitHub.
        assert_eq!(
            DefaultAction::from_config_str("nope"),
            DefaultAction::GitHub
        );
        assert_eq!(DefaultAction::from_config_str(""), DefaultAction::GitHub);
    }
}
