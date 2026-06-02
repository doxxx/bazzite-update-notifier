//! Snapshot-style tests for `checker::parse_status` against captured
//! `rpm-ostree status --json` payloads. The fixtures live alongside in
//! `tests/fixtures/`.
//!
//! These complement (rather than duplicate) the unit tests inside
//! `src/checker.rs`: those exercise the parser via `include_str!`, this
//! file exercises it via the public library surface so we know the API
//! shape is also stable for downstream consumers.

use bazzite_update_notifier::checker::{parse_status, CheckOutcome};

#[test]
fn no_pending_fixture() {
    let json = include_str!("fixtures/status_no_pending.json");
    let outcome = parse_status(json).expect("parse");
    let booted = match outcome {
        CheckOutcome::NoUpdate { booted } => booted,
        other => panic!("expected NoUpdate, got {other:?}"),
    };
    assert_eq!(booted.version, "42.20260510.0");
}

#[test]
fn pending_staged_fixture() {
    let json = include_str!("fixtures/status_pending_staged.json");
    let outcome = parse_status(json).expect("parse");
    let (pending, booted, staged) = match outcome {
        CheckOutcome::UpdateAvailable {
            pending,
            booted,
            staged,
        } => (pending, booted, staged),
        other => panic!("expected UpdateAvailable, got {other:?}"),
    };
    assert_eq!(pending.version, "42.20260512.0");
    // booted is the older one.
    assert_eq!(booted.version, "42.20260510.0");
    // This is a staged deployment.
    assert!(staged);
}

#[test]
fn pending_cached_update_fixture() {
    let json = include_str!("fixtures/status_pending_cached.json");
    let outcome = parse_status(json).expect("parse");
    let (pending, staged) = match outcome {
        CheckOutcome::UpdateAvailable {
            pending, staged, ..
        } => (pending, staged),
        other => panic!("expected UpdateAvailable, got {other:?}"),
    };
    assert_eq!(pending.version, "42.20260513.0");
    assert!(pending.image_ref.unwrap().ends_with(":testing"));
    // Cached update is NOT staged (metadata only, not downloaded).
    assert!(!staged);
}

#[test]
fn empty_deployments_errors() {
    assert!(parse_status(r#"{"deployments":[]}"#).is_err());
}

#[test]
fn garbage_errors() {
    assert!(parse_status("not json at all").is_err());
}
