//! Map a pending Bazzite deployment to two release-notes URLs:
//! one on GitHub Releases, one on Universal Blue's Discourse.
//!
//! ## Channel awareness
//!
//! The Bazzite OCI image tag in `container-image-reference` is the channel
//! signal — `:stable`, `:testing`, `:unstable`, or anything else. We filter
//! both API responses by channel so a `:testing` host doesn't get pointed
//! at a stable release just because it happens to be the most recent one.
//!
//! ## Caching
//!
//! Results are cached for 1 hour keyed by `(channel, pending_checksum)`.
//! A channel switch invalidates cleanly because the channel is part of
//! the key; ditto a checksum change.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::checker::Deployment;
use crate::error::{bail, Result};

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const USER_AGENT: &str = concat!("bazzite-update-notifier/", env!("CARGO_PKG_VERSION"));

/// Channel inferred from the Bazzite image tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Channel {
    Stable,
    Testing,
    Unstable,
    /// Anything else upstream might add. The string is the raw image tag.
    Other(String),
}

impl Channel {
    /// Lowercase tag string for use as a query parameter on the GitHub
    /// releases page fallback.
    pub fn as_query(&self) -> &str {
        match self {
            Channel::Stable => "stable",
            Channel::Testing => "testing",
            Channel::Unstable => "unstable",
            Channel::Other(s) => s.as_str(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Channel::Stable => "stable",
            Channel::Testing => "testing",
            Channel::Unstable => "unstable",
            Channel::Other(s) => s.as_str(),
        }
    }
}

/// Parse the Bazzite image tag from an `image_ref` string.
///
/// `ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:testing` →
/// `Channel::Testing`. Anything we can't parse becomes `Other("")` which
/// disables channel filtering downstream.
pub fn channel_from_image_ref(image_ref: &str) -> Channel {
    // The transport prefix and the registry path each contain colons, so
    // splitting on `:` and taking the last segment is the safe move.
    let last = image_ref.rsplit(':').next().unwrap_or("").trim();
    match last.to_ascii_lowercase().as_str() {
        "stable" => Channel::Stable,
        "testing" => Channel::Testing,
        "unstable" => Channel::Unstable,
        "" => Channel::Other(String::new()),
        other => Channel::Other(other.to_string()),
    }
}

/// Fully resolved release-notes bundle for a single pending deployment.
#[derive(Debug, Clone)]
pub struct ReleaseLinks {
    pub channel: Channel,
    pub github_url: String,
    pub discourse_url: String,
    /// Discourse topic title, when one was successfully matched. Used to
    /// enrich the toast body.
    pub headline: Option<String>,
}

// ---------------------------------------------------------------------------
// Tag/title matchers — kept in one module so they're easy to update if
// upstream changes their conventions.
// ---------------------------------------------------------------------------

pub mod tag_matchers {
    //! Pure-string heuristics for matching channel-prefixed GitHub release
    //! tags and channel-keyword Discourse topic titles.
    //!
    //! These conventions have shifted historically. As of 2026:
    //!
    //! - GitHub: stable releases have no prefix (e.g., `44.20260511`) with
    //!   `prerelease=false`; testing releases prefix with `testing-` and have
    //!   `prerelease=true`; unstable releases prefix with `unstable-`.
    //! - Discourse: a single mixed `bazzite-news` stream with titles such as
    //!   *"Bazzite Stable Update — 42.20260510"* or *"Bazzite Testing
    //!   Update — …"*. Older posts predate the suffix and were stable.

    use super::Channel;

    pub fn release_tag_matches_channel(tag: &str, channel: &Channel) -> bool {
        let lower = tag.to_ascii_lowercase();
        match channel {
            Channel::Stable => {
                // Stable releases either have no channel prefix AND are not prereleases,
                // or start with "stable-". The prerelease field distinguishes stable
                // releases like "44.20260511" from testing/unstable.
                lower.starts_with("stable-")
                    || (!lower.contains("testing") && !lower.contains("unstable"))
            }
            Channel::Testing => lower.starts_with("testing-") || lower.contains("testing"),
            Channel::Unstable => lower.starts_with("unstable-") || lower.contains("unstable"),
            Channel::Other(_) => true, // no filter
        }
    }

