//! Option B — system tray icon via StatusNotifierItem (`ksni`).
//!
//! Per the plan we register the SNI service for the entire lifetime of
//! the daemon and toggle `Status` between `Active` (update available)
//! and `Passive` (no update). We deliberately do **not** unregister the
//! service when there's nothing to show, because:
//!
//! - KDE Plasma remembers per-icon visibility preferences keyed by SNI
//!   service id; a service that flickers in and out of existence loses
//!   its slot.
//! - GNOME's AppIndicator/KStatusNotifierItem extension already honors
//!   `Status::Passive` by hiding the icon.
//!
//! `NeedsAttention` is reserved and intentionally unused for routine
//! updates.

use chrono::{DateTime, Utc};
use ksni::{menu::StandardItem, Category, Icon, MenuItem, Status, ToolTip, Tray, TrayMethods};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{Context, Result};
use crate::icons;
use crate::resolver::ReleaseLinks;
use crate::urls;

/// Events raised by tray interactions that the main loop needs to act on.
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// User asked for an immediate recheck. Main loop should kick the
    /// check loop's `Notify`.
    RecheckRequested,
    /// User picked "Quit" from the menu.
    QuitRequested,
}

/// What the tray should display.
#[derive(Debug, Clone, Default)]
pub struct TrayPresentation {
    /// True if an update is pending.
    pub active: bool,
    /// Pending version label, when `active`.
    pub pending_version: Option<String>,
    /// True if the pending update is staged (downloaded and ready to reboot into).
    pub staged: bool,
    /// Resolved release-notes URLs, when known.
    pub links: Option<ReleaseLinks>,
    /// Wall-clock time of the last successful check.
    pub last_check_at: Option<DateTime<Utc>>,
    /// Set during a manual recheck triggered by left-click; suppresses
    /// the `Passive` description and shows "Checking for updates…".
    pub checking: bool,
}

/// SNI tray implementation. Holds presentation state and an `mpsc::Sender`
/// for events back to the main loop.
pub struct TrayState {
    presentation: TrayPresentation,
    tx: mpsc::UnboundedSender<TrayEvent>,
    icon: Icon,
}

impl TrayState {
    fn new(tx: mpsc::UnboundedSender<TrayEvent>) -> Result<Self> {
        let (w, h, data) = icons::argb32_for_ksni().context("loading tray icon")?;
        Ok(Self {
            presentation: TrayPresentation::default(),
            tx,
            icon: Icon {
                width: w,
                height: h,
                data,
            },
        })
    }
}

impl Tray for TrayState {
    fn id(&self) -> String {
        // Stable id keeps KDE's per-icon visibility preference attached
        // across daemon restarts. Must match the `.desktop` filename and
        // the toast's `desktop-entry` hint.
        "bazzite-update-notifier".into()
    }

    fn title(&self) -> String {
        "Bazzite Update Notifier".into()
    }

    fn category(&self) -> Category {
        Category::SystemServices
    }

