//! Persisted daemon state.
//!
//! Stored at `$XDG_STATE_HOME/bazzite-update-notifier/state.json`
//! (default `~/.local/state/bazzite-update-notifier/state.json`). Survives
//! corruption: a truncated or unparseable file is treated as if no state
//! existed, so we never panic the daemon on a bad on-disk record.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::warn;

use crate::error::{Context, Result};

/// State that survives across daemon restarts.
///
/// The fields are intentionally `Option` so a fresh state file (just
/// after first install) round-trips cleanly without sentinel values.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    /// ostree commit checksum of the last *pending* deployment we observed.
    /// `None` if we've never seen a pending deployment.
    #[serde(default)]
    pub last_seen_pending_checksum: Option<String>,

    /// Human-readable version label that paired with `last_seen_pending_checksum`.
    #[serde(default)]
    pub last_seen_pending_version: Option<String>,

    /// Wall-clock time we last emitted a toast for that pending deployment.
    /// Used only for diagnostics today; reserved for rate-limiting if we
    /// ever decide to re-toast on a longer cadence.
    #[serde(default)]
    pub last_notified_at: Option<DateTime<Utc>>,

    /// Checksum the user actively dismissed. We won't re-toast for the
    /// same checksum once this is set; it's cleared automatically when a
    /// *new* checksum is observed.
    #[serde(default)]
    pub dismissed_for_checksum: Option<String>,

    /// Wall-clock time of the last successful check (regardless of result).
    /// Surfaced in the tray tooltip when status is `Passive`.
    #[serde(default)]
    pub last_check_at: Option<DateTime<Utc>>,
}

impl State {
    /// Default on-disk path resolved against `$XDG_STATE_HOME`.
    pub fn default_path() -> PathBuf {
        let base = dirs::state_dir()
            .or_else(|| {
                // Fallback for older `dirs` semantics or non-XDG hosts.
                dirs::home_dir().map(|h| h.join(".local").join("state"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        base.join("bazzite-update-notifier").join("state.json")
    }

    /// Load state from disk, returning `Default::default()` on any read or
    /// parse failure. We intentionally swallow errors here: a corrupt state
    /// file should never prevent the daemon from starting.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    warn!(?e, path = %path.display(),
                          "state file unreadable; starting with fresh state");
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                warn!(?e, path = %path.display(),
                      "state file I/O error; starting with fresh state");
                Self::default()
            }
        }
    }

    /// Atomically persist state via a `NamedTempFile` in the same directory
    /// as `path`, then `persist` (rename) into place. Using the same directory
    /// guarantees the rename is within one filesystem, which is required for
    /// atomicity. Any error is returned to the caller — persistence failures
    /// are worth logging loudly even though they shouldn't crash the daemon.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating state dir {}", parent.display()))?;
        let bytes = serde_json::to_vec_pretty(self).context("serializing state to JSON")?;
        let mut tmp = NamedTempFile::new_in(parent)
            .with_context(|| format!("creating temp file in {}", parent.display()))?;
        tmp.write_all(&bytes)
            .with_context(|| format!("writing temp state in {}", parent.display()))?;
        tmp.persist(path)
            .with_context(|| format!("persisting state to {}", path.display()))?;
        Ok(())
    }

    /// Record that we've observed a pending deployment with this checksum.
    /// If this is a *new* checksum, the dismiss flag is cleared so a toast
    /// can be emitted (caller's choice when to actually emit).
    pub fn record_seen_pending(&mut self, checksum: &str, version: &str) {
        let is_new = self.last_seen_pending_checksum.as_deref() != Some(checksum);
        if is_new {
            self.dismissed_for_checksum = None;
        }
        self.last_seen_pending_checksum = Some(checksum.to_owned());
        self.last_seen_pending_version = Some(version.to_owned());
    }

    /// Clear pending state — call when the booted deployment matches the
    /// last pending checksum (i.e. user rebooted into the update).
    pub fn clear_pending(&mut self) {
        self.last_seen_pending_checksum = None;
        self.last_seen_pending_version = None;
        self.dismissed_for_checksum = None;
    }

    /// Mark the current pending checksum as user-dismissed. No-op if no
    /// pending checksum is recorded.
    pub fn mark_dismissed(&mut self) {
        if let Some(c) = &self.last_seen_pending_checksum {
            self.dismissed_for_checksum = Some(c.clone());
        }
    }

    /// True if a toast should be suppressed for the *currently-pending*
    /// checksum because the user explicitly dismissed it.
    pub fn is_dismissed(&self, checksum: &str) -> bool {
        self.dismissed_for_checksum.as_deref() == Some(checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let s = State::default();
        s.save(&path).unwrap();
        let back = State::load(&path);
        assert_eq!(s, back);
    }

    #[test]
    fn round_trip_populated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = State::default();
        s.record_seen_pending("abc123", "42.20260510.0");
        s.last_notified_at = Some(Utc::now());
        s.last_check_at = Some(Utc::now());
        s.save(&path).unwrap();
        let back = State::load(&path);
        assert_eq!(
            s.last_seen_pending_checksum,
            back.last_seen_pending_checksum
        );
        assert_eq!(s.last_seen_pending_version, back.last_seen_pending_version);
        // chrono round-trips to RFC3339 with same instant; compare via
        // formatted equivalence to avoid sub-nanosecond drift on some systems.
        assert_eq!(
            s.last_notified_at.map(|t| t.to_rfc3339()),
            back.last_notified_at.map(|t| t.to_rfc3339())
        );
    }

    #[test]
    fn corrupt_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"not json {{{").unwrap();
        let back = State::load(&path);
        assert_eq!(back, State::default());
    }

    #[test]
    fn truncated_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // Looks like the start of a JSON object but is incomplete.
        std::fs::write(&path, b"{\"last_seen_pending_checksum\":").unwrap();
        let back = State::load(&path);
        assert_eq!(back, State::default());
    }

    #[test]
    fn missing_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let back = State::load(&path);
        assert_eq!(back, State::default());
    }

    #[test]
    fn record_seen_clears_dismiss_on_new_checksum() {
        let mut s = State::default();
        s.record_seen_pending("aaa", "v1");
        s.mark_dismissed();
        assert!(s.is_dismissed("aaa"));
        s.record_seen_pending("bbb", "v2");
        assert!(!s.is_dismissed("bbb"));
        assert!(!s.is_dismissed("aaa"));
    }

    #[test]
    fn record_seen_preserves_dismiss_on_same_checksum() {
        let mut s = State::default();
        s.record_seen_pending("aaa", "v1");
        s.mark_dismissed();
        s.record_seen_pending("aaa", "v1");
        assert!(s.is_dismissed("aaa"));
    }

    #[test]
    fn clear_pending_resets_all() {
        let mut s = State::default();
        s.record_seen_pending("aaa", "v1");
        s.mark_dismissed();
        s.clear_pending();
        assert!(s.last_seen_pending_checksum.is_none());
        assert!(s.last_seen_pending_version.is_none());
        assert!(s.dismissed_for_checksum.is_none());
    }
}
