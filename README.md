# Bazzite Update Notifier

A small Linux user-session daemon that watches for Bazzite (rpm-ostree / OCI
image) updates and surfaces them as either a desktop toast notification, a
system tray icon, or both. Targets KDE Plasma and GNOME, DE-agnostic via
freedesktop standards (libnotify + StatusNotifierItem).

> Status: pre-1.0. Built for personal use first; structured to allow a later
> upstream PR to `bazzite-config`.

## Modes

`mode` is set in `config.toml` (default: `tray`):

- `toast` — emit a desktop notification when a new update is detected; no tray.
- `tray` — show a tray icon (Active when an update is staged, Passive otherwise).
- `both` — both of the above.

## Installing From Release

To install from a release:

1. Download the latest from the Release page (e.g
   `bazzite-update-notifier-v0.2.1.tar.gz`).
2. Extract the files into a directory. From the console you can use a command
   like this:
   ```sh
   tar -zxf bazzite-update-notifier-v0.2.1.tar.gz
   ```
   This should create a `bazzite-update-notifier-v0.2.1` subfolder (or whatever
   version you downloaded).
3. In the console, go to the extracted (e.g. `bazzite-update-notifier-v0.2.1`)
   folder and execute the following command:
   ```sh
   ./install.sh
   ```

The installer copies the binary to `~/.local/bin`, the systemd unit to
`~/.config/systemd/user/`, the autostart `.desktop` to `~/.config/autostart/`,
and the icon file to `~/.local/share/icons/hicolor/64x64/apps`, then enables
and starts the service.

To uninstall:

1. In the console, go back to the folder you installed from, re-extracting the
   original release archive if necessary.
2. Run the following command:
   ```sh
   ./uninstall.sh
   ```

That will remove all the installed files and the systemd service.

## Installing From Source

```sh
mise build
./packaging/install.sh
```

To uninstall:

```sh
./packaging/uninstall.sh
```

## CLI

```
bazzite-update-notifier [--config <path>] [--mode toast|tray|both]
                        [--check-once] [--verbose]
```

- `--check-once` — run a single check and dispatch any notification, then exit.
- `--verbose` — equivalent to `RUST_LOG=debug`.
- `--debug-fake-update` — only available in debug builds (or with the
  `debug-fake` cargo feature). Pretends an update is pending so the toast
  and tray paths can be exercised on any machine.

## Configuration

User overrides live at `$XDG_CONFIG_HOME/bazzite-update-notifier/config.toml`.
See `config/default.toml` in this repository for all available keys.

## Desktop environment notes

- **KDE Plasma 6**: works out of the box. The tray icon's visibility follows
  KDE's per-icon *Always shown / Always hidden / Shown when relevant* setting.
  By default with *Shown when relevant*, the icon appears only when an update
  is pending.
- **GNOME**: requires the *AppIndicator and KStatusNotifierItem Support*
  extension (shipped on Bazzite GNOME by default). Honors SNI `Status` so
  Passive icons are hidden globally.
- **Gamescope**: SNI is not surfaced under Gamescope; the daemon detects this
  via `XDG_CURRENT_DESKTOP` and skips tray init while still emitting toasts
  if `mode` includes them.

## Development

```sh
mise run check
```

CI runs the same checks on every push.

## License

Dual-licensed under either of MIT or Apache-2.0, at your option. See
`LICENSE-MIT` and `LICENSE-APACHE`.