    fn status(&self) -> Status {
        if self.presentation.active {
            Status::Active
        } else {
            Status::Passive
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        let (title, description) = if self.presentation.checking {
            (
                "Bazzite Update Notifier".to_string(),
                "Checking for updates…".to_string(),
            )
        } else if self.presentation.active {
            let v = self
                .presentation
                .pending_version
                .as_deref()
                .unwrap_or("a new build");
            let desc = if self.presentation.staged {
                format!("Version {v} waiting for reboot")
            } else {
                format!("Version {v} ready to install")
            };
            (
                "Bazzite update available".to_string(),
                desc,
            )
        } else {
            let when = match self.presentation.last_check_at {
                Some(t) => format_relative(t),
                None => "never".to_string(),
            };
            (
                "Bazzite Update Notifier".to_string(),
                format!("No updates pending. Last checked {when}."),
            )
        };
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: vec![self.icon.clone()],
            title,
            description,
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Left-click semantics depend on status:
        // - Active: open GitHub release notes (the primary "release notes"
        //   target per plan).
        // - Passive: trigger a recheck; the main loop will set `checking`
        //   true so the tooltip updates.
        if self.presentation.active {
            if let Some(url) = self
                .presentation
                .links
                .as_ref()
                .map(|l| l.github_url.clone())
            {
                urls::open(&url);
            } else {
                debug!("no resolved GitHub URL yet; ignoring activate");
            }
        } else {
            send_event(&self.tx, TrayEvent::RecheckRequested);
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        if self.presentation.active {
            // Disabled header showing the pending version + channel.
            let header = match (
                self.presentation.pending_version.as_deref(),
                self.presentation.links.as_ref().map(|l| l.channel.label()),
                self.presentation.staged,
            ) {
                (Some(v), Some(ch), true) => format!("Bazzite {v} ({ch}) — waiting for reboot"),
                (Some(v), Some(ch), false) => format!("Bazzite {v} ({ch})"),
                (Some(v), None, true) => format!("Bazzite {v} — waiting for reboot"),
                (Some(v), None, false) => format!("Bazzite {v} available"),
                (None, _, true) => "Bazzite update — waiting for reboot".to_string(),
                (None, _, false) => "Bazzite update available".to_string(),
            };
            items.push(
                StandardItem {
                    label: header,
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );

            if let Some(links) = &self.presentation.links {
                let github_url = links.github_url.clone();
                items.push(
                    StandardItem {
                        label: "Release Notes (GitHub)".into(),
                        activate: Box::new(move |_: &mut Self| {
                            urls::open(&github_url);
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
                let discourse_url = links.discourse_url.clone();
                items.push(
                    StandardItem {
                        label: "What's New (Discourse)".into(),
                        activate: Box::new(move |_: &mut Self| {
                            urls::open(&discourse_url);
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
            }

            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: "Recheck now".into(),
                    activate: Box::new(|this: &mut Self| {
                        send_event(&this.tx, TrayEvent::RecheckRequested);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "No updates pending".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                StandardItem {
                    label: "Recheck now".into(),
                    activate: Box::new(|this: &mut Self| {
                        send_event(&this.tx, TrayEvent::RecheckRequested);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    send_event(&this.tx, TrayEvent::QuitRequested);
                }),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}

fn send_event(tx: &mpsc::UnboundedSender<TrayEvent>, ev: TrayEvent) {
    if let Err(e) = tx.send(ev) {
        // The receiver has been dropped — this means the daemon is
        // shutting down. We can't do anything useful here; just log.
        warn!(?e, "tray event send failed (receiver gone)");
    }
}

/// Spawn the SNI service on the current tokio runtime. Returns a handle
/// the daemon can use to push `TrayPresentation` updates, plus the
/// `Receiver` for tray events.
///
/// We call `assume_sni_available(true)` so the daemon doesn't fail at
/// startup if it races the desktop environment coming up. ksni will
/// surface the watcher coming online via `Tray::watcher_online` if/when
/// it appears.
pub async fn spawn() -> Result<(TrayHandle, mpsc::UnboundedReceiver<TrayEvent>)> {
    let (tx, rx) = mpsc::unbounded_channel();
    let state = TrayState::new(tx)?;
    let handle = state
        .assume_sni_available(true)
        .spawn()
        .await
        .context("registering StatusNotifierItem")?;
    Ok((TrayHandle { handle }, rx))
}

/// Thin wrapper over `ksni::Handle<TrayState>` so callers don't need to
/// know the inner type. Provides the one operation we actually use:
/// pushing a `TrayPresentation`.
pub struct TrayHandle {
    handle: ksni::Handle<TrayState>,
}

impl TrayHandle {
    pub async fn set(&self, presentation: TrayPresentation) {
        self.handle
            .update(move |tray: &mut TrayState| {
                tray.presentation = presentation;
            })
            .await;
    }

    /// Set just the `checking` flag — used to toggle the transient
    /// "Checking for updates…" tooltip during a manual recheck.
    pub async fn set_checking(&self, checking: bool) {
        self.handle
            .update(move |tray: &mut TrayState| {
                tray.presentation.checking = checking;
            })
            .await;
    }
}

/// Format a wall-clock instant relative to "now" — coarsely; just enough
/// for a tooltip ("just now", "5 minutes ago", etc.).
fn format_relative(t: DateTime<Utc>) -> String {
    let now = Utc::now();
    let delta = now.signed_duration_since(t);
    let secs = delta.num_seconds();
    if secs < 30 {
        return "just now".into();
    }
    if secs < 60 * 90 {
        let mins = (secs + 30) / 60;
        return format!("{} minute{} ago", mins, plural(mins));
    }
    if secs < 60 * 60 * 24 {
        let hrs = (secs + 1800) / 3600;
        return format!("{} hour{} ago", hrs, plural(hrs));
    }
    let days = (secs + 43_200) / 86_400;
    format!("{} day{} ago", days, plural(days))
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Detect a Gamescope session — SNI is not surfaced under Gamescope's
/// compositor, so the daemon should skip tray init there. Toasts can
/// still be useful (passed through to the underlying compositor).
pub fn is_gamescope_session() -> bool {
    matches!(std::env::var("XDG_CURRENT_DESKTOP"), Ok(s)
        if s.to_ascii_lowercase().contains("gamescope"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_just_now() {
        let now = Utc::now();
        let s = format_relative(now);
        assert_eq!(s, "just now");
    }

    #[test]
    fn relative_time_minutes() {
        let t = Utc::now() - chrono::Duration::minutes(5);
        let s = format_relative(t);
        assert!(s.contains("minute"), "got: {s}");
    }

    #[test]
    fn relative_time_hours() {
        let t = Utc::now() - chrono::Duration::hours(3);
        let s = format_relative(t);
        assert!(s.contains("hour"), "got: {s}");
    }

    #[test]
    fn relative_time_days() {
        let t = Utc::now() - chrono::Duration::days(2);
        let s = format_relative(t);
        assert!(s.contains("day"), "got: {s}");
    }

    /// Pure helper used by the gamescope detector — keeps the env-var
    /// mutation out of test code (so we can run tests in parallel).
    fn is_gamescope_value(s: &str) -> bool {
        s.to_ascii_lowercase().contains("gamescope")
    }

    #[test]
    fn gamescope_detection_pure() {
        assert!(!is_gamescope_value("KDE"));
        assert!(is_gamescope_value("gamescope"));
        assert!(is_gamescope_value("KDE:gamescope"));
        assert!(is_gamescope_value("GAMESCOPE"));
    }
}
