# Bazzite Update Notifier — Implementation Plan

A small Linux user-session daemon that watches for Bazzite (rpm-ostree / OCI image) updates and surfaces them to the user as either a toast notification (Option A), a system tray icon (Option B), or both. Targets KDE Plasma and GNOME, DE-agnostic via freedesktop standards.

---

## 1. Decisions (locked in)

| Topic | Choice |
|---|---|
| Language | **Rust** |
| Update detection source | **`rpm-ostree upgrade --check`** (sole source for "is an update available") |
| Release-notes target on click | **Both GitHub releases and Discourse `bazzite-news`**, exposed as menu items (Option B) and as two notification action buttons (Option A) |
| Distribution scope | Personal use first, structured to allow a later upstream PR to `bazzite-config` |
| Modes | One codebase, one daemon, configurable: `mode = "toast" | "tray" | "both"` |

### Why Rust
- A long-running user-session daemon should be small and frugal; a Rust binary is ~2–5 MB stripped with no runtime deps beyond glibc + D-Bus libs already on every Bazzite install.
- The Linux desktop ecosystem has matured Rust crates for exactly this work: `ksni` (StatusNotifierItem), `notify-rust` (D-Bus desktop notifications with actionable buttons), `zbus` (pure-Rust D-Bus), `webbrowser`.
- Atomic distros benefit from binaries that don't drag a Python interpreter and a venv into a layered package.
- Tradeoff acknowledged: if/when you upstream this to `bazzite-config`, the Universal Blue project leans Bash + Python. Mitigation: keep all "policy" (intervals, URLs, behavior) in a TOML config so a Python rewrite is mechanical; keep packaging artifacts (systemd unit, .desktop) language-agnostic.

---

## 2. High-level architecture

```
┌──────────────────────────────────────────────────────────────┐
│  bazzite-update-notifier (long-running user service)         │
│                                                              │
│  ┌──────────────┐    ┌──────────────┐    ┌───────────────┐   │
│  │ Check loop   │───▶│ Update model │───▶│ Notifier (A)  │   │
│  │ (tokio timer)│    │  (state +    │    │ notify-rust   │   │
│  └──────────────┘    │   resolver)  │    └───────────────┘   │
│         │            │              │    ┌───────────────┐   │
│         │            │              │───▶│ Tray (B)      │   │
│         │            │              │    │ ksni          │   │
│         ▼            └──────────────┘    └───────────────┘   │
│  rpm-ostree CLI            │                                 │
│  (subprocess)              ▼                                 │
│                      reqwest → GitHub API + Discourse JSON   │
└──────────────────────────────────────────────────────────────┘
```

A single async daemon owns:
1. A periodic check loop (configurable interval).
2. A persisted state file (last-seen pending checksum, last-notified timestamp).
3. A resolver that maps the pending deployment to two URLs (GitHub release, Discourse topic).
4. Two output adapters — a notifier and a tray — driven by the mode flag.

The tray must be hosted in the same long-running process (SNI is a D-Bus service that has to stay registered). Toast-only mode could in principle be a one-shot run by a systemd timer, but for simplicity and shared code we run the same binary in all modes and just disable the tray when `mode = "toast"`.

---

## 3. Project layout

