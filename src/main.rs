//! Bazzite Update Notifier — daemon entry point.
//!
//! See `docs/architecture-discussion.md` and the implementation plan for
//! the high-level design. Module overview:
//!
//! - [`checker`]   — runs `rpm-ostree` and parses its JSON status
//! - [`state`]     — persisted last-seen / dismissed checksum
//! - [`resolver`]  — channel-aware GitHub + Discourse URL resolution
//! - [`notifier`]  — Option A: desktop toast (libnotify via D-Bus)
//! - [`tray`]      — Option B: SNI tray icon (ksni)
//! - [`config`]    — TOML config: defaults + user overlay
//! - [`icons`] / [`urls`] — small support modules

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use clap::Parser;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use bazzite_update_notifier::{
    checker::{self, CheckOutcome, Deployment},
    config::{Config, Mode},
    error::{Context, Result},
    notifier::{self, DefaultAction, ToastResult},
    resolver::{ReleaseLinks, Resolver, ResolverConfig},
    state::State,
    tray::{self, TrayEvent, TrayHandle, TrayPresentation},
};

/// `desktop-entry` hint for toasts; must match the .desktop file name.
const DESKTOP_ENTRY: &str = "bazzite-update-notifier";

#[derive(Debug, Parser)]
#[command(
    name = "bazzite-update-notifier",
    version,
    about = "Watches for Bazzite (rpm-ostree) updates and notifies via toast and/or tray icon."
)]
struct Cli {
    /// Path to a config file. Defaults to
    /// `$XDG_CONFIG_HOME/bazzite-update-notifier/config.toml`.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Override the configured mode for this run.
    #[arg(long, value_parser = parse_mode)]
    mode: Option<Mode>,

    /// Run a single check, dispatch any notification, then exit.
    #[arg(long)]
    check_once: bool,

    /// Enable debug-level logging (equivalent to `RUST_LOG=debug`).
    #[arg(long)]
    verbose: bool,

    /// Pretend an update is pending (debug builds only). Skips the real
    /// rpm-ostree check entirely so the toast and tray paths can be
    /// exercised on any machine.
    #[cfg(any(debug_assertions, feature = "debug-fake"))]
    #[arg(long)]
    debug_fake_update: bool,
}

fn parse_mode(s: &str) -> std::result::Result<Mode, String> {
    s.parse::<Mode>()
}