    pub fn discourse_title_matches_channel(title: &str, channel: &Channel) -> bool {
        let lower = title.to_ascii_lowercase();
        match channel {
            // Stable: title says "stable" OR contains no channel keyword
            // (older posts often weren't suffixed and were stable).
            Channel::Stable => {
                lower.contains("stable")
                    || (!lower.contains("testing") && !lower.contains("unstable"))
            }
            Channel::Testing => lower.contains("testing"),
            Channel::Unstable => lower.contains("unstable"),
            Channel::Other(_) => true, // no filter
        }
    }
}

// ---------------------------------------------------------------------------
// GitHub releases — JSON shape (only the fields we use).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

/// Pure selection from a list of releases — testable without HTTP.
pub fn pick_github_release<'a>(
    releases: &'a [GitHubRelease],
    channel: &Channel,
    pending_version: &str,
) -> Option<&'a GitHubRelease> {
    let candidates: Vec<&GitHubRelease> = releases
        .iter()
        .filter(|r| !r.draft)
        .filter(|r| tag_matchers::release_tag_matches_channel(&r.tag_name, channel))
        .collect();

    // Prefer an exact substring match against the version label. Bazzite
    // version labels embed the build date (e.g. `42.20260512.0`); the GitHub
    // tag is `stable-20260512`. We match on the date component plus on the
    // raw label for robustness.
    let date_token = pending_version.split('.').nth(1).unwrap_or("");

    if !date_token.is_empty() {
        if let Some(r) = candidates.iter().find(|r| r.tag_name.contains(date_token)) {
            return Some(*r);
        }
    }
    if !pending_version.is_empty() {
        if let Some(r) = candidates
            .iter()
            .find(|r| r.tag_name.contains(pending_version))
        {
            return Some(*r);
        }
    }
    // Fall back to the first (which on the GitHub API is the most recent
    // for `releases?per_page=N` ordering).
    candidates.first().copied()
}

// ---------------------------------------------------------------------------
// Discourse — JSON shape (only the fields we use).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct DiscourseTagPage {
    pub topic_list: DiscourseTopicList,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DiscourseTopicList {
    pub topics: Vec<DiscourseTopic>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DiscourseTopic {
    pub id: u64,
    pub title: String,
    pub slug: String,
}

pub fn pick_discourse_topic<'a>(
    topics: &'a [DiscourseTopic],
    channel: &Channel,
) -> Option<&'a DiscourseTopic> {
    topics
        .iter()
        .find(|t| tag_matchers::discourse_title_matches_channel(&t.title, channel))
        .or_else(|| topics.first())
}

pub fn build_discourse_url(base: &str, topic: &DiscourseTopic) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{}/t/{}/{}", trimmed, topic.slug, topic.id)
}

// ---------------------------------------------------------------------------
// Resolver — the live, network-using entry point with caching.
// ---------------------------------------------------------------------------

/// Configurable knobs the resolver pulls from the daemon's config.
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    pub github_owner: String,
    pub github_repo: String,
    pub discourse_base: String,
    pub discourse_tag: String,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    inserted: Instant,
    links: ReleaseLinks,
}

#[derive(Clone)]
pub struct Resolver {
    config: ResolverConfig,
    http: reqwest::Client,
    cache: Arc<Mutex<HashMap<(Channel, String), CacheEntry>>>,
}

/// Validate a GitHub owner or repository name.
///
/// GitHub's naming rules allow alphanumerics, hyphens, underscores, and
/// dots. Anything else (especially `/`, `?`, `#`) would break the URL path
/// segments we interpolate these values into.
fn validate_github_slug(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        bail!("GitHub {field} must not be empty");
    }
    if let Some(bad) = value
        .chars()
        .find(|c| !matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.'))
    {
        bail!(
            "GitHub {field} {:?} contains invalid character {:?}",
            value,
            bad
        );
    }
    Ok(())
}

