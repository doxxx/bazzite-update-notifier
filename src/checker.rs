//! `rpm-ostree` subprocess wrapper.
//!
//! Per the implementation plan we run two commands every cycle, in order:
//!
//! 1. `rpm-ostree upgrade --check` — refreshes the OCI registry cache
//!    (without this, `status` may report a stale "no pending").
//! 2. `rpm-ostree status --json` — the only stable, machine-readable
//!    surface for deployment metadata.
//!
//! The daemon is read-only: we never invoke any rpm-ostree command that
//! would mutate state.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::error::{anyhow, bail, Context, Result};

/// What the checker tells the rest of the daemon after one cycle.
///
/// The two variants make the "update available ↔ pending deployment exists"
/// invariant structural: it is impossible to construct an `UpdateAvailable`
/// without a `pending` deployment, or a `NoUpdate` with one. This removes
/// the need for runtime `.expect()` calls at every use-site.
#[derive(Debug, Clone)]
pub enum CheckOutcome {
    /// A pending deployment is ready to reboot into (or has been fetched
    /// by the rpm-ostree daemon but not yet written to a deployment slot).
    UpdateAvailable {
        pending: Deployment,
        booted: Deployment,
        /// True if the update is staged (downloaded and ready to reboot into).
        /// False if it's a cached update (metadata known but not yet downloaded).
        staged: bool,
    },
    /// No pending update; the booted deployment is current.
    NoUpdate { booted: Deployment },
}

/// The subset of rpm-ostree deployment fields we care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    /// ostree commit checksum.
    pub checksum: String,
    /// Human-readable version label e.g. `"42.20260510.0"`.
    pub version: String,
    /// `ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable` etc.
    /// `None` for non-image-pinned deployments.
    pub image_ref: Option<String>,
    /// Unix epoch seconds of the deployment.
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// Raw JSON shape produced by `rpm-ostree status --json`.
// We deserialize into private structs and project into our public types
// so schema drift in fields we don't use can't break the daemon.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawStatus {
    #[serde(default)]
    deployments: Vec<RawDeployment>,
    /// Present when rpm-ostree's daemon already knows about a remote update
    /// but it hasn't been staged (downloaded) yet. We treat this exactly like
    /// a staged deployment for "is an update available" purposes.
    #[serde(rename = "cached-update", default)]
    cached_update: Option<RawCachedUpdate>,
}

#[derive(Debug, Deserialize)]
struct RawDeployment {
    #[serde(default)]
    checksum: String,
    #[serde(default)]
    version: String,
    #[serde(rename = "container-image-reference", default)]
    image_ref: Option<String>,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    booted: bool,
    #[serde(default)]
    staged: bool,
    /// Some rpm-ostree versions emit `pending` instead of `staged` for the
    /// deployment that will be active after the next reboot. Accept both.
    #[serde(default)]
    pending: bool,
}

#[derive(Debug, Deserialize)]
struct RawCachedUpdate {
    #[serde(default)]
    checksum: String,
    #[serde(default)]
    version: String,
    #[serde(rename = "container-image-reference", default)]
    image_ref: Option<String>,
    #[serde(default)]
    timestamp: i64,
}

/// Pure parser used by tests and by `check()`. Separated from the subprocess
/// runner so the JSON-handling can be exercised against fixture files.
pub fn parse_status(json: &str) -> Result<CheckOutcome> {
    let raw: RawStatus = serde_json::from_str(json).context("parsing rpm-ostree status JSON")?;

    if raw.deployments.is_empty() {
        bail!("rpm-ostree status returned no deployments");
    }

    let booted = raw
        .deployments
        .iter()
        .find(|d| d.booted)
        .ok_or_else(|| anyhow!("no booted deployment in rpm-ostree status"))?;

    let booted = Deployment {
        checksum: booted.checksum.clone(),
        version: booted.version.clone(),
        image_ref: booted.image_ref.clone(),
        timestamp: booted.timestamp,
    };

    // First preference: an explicitly staged/pending deployment that is *not*
    // the booted one. This is the "ready to reboot into" case.
    let staged = raw
        .deployments
        .iter()
        .find(|d| (d.staged || d.pending) && !d.booted);

    if let Some(d) = staged {
        return Ok(CheckOutcome::UpdateAvailable {
            pending: Deployment {
                checksum: d.checksum.clone(),
                version: d.version.clone(),
                image_ref: d.image_ref.clone(),
                timestamp: d.timestamp,
            },
            booted,
            staged: d.staged || d.pending,
        });
    }

    // Second preference: a `cached-update` block. This means the daemon has
    // pulled metadata for a newer image but it hasn't been written to a
    // deployment slot yet. Still counts as "update available", but it's
    // not staged yet (requires download).
    if let Some(c) = raw.cached_update {
        // Sanity check: the cached update's checksum must differ from the
        // booted deployment, otherwise rpm-ostree is just echoing the
        // current commit and there's nothing new.
        if !c.checksum.is_empty() && c.checksum != booted.checksum {
            return Ok(CheckOutcome::UpdateAvailable {
                pending: Deployment {
                    checksum: c.checksum,
                    version: c.version,
                    image_ref: c.image_ref,
                    timestamp: c.timestamp,
                },
                booted,
                staged: false,
            });
        }
    }

    Ok(CheckOutcome::NoUpdate { booted })
}

/// Default location to look up rpm-ostree. Overridable via env var for tests.
fn rpm_ostree_bin() -> String {
    std::env::var("BAZZITE_RPM_OSTREE_BIN").unwrap_or_else(|_| "rpm-ostree".to_string())
}