```
BazziteUpdateNotifier/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE                      (MIT or Apache-2.0; matches Universal Blue style)
├── .gitignore
├── docs/
│   ├── architecture-discussion.md   (existing)
│   └── design-notes.md              (decisions, DE quirks)
├── assets/
│   ├── icon-update-available.png    (used by tray + toast)
│   └── icon.svg                     (source)
├── config/
│   └── default.toml                 (shipped defaults)
├── packaging/
│   ├── bazzite-update-notifier.service   (systemd --user)
│   ├── bazzite-update-notifier.desktop   (XDG autostart)
│   └── install.sh                        (copies user units to ~/.config)
├── src/
│   ├── main.rs           # CLI parsing, config load, daemon bootstrap
│   ├── config.rs         # serde struct, load + merge with defaults
│   ├── state.rs          # last-seen persistence (XDG_STATE_HOME)
│   ├── checker.rs        # rpm-ostree subprocess + JSON parsing
│   ├── resolver.rs       # version → GitHub release URL + Discourse topic URL
│   ├── notifier.rs       # Option A: notify-rust with action buttons
│   ├── tray.rs           # Option B: ksni icon + menu
│   ├── icons.rs          # include_bytes!() of PNG, ARGB32 conversion for ksni
│   ├── urls.rs           # tiny helpers + webbrowser::open
│   └── error.rs          # anyhow re-export + module errors
└── tests/
    ├── checker_parse.rs  # snapshot tests against captured rpm-ostree --json output
    └── resolver_match.rs # version-tag matching logic
```

---

## 4. Update detection (`checker.rs`)

### Strategy
Run `rpm-ostree upgrade --check` to perform the network probe, then read `rpm-ostree status --json` to get structured data about both the current and pending deployments.

### Why both commands
- `upgrade --check` is the only command that reaches out to the registry to refresh the cache; without it, a `status` call may report a stale "no pending."
- `status --json` is the only stable, machine-readable surface for the deployment metadata (version label, checksum, container image reference).

### Wrapper API
```rust
pub struct CheckOutcome {
    pub update_available: bool,
    pub pending: Option<Deployment>,
    pub booted: Deployment,
}

pub struct Deployment {
    pub checksum: String,        // ostree commit
    pub version: String,         // e.g. "42.20260510.0"
    pub image_ref: Option<String>, // ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable
    pub timestamp: i64,
}

pub async fn check() -> Result<CheckOutcome>;
```

### Failure modes handled
- rpm-ostree daemon busy (lock held): retry with exponential backoff up to 3 attempts, then skip this cycle.
- Network failure during `--check`: log at warn, treat as "no new info this cycle"; do **not** clear an existing pending state.
- Malformed JSON / schema drift: log error with a captured payload sample, return `Err`.
- Non-Bazzite host (no rpm-ostree binary): fail fast at startup with a clear message.

### Privileges
`rpm-ostree upgrade --check` runs as the user; on Atomic systems it is configured to allow the check without polkit prompts. We will **not** invoke any command that mutates state.

---

## 5. State (`state.rs`)

Persisted at `$XDG_STATE_HOME/bazzite-update-notifier/state.json` (default `~/.local/state/...`).

```json
{
  "last_seen_pending_checksum": "abc123…",
  "last_seen_pending_version": "42.20260510.0",
  "last_notified_at": "2026-05-10T12:34:56Z",
  "dismissed_for_checksum": null
}
```

