//! TOML configuration: shipped defaults overlaid with user overrides.
//!
//! Layering:
//! 1. Built-in defaults (compiled-in copy of `config/default.toml`).
//! 2. `$XDG_CONFIG_HOME/bazzite-update-notifier/config.toml` if present.
//! 3. CLI flags (`--mode`) — applied last in `main.rs`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Context, Result};
use std::io;

const DEFAULT_TOML: &str = include_str!("../config/default.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Toast,
    Tray,
    Both,
}

impl Mode {
    pub fn includes_tray(self) -> bool {
        matches!(self, Mode::Tray | Mode::Both)
    }
    pub fn includes_toast(self) -> bool {
        matches!(self, Mode::Toast | Mode::Both)
    }
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "toast" => Ok(Mode::Toast),
            "tray" => Ok(Mode::Tray),
            "both" => Ok(Mode::Both),
            other => Err(format!(
                "invalid mode `{other}`; expected toast, tray, or both"
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitHubConfig {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscourseConfig {
    pub base: String,
    pub tag: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BehaviorConfig {
    /// Which URL the toast body click ("default" action) opens.
    /// `"github"` (default) or `"discourse"`.
    pub toast_default_action: String,
    /// Suppress re-toast for the same checksum after the user dismisses it.
    pub suppress_after_dismiss: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub mode: Mode,
    pub check_interval_hours: u64,
    pub initial_delay_seconds: u64,
    pub github: GitHubConfig,
    pub discourse: DiscourseConfig,
    pub behavior: BehaviorConfig,
}

impl Config {
    /// Default user config path: `$XDG_CONFIG_HOME/bazzite-update-notifier/config.toml`.
    pub fn default_user_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.join(".config"))
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
            })
            .join("bazzite-update-notifier")
            .join("config.toml")
    }

    /// Built-in defaults — used as a baseline and as the value when no
    /// user config file exists.
    pub fn defaults() -> Result<Self> {
        toml::from_str(DEFAULT_TOML).context("parsing built-in default config")
    }

    /// Load defaults, then merge in the user config file if it exists at
    /// `path`. Missing file is **not** an error; malformed file **is**.
    ///
    /// Merging is field-by-field: any key the user specifies overrides
    /// the default, and unspecified keys keep their default value.
    pub fn load(path: &Path) -> Result<Self> {
        let mut cfg = Self::defaults()?;
        let bytes = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(cfg),
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", path.display()));
            }
        };
        let user: UserOverlay =
            toml::from_str(&bytes).with_context(|| format!("parsing {}", path.display()))?;
        user.apply(&mut cfg);
        Ok(cfg)
    }
}

/// Mirror of `Config` where every field is optional, so user TOML files
/// only need to mention the keys they want to override. We can't just
/// use `#[serde(default)]` on `Config` because `mode` is an enum with
/// no obvious "absent" representation.
#[derive(Debug, Default, Deserialize)]
struct UserOverlay {
    mode: Option<Mode>,
    check_interval_hours: Option<u64>,
    initial_delay_seconds: Option<u64>,
    github: Option<GitHubOverlay>,
    discourse: Option<DiscourseOverlay>,
    behavior: Option<BehaviorOverlay>,
}

#[derive(Debug, Default, Deserialize)]
struct GitHubOverlay {
    owner: Option<String>,
    repo: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DiscourseOverlay {
    base: Option<String>,
    tag: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BehaviorOverlay {
    toast_default_action: Option<String>,
    suppress_after_dismiss: Option<bool>,
}

impl UserOverlay {
    fn apply(self, cfg: &mut Config) {
        if let Some(v) = self.mode {
            cfg.mode = v;
        }
        if let Some(v) = self.check_interval_hours {
            cfg.check_interval_hours = v;
        }
        if let Some(v) = self.initial_delay_seconds {
            cfg.initial_delay_seconds = v;
        }
        if let Some(g) = self.github {
            if let Some(v) = g.owner {
                cfg.github.owner = v;
            }
            if let Some(v) = g.repo {
                cfg.github.repo = v;
            }
        }
        if let Some(d) = self.discourse {
            if let Some(v) = d.base {
                cfg.discourse.base = v;
            }
            if let Some(v) = d.tag {
                cfg.discourse.tag = v;
            }
        }
        if let Some(b) = self.behavior {
            if let Some(v) = b.toast_default_action {
                cfg.behavior.toast_default_action = v;
            }
            if let Some(v) = b.suppress_after_dismiss {
                cfg.behavior.suppress_after_dismiss = v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let cfg = Config::defaults().expect("defaults parse");
        assert_eq!(cfg.mode, Mode::Tray);
        assert_eq!(cfg.github.owner, "ublue-os");
        assert_eq!(cfg.github.repo, "bazzite");
        assert_eq!(cfg.discourse.tag, "bazzite-news");
        assert_eq!(cfg.behavior.toast_default_action, "github");
        assert!(cfg.behavior.suppress_after_dismiss);
    }

    #[test]
    fn user_override_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
mode = "both"

[behavior]
toast_default_action = "discourse"
"#,
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.mode, Mode::Both);
        assert_eq!(cfg.behavior.toast_default_action, "discourse");
        // Untouched keys keep their defaults.
        assert_eq!(cfg.github.owner, "ublue-os");
        assert!(cfg.behavior.suppress_after_dismiss);
    }

    #[test]
    fn missing_user_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.mode, Mode::Tray);
    }

    #[test]
    fn malformed_user_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this = is not [valid").unwrap();
        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn mode_includes_helpers() {
        assert!(Mode::Tray.includes_tray() && !Mode::Tray.includes_toast());
        assert!(!Mode::Toast.includes_tray() && Mode::Toast.includes_toast());
        assert!(Mode::Both.includes_tray() && Mode::Both.includes_toast());
    }

    #[test]
    fn mode_from_str() {
        use std::str::FromStr;
        assert_eq!(Mode::from_str("tray").unwrap(), Mode::Tray);
        assert_eq!(Mode::from_str("Toast").unwrap(), Mode::Toast);
        assert_eq!(Mode::from_str("BOTH").unwrap(), Mode::Both);
        assert!(Mode::from_str("nope").is_err());
    }
}