impl Resolver {
    pub fn new(config: ResolverConfig) -> Result<Self> {
        validate_github_slug(&config.github_owner, "owner")?;
        validate_github_slug(&config.github_repo, "repo")?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(HTTP_TIMEOUT)
            .build()?;
        Ok(Self {
            config,
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Resolve release-notes URLs for a pending deployment. Always returns
    /// a `ReleaseLinks` value — network failures degrade to fallback URLs
    /// rather than propagating errors, since "open *something* useful" is
    /// always better than refusing to open anything.
    pub async fn resolve(&self, pending: &Deployment) -> ReleaseLinks {
        let channel = pending
            .image_ref
            .as_deref()
            .map(channel_from_image_ref)
            .unwrap_or(Channel::Other(String::new()));

        let cache_key = (channel.clone(), pending.checksum.clone());

        // Cache hit?
        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(&cache_key) {
                if entry.inserted.elapsed() < CACHE_TTL {
                    debug!(channel = ?channel, "resolver cache hit");
                    return entry.links.clone();
                }
            }
        }

        info!(channel = ?channel, version = %pending.version, "resolving release URLs");
        let github_url = self.resolve_github(&channel, &pending.version).await;
        let (discourse_url, headline) = self.resolve_discourse(&channel).await;

        let links = ReleaseLinks {
            channel: channel.clone(),
            github_url,
            discourse_url,
            headline,
        };

        let mut cache = self.cache.lock().await;
        cache.insert(
            cache_key,
            CacheEntry {
                inserted: Instant::now(),
                links: links.clone(),
            },
        );
        links
    }

    async fn resolve_github(&self, channel: &Channel, version: &str) -> String {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases?per_page=30",
            self.config.github_owner, self.config.github_repo
        );
        info!(
            "fetching GitHub releases for channel {:?} (version {})",
            channel, version
        );
        match self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<Vec<GitHubRelease>>().await {
                    Ok(releases) => match pick_github_release(&releases, channel, version) {
                        Some(r) => {
                            info!(
                                "matched GitHub release: tag={}, url={}",
                                r.tag_name, r.html_url
                            );
                            r.html_url.clone()
                        }
                        None => {
                            warn!("no GitHub release matched for channel {:?}", channel);
                            self.github_fallback_url(channel)
                        }
                    },
                    Err(e) => {
                        warn!(?e, "GitHub releases JSON parse failed");
                        self.github_fallback_url(channel)
                    }
                }
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "GitHub releases API non-success");
                self.github_fallback_url(channel)
            }
            Err(e) => {
                warn!(?e, "GitHub releases API request failed");
                self.github_fallback_url(channel)
            }
        }
    }

    fn github_fallback_url(&self, channel: &Channel) -> String {
        // "Releases" page, optionally filtered by channel keyword.
        match channel {
            Channel::Other(s) if s.is_empty() => format!(
                "https://github.com/{}/{}/releases",
                self.config.github_owner, self.config.github_repo
            ),
            _ => format!(
                "https://github.com/{}/{}/releases?q={}",
                self.config.github_owner,
                self.config.github_repo,
                channel.as_query()
            ),
        }
    }

    async fn resolve_discourse(&self, channel: &Channel) -> (String, Option<String>) {
        let base = self.config.discourse_base.trim_end_matches('/');
        let tag = &self.config.discourse_tag;
        let url = format!("{}/tag/{}.json", base, tag);
        info!("fetching Discourse tag page for channel {:?}", channel);
        match self.http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<DiscourseTagPage>().await {
                Ok(page) => match pick_discourse_topic(&page.topic_list.topics, channel) {
                    Some(t) => {
                        let url = build_discourse_url(base, t);
                        info!(
                            topic_id = t.id,
                            "matched Discourse topic: title='{}'", t.title
                        );
                        (url, Some(t.title.clone()))
                    }
                    None => {
                        warn!("no Discourse topic matched for channel {:?}", channel);
                        (self.discourse_fallback_url(), None)
                    }
                },
                Err(e) => {
                    warn!(?e, "Discourse JSON parse failed");
                    (self.discourse_fallback_url(), None)
                }
            },
            Ok(resp) => {
                warn!(status = %resp.status(), "Discourse non-success");
                (self.discourse_fallback_url(), None)
            }
            Err(e) => {
                warn!(?e, "Discourse request failed");
                (self.discourse_fallback_url(), None)
            }
        }
    }

    fn discourse_fallback_url(&self) -> String {
        format!(
            "{}/tag/{}",
            self.config.discourse_base.trim_end_matches('/'),
            self.config.discourse_tag
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_from_stable_ref() {
        let c =
            channel_from_image_ref("ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable");
        assert_eq!(c, Channel::Stable);
    }

    #[test]
    fn channel_from_testing_ref() {
        let c =
            channel_from_image_ref("ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:testing");
        assert_eq!(c, Channel::Testing);
    }

    #[test]
    fn channel_from_unstable_ref() {
        let c = channel_from_image_ref(
            "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:unstable",
        );
        assert_eq!(c, Channel::Unstable);
    }

    #[test]
    fn channel_from_unknown_ref() {
        let c = channel_from_image_ref(
            "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:weird-channel",
        );
        assert_eq!(c, Channel::Other("weird-channel".to_string()));
    }

    #[test]
    fn channel_from_empty_ref() {
        let c = channel_from_image_ref("");
        // Empty input → trailing segment is empty → Other("").
        assert_eq!(c, Channel::Other(String::new()));
    }

    fn load_releases() -> Vec<GitHubRelease> {
        let json = include_str!("../tests/fixtures/github_releases_mixed.json");
        serde_json::from_str(json).unwrap()
    }

    fn load_topics() -> Vec<DiscourseTopic> {
        let json = include_str!("../tests/fixtures/discourse_bazzite_news.json");
        let page: DiscourseTagPage = serde_json::from_str(json).unwrap();
        page.topic_list.topics
    }

    #[test]
    fn github_picks_stable_for_stable_channel() {
        let releases = load_releases();
        let r =
            pick_github_release(&releases, &Channel::Stable, "42.20260511.0").expect("a release");
        assert_eq!(r.tag_name, "44.20260511");
    }

    #[test]
    fn github_picks_testing_for_testing_channel_even_when_newer_unstable_exists() {
        // An unstable release sits in the list; we must not pick it for a testing host.
        let releases = load_releases();
        let r =
            pick_github_release(&releases, &Channel::Testing, "42.20260510.0").expect("a release");
        assert_eq!(r.tag_name, "testing-44.20260510");
    }

    #[test]
    fn github_falls_back_to_most_recent_in_channel_when_version_misses() {
        // No release matches version 42.20260601.0, but the channel filter
        // should still narrow to stable releases and yield the newest one.
        let releases = load_releases();
        let r =
            pick_github_release(&releases, &Channel::Stable, "42.20260601.0").expect("a release");
        assert_eq!(r.tag_name, "44.20260511");
    }

    #[test]
    fn github_other_channel_picks_most_recent() {
        let releases = load_releases();
        let r = pick_github_release(&releases, &Channel::Other("nightly".to_string()), "x")
            .expect("a release");
        // First in the list is the newest; with no channel filter we take it.
        assert_eq!(r.tag_name, "44.20260511");
    }

    #[test]
    fn github_empty_release_list_yields_none() {
        let r = pick_github_release(&[], &Channel::Stable, "x");
        assert!(r.is_none());
    }

    #[test]
    fn discourse_picks_stable_title_for_stable() {
        let topics = load_topics();
        let t = pick_discourse_topic(&topics, &Channel::Stable).expect("a topic");
        assert!(t.title.contains("Stable"));
    }

    #[test]
    fn discourse_picks_testing_title_for_testing_channel() {
        let topics = load_topics();
        let t = pick_discourse_topic(&topics, &Channel::Testing).expect("a topic");
        assert!(t.title.contains("Testing"));
    }

    #[test]
    fn discourse_picks_unstable_title_for_unstable_channel() {
        let topics = load_topics();
        let t = pick_discourse_topic(&topics, &Channel::Unstable).expect("a topic");
        assert!(t.title.contains("Unstable"));
    }

    #[test]
    fn discourse_falls_back_to_first_when_no_match() {
        let topics = load_topics();
        let t = pick_discourse_topic(&topics, &Channel::Other("zzz".to_string())).expect("a topic");
        // First topic in fixture is the testing one.
        assert_eq!(t.id, 1003);
    }

    #[test]
    fn discourse_empty_topics_yields_none() {
        let r = pick_discourse_topic(&[], &Channel::Stable);
        assert!(r.is_none());
    }

    #[test]
    fn discourse_url_construction() {
        let topic = DiscourseTopic {
            id: 1234,
            title: "Foo".into(),
            slug: "foo-bar".into(),
        };
        let url = build_discourse_url("https://universal-blue.discourse.group/", &topic);
        assert_eq!(url, "https://universal-blue.discourse.group/t/foo-bar/1234");
    }

    #[test]
    fn stable_match_includes_unsuffixed_old_titles() {
        // Older Discourse posts predating the suffix convention (no
        // "Stable"/"Testing"/"Unstable" in title) should be treated as stable.
        let topics = vec![DiscourseTopic {
            id: 99,
            title: "Bazzite Update — 42.20251101".into(),
            slug: "bazzite-update-42-20251101".into(),
        }];
        let t = pick_discourse_topic(&topics, &Channel::Stable).expect("a topic");
        assert_eq!(t.id, 99);
    }

    #[test]
    fn github_slug_valid_names_accepted() {
        assert!(validate_github_slug("ublue-os", "owner").is_ok());
        assert!(validate_github_slug("bazzite", "repo").is_ok());
        assert!(validate_github_slug("My.Repo_123", "repo").is_ok());
    }

    #[test]
    fn github_slug_empty_rejected() {
        assert!(validate_github_slug("", "owner").is_err());
    }

    #[test]
    fn github_slug_path_separator_rejected() {
        // A slash would break the URL path segment.
        let err = validate_github_slug("foo/bar", "owner").unwrap_err();
        assert!(format!("{err}").contains('/'));
    }

    #[test]
    fn github_slug_query_chars_rejected() {
        assert!(validate_github_slug("foo?q=evil", "repo").is_err());
        assert!(validate_github_slug("foo#fragment", "repo").is_err());
    }
}