Behavior:
- A toast is emitted **once per new checksum**. After the user dismisses it (or just doesn't click), we set `dismissed_for_checksum` and won't re-toast for the same pending update unless they pick "Recheck" from the tray (mode=both/tray) or the checksum changes.
- The tray icon's visibility is driven by "is there a pending checksum that has not been cleared by booting into it?" — i.e. we re-show on every login until the user actually updates.

---

## 6. Release-notes resolver (`resolver.rs`)

Two independent URL resolutions; failures fall back gracefully. The resolver is **channel-aware** so it works correctly on both `:stable` and `:testing` Bazzite installs (and reasonably on `:unstable` and any future channels).

### Channel inference
The `image_ref` captured by the checker (e.g. `ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:testing`) is parsed to extract the image **tag**, which we treat as the channel name. Recognized values: `stable`, `testing`, `unstable`. Unknown tags fall through to "any channel" matching.

```rust
pub enum Channel { Stable, Testing, Unstable, Other(String) }

fn channel_from_image_ref(image_ref: &str) -> Channel { /* split on ':', take last */ }
```

### GitHub
1. `GET https://api.github.com/repos/{owner}/{repo}/releases?per_page=30` (default `ublue-os/bazzite`).
2. Filter releases by channel:
   - `Stable` → tags matching `^stable-` (or whose name contains "Stable").
   - `Testing` → tags matching `^testing-`.
   - `Unstable` → tags matching `^unstable-`.
   - `Other(_)` → no channel filter.
3. Within the filtered set, prefer a release whose `tag_name` contains the pending `version` substring (Bazzite version labels embed the build date, which is also embedded in tag names). Fall back to the most recent release in that channel if no exact substring match (this covers the common case where the release is published shortly after the image but the tag doesn't perfectly mirror the version label).
4. On match: open `release.html_url`.
5. On miss/failure: open the channel-filtered releases page when possible — `https://github.com/{owner}/{repo}/releases?q=stable` (or `testing`, `unstable`) — otherwise the plain releases index.

Implementation note: tag-naming conventions on Universal Blue have shifted over time. The channel-prefix heuristics above match the current `ublue-os/bazzite` convention as of 2026; the matchers live in one place (`resolver::tag_matchers`) so they're easy to update if upstream changes.

### Discourse (`bazzite-news`)
The `bazzite-news` tag mixes channels in a single stream (titles like *"Bazzite Stable Update — 42.20260510"* vs *"Bazzite Testing Update — …"*).

1. `GET https://universal-blue.discourse.group/tag/bazzite-news.json`.
2. Walk `topic_list.topics` (newest first) and pick the first whose `title` matches the channel:
   - `Stable` → title contains "Stable" (case-insensitive) **or** lacks any channel keyword (older posts often weren't suffixed and were stable).
   - `Testing` → title contains "Testing".
   - `Unstable` → title contains "Unstable".
   - `Other(_)` → take `topics[0]`.
3. Build URL: `https://universal-blue.discourse.group/t/{slug}/{id}`.
4. On no match within the page: fall back to the tag landing page `https://universal-blue.discourse.group/tag/bazzite-news`.

If a channel-specific tag exists upstream in the future (e.g. `bazzite-testing-news`), the config can override `discourse.tag` per channel without code changes.

### Caching & networking
Both calls use a 5-second timeout, a `User-Agent` of `bazzite-update-notifier/<version>`, and are cached for 1 hour keyed by `(channel, pending_checksum)` so re-resolves for the same update are free and a channel switch invalidates the cache cleanly.

### Resolved bundle
```rust
pub struct ReleaseLinks {
    pub channel: Channel,
    pub github_url: String,
    pub discourse_url: String,
    pub headline: Option<String>, // Discourse topic title for richer toast text
}
```

### Tests
The table-driven tests in `tests/resolver_match.rs` cover, at minimum:
- Stable image_ref → picks a `stable-*` release and a "Stable" Discourse title.
- Testing image_ref → picks a `testing-*` release and a "Testing" Discourse title, even if a newer stable release/topic exists.
- Unknown channel → falls through to most-recent-overall.
- Empty/garbled API response → falls back to the index URL.
- Network error → returns the index URL fallback, never panics.

---

## 7. Option A — Toast notifier (`notifier.rs`)

Library: `notify-rust = { version = "4", features = ["d", "images"] }` (the `d` feature pins the D-Bus backend so we never silently fall through to a Linux-on-macOS shim).

### Notification shape
- **Summary:** `Bazzite update available`
- **Body:** `Version {version} is ready to install.\n{discourse_headline_if_known}`
- **Icon:** the embedded PNG (`icons.rs`) so it works regardless of icon-theme.
- **Hints:**
  - `urgency = Normal`
  - `category = system`
  - `desktop-entry = bazzite-update-notifier` (associates the toast with the .desktop file so KDE/GNOME group it correctly)
- **Actions:**
  - `default` → opens GitHub release page (clicking the toast body)
  - `notes-github` → "Release Notes (GitHub)"
  - `notes-discourse` → "What's New (Discourse)"
  - `dismiss` → "Dismiss"

### Action handling
notify-rust's `show_async()` returns a `NotificationHandle` whose `wait_for_action()` callback runs on a background task. On any action other than `dismiss`, call `webbrowser::open(url)`. On `dismiss`, set `state.dismissed_for_checksum`.

### DE caveats
- **GNOME shell:** does not render custom action buttons inline; only the default action (clicking the body) is exposed unless GNOME's "Notification Center" expansion is used. We mitigate by setting `default` to GitHub (the user's stated preference) so a single click works everywhere, and surfacing both URLs in the tray menu when mode=both.
- **KDE Plasma:** renders all actions as buttons.
- **Gamescope session:** notifications may be hidden by the Steam overlay; document this in README and skip tray init in that session (see §8).

---

## 8. Option B — Tray icon (`tray.rs`)

Library: `ksni = "0.3"` — a pure-Rust StatusNotifierItem implementation that speaks the spec directly over zbus. SNI is the same protocol KDE uses natively and the GNOME *AppIndicator/KStatusNotifierItem Support* extension bridges. Bazzite ships the GNOME extension, so this works on both DEs out of the box.

### Visibility rule
> Register the SNI service for the entire lifetime of the daemon and toggle its `Status` property between `Passive` (no update) and `Active` (update available). Let the user's tray configuration decide whether to actually display it.

This is the SNI-native way to express "show when relevant" and lines up with how KDE Plasma's tray UI is designed:

- **KDE Plasma:** The user sees the icon in the tray configuration with three choices — *Always shown*, *Always hidden*, or *Shown when relevant* (the default). With "Shown when relevant," KDE displays the icon when our status is `Active` or `NeedsAttention` and hides it when `Passive`. A user who wants the icon up permanently can switch to *Always shown* and we will not fight them.
- **GNOME (AppIndicator/KStatusNotifierItem extension):** Has no per-icon visibility control, but it does honor the SNI `Status` property: `Passive` items are hidden, `Active`/`NeedsAttention` are shown. So the same single mechanism produces the right behavior on GNOME by default.

Implementation: register the SNI service once at startup. The daemon owns a single `TrayHandle`; on every check, set:
- `Status::Active` when `update_available == true`
- `Status::Passive` otherwise

`NeedsAttention` is reserved; we deliberately don't use it for routine updates so we don't override KDE's "demand attention" animations for something that isn't urgent. (Could revisit if a future update is flagged critical/security.)

We never unregister/re-register on state changes — that would interfere with the user's tray configuration (KDE remembers the visibility preference per SNI service id, so a service that disappears and reappears can lose its slot or get re-prompted). ksni's built-in retry still handles genuine D-Bus reconnects.

### Icon
A single 64×64 PNG rendered from the SVG. ksni accepts ARGB32 pixel buffers; we decode the PNG once at startup with the `image` crate and cache. Optional future enhancement: switch between two color variants (e.g., neutral when checking, accent when update found), but the spec says hide-when-clean covers the same UX.

### Tooltip
The tooltip text varies by status so users who pin the icon to "always shown" still get useful information:

- `Active`: title `"Bazzite update available"`, description `"Version {version} ready to install"`.
- `Passive`: title `"Bazzite Update Notifier"`, description `"No updates pending. Last checked {relative time}."`.

Both KDE and the GNOME extension render this on hover.

### Menu
The menu adapts to status so a user with the icon pinned visible always has useful actions:

When `Active` (update pending):
1. *header (disabled label)*: `Bazzite {version} available`
2. `Release Notes (GitHub)` → opens GitHub URL
3. `What's New (Discourse)` → opens Discourse URL
4. *(separator)*
5. `Recheck now` → triggers an immediate check loop iteration
6. `Quit`

When `Passive` (no update):
1. *header (disabled label)*: `No updates pending`
2. `Recheck now`
3. `Quit`

### Activate (left-click) action
- When `Active`: a single click opens **GitHub release notes** (the primary "release notes" target).
- When `Passive`: a single click triggers **Recheck now**. This is most useful for KDE users who pinned the icon to *Always shown* — clicking the visible-but-quiet icon lets them poke the daemon. While the recheck runs, briefly update the tooltip to "Checking for updates…" and restore it when the check completes.

Right-click reveals the menu on both KDE and the GNOME extension.

### Gamescope detection
At startup, read `XDG_CURRENT_DESKTOP`. If it contains `gamescope`, log a warning and skip tray init (SNI is not surfaced in Gamescope's compositor); still emit toasts if the mode includes them.

---

## 9. Mode glue & config (`config.rs`, `main.rs`)

### `config/default.toml`
```toml
mode = "tray"                  # "toast" | "tray" | "both"
check_interval_hours = 4
initial_delay_seconds = 60     # delay first check after login

[github]
owner = "ublue-os"
repo  = "bazzite"

[discourse]
base = "https://universal-blue.discourse.group"
tag  = "bazzite-news"

[behavior]
# When the toast body is clicked, which URL to open:
toast_default_action = "github"   # "github" | "discourse"
# Suppress re-toast for the same checksum after the user dismisses it:
suppress_after_dismiss = true
```

User overrides at `$XDG_CONFIG_HOME/bazzite-update-notifier/config.toml`.

### CLI surface (clap)
```
bazzite-update-notifier [--config <path>] [--mode toast|tray|both]
                        [--check-once] [--verbose]
```
- `--check-once`: run a single check, dispatch any notification, exit (handy for systemd timer experimentation and for manual testing).
- `--verbose` / `RUST_LOG=debug`: structured logs via `tracing-subscriber`.

### Main loop (pseudocode)
```
config = load();
state  = State::load();
tray   = if config.mode != toast { Some(Tray::spawn(rx_events)) } else { None };

sleep(config.initial_delay);
loop {
    match checker::check().await {
        Ok(outcome) => {
            update_tray(tray, &outcome);
            if outcome.update_available
               && outcome.pending.checksum != state.last_seen_pending_checksum {
                let links = resolver::resolve(&outcome.pending).await;
                if config.mode != tray { notifier::toast(&outcome, &links).await; }
                state.record_seen(&outcome.pending);
                state.persist();
            }
        }
        Err(e) => warn!(?e, "check failed"),
    }
    sleep(config.check_interval).await;
}
```

The tray's `Recheck now` menu item just sends a kick on a `tokio::sync::Notify` so the loop wakes immediately.

---

## 10. Packaging

### `packaging/bazzite-update-notifier.service` (systemd `--user`)
```ini
[Unit]
Description=Bazzite Update Notifier
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/bin/bazzite-update-notifier
Restart=on-failure
RestartSec=30s

[Install]
WantedBy=graphical-session.target
```

### `packaging/bazzite-update-notifier.desktop` (XDG autostart fallback)
Used for desktop sessions that don't activate user services on login. Same binary, started hidden.

### `packaging/install.sh`
- Copies the binary to `~/.local/bin/`
- Copies the `.service` to `~/.config/systemd/user/`
- Copies the `.desktop` to `~/.config/autostart/`
- Runs `systemctl --user daemon-reload && systemctl --user enable --now bazzite-update-notifier.service`

### Notes for later upstreaming
- The unit and `.desktop` are language-agnostic, so they survive a future Python rewrite untouched.
- A Bazzite-style `ujust install-update-notifier` recipe could call `install.sh` from a layered package or fetch a release artifact.
- A COPR build is out of scope for v1 but the project structure is compatible with a `.spec` file later.

---

## 11. Testing strategy

| Layer | Approach |
|---|---|
| `checker.rs` JSON parsing | Snapshot tests against captured `rpm-ostree status --json` output for both "no pending" and "pending" states. Keep fixtures under `tests/fixtures/`. |
| `resolver.rs` tag match | Table-driven tests covering channel inference from `image_ref`, channel-filtered GitHub tag matching for stable/testing/unstable, channel-filtered Discourse title matching, and all fallback paths. |
| `state.rs` | Round-trip serialization tests, plus a corruption-recovery test (truncated/garbage state file → fresh state, no panic). |
| Notifier | Manual smoke run with `--check-once` against a synthetic outcome via a `--debug-fake-update` flag (compiled out of release builds). |
| Tray | Manual on KDE Plasma 6 and GNOME Shell with the AppIndicator extension; document a quick `dbus-monitor` recipe for verifying SNI registration. |

A simple GitHub Actions workflow runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on Linux.

---

## 12. Implementation phases

Each phase ends in a runnable, testable artifact.

1. **Bootstrap (½ day)** — `cargo new`, `Cargo.toml` deps, license, README skeleton, GitHub Actions, asset placeholders.
2. **Checker + state (1 day)** — `checker.rs`, `state.rs`, fixture tests. CLI prints "update: yes/no" with `--check-once`.
3. **Resolver (½ day)** — `resolver.rs` plus `--check-once` now also prints the resolved GitHub and Discourse URLs.
4. **Option A: Notifier (½ day)** — wire `notifier.rs`; `--check-once --mode toast` emits a real toast on KDE and GNOME.
5. **Option B: Tray (1 day)** — wire `tray.rs`; running with `--mode tray` shows the icon when an update is staged.
6. **Daemon glue & config (½ day)** — main loop, TOML config, `--mode both`, dismiss/recheck logic.
7. **Packaging (½ day)** — systemd user unit, autostart `.desktop`, `install.sh`, README install instructions.
8. **Polish (½ day)** — logging, Gamescope detection, suppress-after-dismiss tuning, README screenshots, design notes doc.

Total: roughly 4–5 focused days of work for a first releasable version.

---

## 13. Open items (deferred, not blockers)

- **GHCR digest check as cross-validation.** Could be added later as a *second* signal alongside `rpm-ostree --check` to detect updates faster than the rpm-ostree refresh cadence.
- **`rpm-ostree db diff` integration.** Could enrich the toast body or add a "Show package changes" menu item that opens a small text window. Defer to v2.
- **In-app rendering of Discourse `cooked` HTML** instead of the browser. Requires a webview crate (`webkit2gtk` or similar) and is a much bigger surface area; out of scope for v1.
- ~~**Stable/Testing/Unstable channel awareness.**~~ Folded into v1 (see §6). The resolver derives the channel from `image_ref` and filters GitHub releases and Discourse topics accordingly.
- **Localization.** All strings will be in code constants for v1; if this is upstreamed, factor through `gettext` or Fluent.

---

## 14. Acceptance criteria for v1

- [ ] On a Bazzite system with a pending update, running `bazzite-update-notifier --mode toast --check-once` emits a clickable toast on both KDE Plasma and GNOME (with the AppIndicator extension installed for the tray case).
- [ ] On a Bazzite system with a pending update, running `bazzite-update-notifier --mode tray` produces a visible tray icon (under default KDE *Shown when relevant* and default GNOME AppIndicator behavior) with hover tooltip containing the version, and a left-click that opens the GitHub release page.
- [ ] On a Bazzite system with **no** pending update, the daemon's tray service is registered with `Status = Passive`, the icon is hidden by default on both KDE (via *Shown when relevant*) and GNOME (via the AppIndicator extension's status filtering), and a KDE user who explicitly sets the icon to *Always shown* sees a passive icon with a "no updates" tooltip and menu.
- [ ] State persists across restarts: a dismissed toast is not re-emitted for the same checksum until either the checksum changes or the user picks "Recheck now."
- [ ] `systemctl --user enable --now bazzite-update-notifier.service` starts the daemon and `systemctl --user status` reports it healthy.
- [ ] `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` pass in CI.
