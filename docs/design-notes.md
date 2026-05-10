# Design notes

A running log of decisions, DE quirks, and trade-offs that aren't obvious
from the code or the implementation plan. The plan
(`/home/gordon/.var/app/com.visualstudio.code/data/kilo/plans/...`) is the
spec; this file collects the smaller, contextual things.

## Locked decisions

- **Auto-detection only** for the Bazzite channel. The image tag in
  `container-image-reference` is the sole signal; there is no
  configuration override. If upstream introduces a new channel, the
  resolver falls through to `Channel::Other` and disables filtering
  rather than misclassifying.
- **Toast body click → GitHub.** The plan calls out that GNOME shell does
  not render notification action buttons inline, so the body-click
  default action has to be the right thing for most users. We picked
  GitHub releases because the project's stated preference is to land
  there first.
- **Tray left-click is status-aware.**
  - `Active` (update pending): opens GitHub release notes.
  - `Passive` (no update): triggers an immediate recheck, with the
    tooltip switching to "Checking for updates…" while it runs.
- **`mode = "tray"` is the shipped default.** Toast is also supported,
  but a passive icon is the lowest-friction default for KDE's *Shown
  when relevant* behavior.

## SNI Status semantics

KDE's per-icon **Shown when relevant** preference shows the icon when our
`Status` is `Active` or `NeedsAttention` and hides it when `Passive`.
GNOME's AppIndicator/KStatusNotifierItem extension has no per-icon
visibility UI but applies the same `Status` filter globally.

We register the SNI service for the entire daemon lifetime and just
toggle `Status` between `Active` and `Passive`. We **never** unregister/
re-register on state changes — KDE remembers visibility preferences
keyed by SNI service id, and a service that disappears and reappears
loses its slot.

`NeedsAttention` is reserved for possible future critical/security
signaling. It is not used for routine updates because we don't want to
override KDE's "demand attention" animation for something that isn't
urgent.

## DE-specific quirks

- **GNOME shell:** does not render notification action buttons inline.
  Only the body-click default action is exposed without expanding the
  Notification Center. Mitigation: `default` action goes to the
  user-preferred URL; the tray menu (when `mode = "both"`) carries both
  URLs as separate items.
- **GNOME extension dependency:** the AppIndicator/KStatusNotifierItem
  Support extension is required for the tray on GNOME. Bazzite GNOME
  ships it by default; on stock Fedora Workstation it must be installed
  separately.
- **KDE Plasma:** renders all toast actions as buttons; per-icon tray
  visibility is configurable.
- **Gamescope:** SNI is not surfaced. We detect via
  `XDG_CURRENT_DESKTOP` containing `gamescope` and skip tray init.
  Toasts can still be useful (passed to the underlying compositor) so
  they're emitted normally if the mode includes them.

## rpm-ostree handling

- **Two commands per cycle, in order:**
  `rpm-ostree upgrade --check` (network-side cache refresh) followed by
  `rpm-ostree status --json` (structured read). Skipping the first means
  stale "no pending" results.
- **Privilege:** both commands run as the user; on Atomic systems
  `upgrade --check` is configured to allow the read without polkit
  prompts. The daemon never invokes a mutating rpm-ostree command.
- **Non-zero exit from `upgrade --check`** is *not* treated as a hard
  error — some rpm-ostree versions return non-zero to signal "no update
  available." The status command is the source of truth.
- **Daemon-busy retries:** if rpm-ostree's transaction lock is held, we
  retry up to 3 times with 0.5s/1s/2s backoff. After that we skip the
  cycle without clearing any pending state. Heuristic for "busy" is
  stderr containing `transaction in progress`, `is locked`, etc.
- **Missing rpm-ostree binary** at startup is a fail-fast condition with
  a clear error. We deliberately do not silently degrade on a non-Bazzite
  host.

## State persistence

- File: `$XDG_STATE_HOME/bazzite-update-notifier/state.json`
  (default `~/.local/state/bazzite-update-notifier/state.json`).
- **Corruption-tolerant:** any read or parse failure is logged at warn
  level and treated as "fresh state." The daemon never panics on a bad
  state file, and there's a unit test for both truncation and garbage
  contents.
- Atomic writes via `tempfile::NamedTempFile::new_in(parent)` followed
  by `persist(path)` (rename). Creating the temp file in the same
  directory as the target guarantees both are on the same filesystem,
  which is required for the rename to be atomic.

## Toast suppression rule

Implemented in `main::handle_outcome`:

| Condition                           | Toast emitted? |
|-------------------------------------|----------------|
| Brand-new pending checksum          | Yes            |
| User explicitly clicked "Recheck"   | Yes (forced)   |
| Same checksum, dismissed earlier    | No (when `behavior.suppress_after_dismiss = true`) |
| Same checksum, no prior dismiss     | No (we'd repeat ourselves)                          |

A new checksum automatically clears `dismissed_for_checksum` so a fresh
pending image always notifies even if the previous one was dismissed.

## Resolver caching

- Key: `(Channel, pending_checksum)`. TTL 1 hour.
- Both axes matter: a channel switch on the host changes the key, so
  cached "stable" links don't accidentally surface for a now-`testing`
  install. A new pending checksum likewise gets a fresh resolution.
- Cache only lives in-process; we don't try to persist it. The resolver
  is cheap enough that re-warming on restart is fine.

## HTTP details

- 5-second timeout on every request.
- `User-Agent: bazzite-update-notifier/<version>`. GitHub's API rejects
  unagented requests; setting it everywhere keeps things uniform.
- Reqwest is built with `rustls-tls` to avoid linking OpenSSL — keeps
  the binary self-contained and the dependency tree small.

## Why include `image` as a hard dep

`ksni` accepts ARGB32 buffers and `notify-rust` accepts raw RGBA, but
neither decodes PNG. The `image` crate (with only the `png` feature
enabled) does the decode once at startup and we cache the result via
`OnceCell`. This keeps the runtime cost zero per cycle while letting us
ship a single PNG asset rather than two pre-baked pixel buffers.

## License choice

Dual MIT / Apache-2.0 to match Universal Blue conventions. Easier to
upstream into `bazzite-config` later if we choose; either license is
also unobjectionable for personal use.

## Future work (deferred)

- GHCR digest probe as a second update signal alongside `rpm-ostree`.
- `rpm-ostree db diff` integration to enrich the toast / show package
  changes.
- Localization via gettext or Fluent.
- Critical/security update flag → `NeedsAttention`.
- COPR packaging.