impl Cli {
    /// True if a fake-update run was requested. In release builds without
    /// the `debug-fake` feature, the flag doesn't exist so we hardcode `false`.
    fn fake_update(&self) -> bool {
        #[cfg(any(debug_assertions, feature = "debug-fake"))]
        {
            self.debug_fake_update
        }
        #[cfg(not(any(debug_assertions, feature = "debug-fake")))]
        {
            false
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let config_path = cli.config.clone().unwrap_or_else(Config::default_user_path);
    let mut config = Config::load(&config_path)
        .with_context(|| format!("loading config {}", config_path.display()))?;
    if let Some(m) = cli.mode {
        config.mode = m;
    }
    info!(
        mode = ?config.mode,
        interval_h = config.check_interval_hours,
        "config loaded"
    );

    // Fail fast if rpm-ostree isn't installed (non-Bazzite host).
    // Skip this in `--debug-fake-update` mode so the daemon can be
    // exercised on any Linux box.
    if !cli.fake_update() {
        checker::ensure_rpm_ostree_available()
            .await
            .context("rpm-ostree not available; this daemon requires an Atomic system")?;
    }

    let resolver = Resolver::new(ResolverConfig {
        github_owner: config.github.owner.clone(),
        github_repo: config.github.repo.clone(),
        discourse_base: config.discourse.base.clone(),
        discourse_tag: config.discourse.tag.clone(),
    })?;

    if cli.check_once {
        return run_once(&cli, &config, &resolver).await;
    }

    run_daemon(cli, config, resolver).await
}

fn init_logging(verbose: bool) {
    // Precedence: `--verbose` > `RUST_LOG` > "info".
    // `--verbose` should be a no-surprises shortcut, so it takes priority
    // over an existing `RUST_LOG` (e.g. `RUST_LOG=warn`).
    //
    // zbus::object_server logs every unhandled D-Bus method at DEBUG,
    // including `ProvideXdgActivationToken` which the desktop calls on
    // SNI items but ksni doesn't implement. That's benign noise, so we
    // keep zbus at warn even in verbose mode.
    let filter = if verbose {
        EnvFilter::new("debug,zbus=warn")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Single-shot mode: one check, dispatch, exit. No tray, no scheduling.
async fn run_once(cli: &Cli, config: &Config, resolver: &Resolver) -> Result<()> {
    let outcome = if cli.fake_update() {
        fake_outcome()
    } else {
        checker::check().await?
    };

    match &outcome {
        CheckOutcome::NoUpdate { .. } => {
            println!("update: no");
        }
        CheckOutcome::UpdateAvailable { pending, .. } => {
            println!("update: yes");
            println!("pending version: {}", pending.version);
            println!("pending checksum: {}", pending.checksum);
            if let Some(ir) = &pending.image_ref {
                println!("image_ref: {ir}");
            }
            let links = resolver.resolve(pending).await;
            println!("github:    {}", links.github_url);
            println!("discourse: {}", links.discourse_url);
            if let Some(h) = &links.headline {
                println!("headline:  {h}");
            }

            if config.mode.includes_toast() {
                let action = DefaultAction::from_config_str(&config.behavior.toast_default_action);
                match notifier::toast(pending, &links, action, DESKTOP_ENTRY).await {
                    Ok(r) => debug!(?r, "toast finished"),
                    Err(e) => warn!(?e, "toast failed"),
                }
            }
        }
    }
    Ok(())
}

/// Long-running daemon. Owns the tray (when in scope), the state file,
/// and the periodic check loop.
async fn run_daemon(cli: Cli, config: Config, resolver: Resolver) -> Result<()> {
    let state_path = State::default_path();
    let mut persisted = State::load(&state_path);

    let want_tray = config.mode.includes_tray();
    let in_gamescope = tray::is_gamescope_session();
    if want_tray && in_gamescope {
        warn!("Gamescope session detected; SNI is not surfaced here. Skipping tray init.");
    }
    let (tray_handle, mut tray_rx): (Option<TrayHandle>, _) = if want_tray && !in_gamescope {
        let (h, rx) = tray::spawn().await?;
        (Some(h), Some(rx))
    } else {
        (None, None)
    };

    // Recheck signal: tray menu / left-click pushes a `TrayEvent::Recheck`,
    // which we forward into this `Notify` so the check loop wakes up.
    // When set, the next outcome is treated as if it were brand-new
    // (clears any "dismissed_for_checksum" suppression so the user can
    // re-summon a toast for the *same* pending image).
    let recheck = Arc::new(Notify::new());
    let force_resurface = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let interval = Duration::from_secs(config.check_interval_hours * 3600);

    // SIGTERM/SIGINT handler so systemd `stop` is graceful.
    // Created before the tray-event pump so we can share the same Notify
    // with both signal sources (OS signal and tray "Quit" menu item).
    let shutdown = Arc::new(Notify::new());
    spawn_signal_handler(shutdown.clone());

    // Spawn the tray-event pump before the initial delay so that
    // "Recheck now" during the startup window correctly wakes the delay
    // select and skips straight to the first check.
    if let Some(rx) = tray_rx.take() {
        let recheck = recheck.clone();
        let force = force_resurface.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move { pump_tray_events(rx, recheck, force, shutdown).await });
    }

    // Initial sleep — gives the user session time to fully start before
    // we hit the network. Skipped in fake mode so QA cycles are fast.
    if !cli.fake_update() {
        debug!("initial delay: {}s", config.initial_delay_seconds);
        let initial = Duration::from_secs(config.initial_delay_seconds);
        tokio::select! {
            _ = tokio::time::sleep(initial) => {}
            _ = recheck.notified() => {
                debug!("recheck during initial delay; skipping to first check");
            }
            _ = shutdown.notified() => {
                return Ok(());
            }
        }
    }

    loop {
        // Mark "checking" in the tray (transient).
        if let Some(h) = &tray_handle {
            h.set_checking(true).await;
        }

        let outcome_res = if cli.fake_update() {
            Ok(fake_outcome())
        } else {
            checker::check().await
        };

        match outcome_res {
            Ok(outcome) => {
                persisted.last_check_at = Some(Utc::now());
                let force = force_resurface.swap(false, std::sync::atomic::Ordering::SeqCst);
                if force {
                    // User explicitly asked for a recheck: clear the
                    // dismiss flag so the same pending checksum can
                    // re-toast.
                    persisted.dismissed_for_checksum = None;
                }
                handle_outcome(
                    &outcome,
                    &config,
                    &resolver,
                    &mut persisted,
                    tray_handle.as_ref(),
                    force,
                )
                .await;
                if let Err(e) = persisted.save(&state_path) {
                    warn!(?e, path = %state_path.display(), "state save failed");
                }
            }
            Err(e) => {
                warn!(?e, "rpm-ostree check failed");
                if let Some(h) = &tray_handle {
                    // Don't change presentation on transient failure —
                    // just clear the "checking" flag.
                    h.set_checking(false).await;
                }
            }
        }

        // Sleep until the next interval, an explicit recheck, or shutdown.
        // Use 60-second chunks so we can refresh the tray tooltip periodically.
        let refresh_interval = Duration::from_secs(60);
        let mut remaining = interval;
        let mut shutdown_received = false;
        while remaining > Duration::ZERO && !shutdown_received {
            let chunk = remaining.min(refresh_interval);
            tokio::select! {
                _ = tokio::time::sleep(chunk) => {
                    remaining = remaining.saturating_sub(chunk);
                    // Refresh tray to update tooltip relative time.
                    if let Some(ref h) = tray_handle {
                        h.refresh().await;
                    }
                }
                _ = recheck.notified() => {
                    info!("recheck triggered");
                    break;
                }
                _ = shutdown.notified() => {
                    info!("shutdown signal received");
                    shutdown_received = true;
                }
            }
        }
        if shutdown_received {
            break;
        }
    }
    Ok(())
}

/// Drive presentation + toast emission from a fresh check outcome.
///
/// `force` is set when the user explicitly clicked "Recheck now"; it
/// causes a toast to be re-emitted even if the pending checksum hasn't
/// changed since last cycle.
async fn handle_outcome(
    outcome: &CheckOutcome,
    config: &Config,
    resolver: &Resolver,
    state: &mut State,
    tray: Option<&TrayHandle>,
    force: bool,
) {
    match outcome {
        CheckOutcome::UpdateAvailable { pending, booted: _, staged } => {
             // Resolve URLs (hits cache after the first time per-checksum).
             let links = resolver.resolve(pending).await;

             let is_new =
                 state.last_seen_pending_checksum.as_deref() != Some(pending.checksum.as_str());
             state.record_seen_pending(&pending.checksum, &pending.version);

             // Update tray presentation regardless of toast suppression.
             if let Some(h) = tray {
                 h.set(TrayPresentation {
                     active: true,
                     pending_version: Some(pending.version.clone()),
                     staged: *staged,
                     links: Some(links.clone()),
                     last_check_at: state.last_check_at,
                     checking: false,
                 })
                 .await;
             }

            // Toast rules:
            //   - Always emit on a brand-new checksum.
            //   - Re-emit on a forced recheck (user asked for it).
            //   - Otherwise, suppress if the user already dismissed this
            //     checksum and `suppress_after_dismiss` is enabled.
            let dismissed_now = state.is_dismissed(&pending.checksum);
            let suppressed = config.behavior.suppress_after_dismiss && dismissed_now;
            let should_toast = config.mode.includes_toast() && (is_new || force || !suppressed);
            if should_toast {
                emit_toast(pending, &links, config, state).await;
            } else {
                debug!(
                    is_new,
                    force, suppressed, "toast suppressed for current pending checksum"
                );
            }
        }
        CheckOutcome::NoUpdate { .. } => {
            // No pending update. Clear pending state if we previously had one
            // (the user must have rebooted into the update).
            if state.last_seen_pending_checksum.is_some() {
                debug!("pending cleared (booted into update?)");
                state.clear_pending();
            }
            if let Some(h) = tray {
                h.set(TrayPresentation {
                    active: false,
                    pending_version: None,
                    staged: false,
                    links: None,
                    last_check_at: state.last_check_at,
                    checking: false,
                })
                .await;
            }
        }
    }
}

async fn emit_toast(
    pending: &Deployment,
    links: &ReleaseLinks,
    config: &Config,
    state: &mut State,
) {
    let action = DefaultAction::from_config_str(&config.behavior.toast_default_action);
    match notifier::toast(pending, links, action, DESKTOP_ENTRY).await {
        Ok(ToastResult::Dismissed) => {
            state.mark_dismissed();
            state.last_notified_at = Some(Utc::now());
        }
        Ok(ToastResult::Opened) => {
            state.last_notified_at = Some(Utc::now());
        }
        Ok(ToastResult::NoAction) => {
            state.last_notified_at = Some(Utc::now());
        }
        Err(e) => {
            error!(?e, "toast emission failed");
        }
    }
}

/// Forward tray menu/left-click events to the main loop's `Notify`.
///
/// On a recheck request we set `force_resurface = true`; the next pass
/// through the check loop will clear `dismissed_for_checksum` so the
/// same pending update can re-toast.
///
/// On a quit request we notify `shutdown` so the main loop breaks cleanly,
/// allowing the final `persisted.save()` to run before the process exits.
async fn pump_tray_events(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<TrayEvent>,
    recheck: Arc<Notify>,
    force_resurface: Arc<std::sync::atomic::AtomicBool>,
    shutdown: Arc<Notify>,
) {
    while let Some(ev) = rx.recv().await {
        match ev {
            TrayEvent::RecheckRequested => {
                force_resurface.store(true, std::sync::atomic::Ordering::SeqCst);
                recheck.notify_one();
            }
            TrayEvent::QuitRequested => {
                info!("quit requested from tray");
                shutdown.notify_one();
            }
        }
    }
}

fn spawn_signal_handler(shutdown: Arc<Notify>) {
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("install SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        shutdown.notify_one();
    });
}

/// Synthetic outcome used by `--debug-fake-update` to exercise the
/// notification + tray pipelines without an actual pending image.
fn fake_outcome() -> CheckOutcome {
    CheckOutcome::UpdateAvailable {
        booted: Deployment {
            checksum: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            version: "42.20260510.0".to_string(),
            image_ref: Some(
                "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable".to_string(),
            ),
            timestamp: 0,
        },
        pending: Deployment {
            checksum: "1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
            version: "42.20260512.0".to_string(),
            image_ref: Some(
                "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable".to_string(),
            ),
            timestamp: 0,
        },
        staged: true,
    }
}