/// Verify the rpm-ostree binary is on PATH. Called once at startup so the
/// daemon fails fast on a non-Bazzite host instead of silently looping.
pub async fn ensure_rpm_ostree_available() -> Result<()> {
    let bin = rpm_ostree_bin();
    let status = Command::new(&bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| format!("spawning `{} --version`", bin))?;
    if !status.success() {
        bail!("`{} --version` exited with status {:?}", bin, status.code());
    }
    Ok(())
}

/// Run `upgrade --check` then `status --json`, with up to 3 attempts and
/// exponential backoff if rpm-ostree's transaction lock is held.
pub async fn check() -> Result<CheckOutcome> {
    const MAX_ATTEMPTS: usize = 3;
    let mut delay = Duration::from_millis(500);

    for attempt in 1..=MAX_ATTEMPTS {
        match run_check_once().await {
            Ok(outcome) => return Ok(outcome),
            Err(e) if attempt < MAX_ATTEMPTS && is_transient_lock_error(&e) => {
                warn!(?e, attempt, "rpm-ostree busy; retrying after backoff");
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("loop exits via return on every non-retryable path")
}

/// One pass: `upgrade --check` followed by `status --json`.
async fn run_check_once() -> Result<CheckOutcome> {
    let bin = rpm_ostree_bin();

    // 1. Refresh the OCI cache. The non-zero exit code from this command
    //    can mean "no update available" on some rpm-ostree versions, so we
    //    don't treat a non-zero status here as fatal — we log stderr and
    //    continue to `status --json` which is the source of truth.
    let upgrade = Command::new(&bin)
        .args(["upgrade", "--check"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawning `{} upgrade --check`", bin))?;

    if !upgrade.status.success() {
        let stderr = String::from_utf8_lossy(&upgrade.stderr);
        // "Transaction in progress" / "is busy" / "is locked" → propagate as a
        // transient error so `check()` can back off and retry.
        if is_busy_stderr(&stderr) {
            bail!("rpm-ostree busy: {}", stderr.trim());
        }
        debug!(
            stderr = %stderr.trim(),
            code = ?upgrade.status.code(),
            "`rpm-ostree upgrade --check` returned non-zero; continuing to status",
        );
    }

    // 2. Read structured deployment data.
    let status = Command::new(&bin)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawning `{} status --json`", bin))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        if is_busy_stderr(&stderr) {
            bail!("rpm-ostree busy: {}", stderr.trim());
        }
        bail!(
            "`{} status --json` exited with code {:?}: {}",
            bin,
            status.status.code(),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(status.stdout)
        .context("rpm-ostree status --json produced non-UTF-8 output")?;
    parse_status(&stdout)
}

/// Heuristic for "rpm-ostree daemon is busy" errors so we can retry.
fn is_transient_lock_error(e: &crate::error::Error) -> bool {
    let msg = format!("{e:#}").to_lowercase();
    msg.contains("rpm-ostree busy")
        || msg.contains("transaction in progress")
        || msg.contains("is locked")
        || msg.contains("client transaction already")
}

fn is_busy_stderr(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("transaction in progress")
        || s.contains("client transaction already")
        || s.contains("is locked")
        || s.contains("could not acquire")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_no_pending() {
        let json = include_str!("../tests/fixtures/status_no_pending.json");
        let outcome = parse_status(json).expect("parse");
        let booted = match outcome {
            CheckOutcome::NoUpdate { booted } => booted,
            other => panic!("expected NoUpdate, got {other:?}"),
        };
        assert_eq!(booted.version, "42.20260510.0");
        assert_eq!(
            booted.image_ref.as_deref(),
            Some("ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable")
        );
    }

    #[test]
    fn parse_pending_staged() {
        let json = include_str!("../tests/fixtures/status_pending_staged.json");
        let outcome = parse_status(json).expect("parse");
        let (pending, booted, staged) = match outcome {
            CheckOutcome::UpdateAvailable { pending, booted, staged } => (pending, booted, staged),
            other => panic!("expected UpdateAvailable, got {other:?}"),
        };
        assert_eq!(pending.version, "42.20260512.0");
        assert!(pending.checksum.starts_with("cafebabe"));
        // Booted is still the older one.
        assert_eq!(booted.version, "42.20260510.0");
        // This is a staged deployment (ready to reboot).
        assert!(staged);
    }

    #[test]
    fn parse_pending_cached_update() {
        let json = include_str!("../tests/fixtures/status_pending_cached.json");
        let outcome = parse_status(json).expect("parse");
        let (pending, staged) = match outcome {
            CheckOutcome::UpdateAvailable { pending, staged, .. } => (pending, staged),
            other => panic!("expected UpdateAvailable, got {other:?}"),
        };
        assert_eq!(pending.version, "42.20260513.0");
        assert!(pending.checksum.starts_with("feedface"));
        // image_ref on the cached update should be preserved.
        assert!(pending.image_ref.unwrap().ends_with(":testing"));
        // Cached update is NOT staged (metadata only, not downloaded).
        assert!(!staged);
    }

    #[test]
    fn parse_no_deployments_is_error() {
        let err = parse_status(r#"{"deployments":[]}"#).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no deployments"), "got: {msg}");
    }

    #[test]
    fn parse_garbage_is_error() {
        assert!(parse_status("not json").is_err());
    }

    #[test]
    fn busy_stderr_detection() {
        assert!(is_busy_stderr("Transaction in progress: bla"));
        assert!(is_busy_stderr("client transaction already running"));
        assert!(is_busy_stderr("Could not acquire lock"));
        assert!(!is_busy_stderr("network unreachable"));
    }
}
