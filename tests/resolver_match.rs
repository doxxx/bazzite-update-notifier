//! Integration tests for the resolver's pure selection logic
//! (`channel_from_image_ref`, `pick_github_release`, `pick_discourse_topic`).
//! Network paths are not exercised here.

use bazzite_update_notifier::resolver::{
    build_discourse_url, channel_from_image_ref, pick_discourse_topic, pick_github_release,
    Channel, DiscourseTagPage, DiscourseTopic, GitHubRelease,
};

fn load_releases() -> Vec<GitHubRelease> {
    serde_json::from_str(include_str!("fixtures/github_releases_mixed.json")).unwrap()
}

fn load_topics() -> Vec<DiscourseTopic> {
    let page: DiscourseTagPage =
        serde_json::from_str(include_str!("fixtures/discourse_bazzite_news.json")).unwrap();
    page.topic_list.topics
}

#[test]
fn channel_inference_table() {
    let cases = [
        (
            "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:stable",
            Channel::Stable,
        ),
        (
            "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:testing",
            Channel::Testing,
        ),
        (
            "ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:unstable",
            Channel::Unstable,
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(channel_from_image_ref(input), expected, "{input}");
    }
    // Unknown tag → Other(tag).
    let other =
        channel_from_image_ref("ostree-image-signed:docker://ghcr.io/ublue-os/bazzite:nightly");
    assert!(matches!(other, Channel::Other(s) if s == "nightly"));
}

#[test]
fn github_stable_picks_stable() {
    let releases = load_releases();
    let r = pick_github_release(&releases, &Channel::Stable, "42.20260512.0").expect("a release");
    assert_eq!(r.tag_name, "stable-20260512");
}

#[test]
fn github_testing_picks_testing_not_unstable() {
    // unstable-20260513 is dated *after* testing-20260513 in the fixture
    // ordering; the channel filter must keep us on testing.
    let releases = load_releases();
    let r = pick_github_release(&releases, &Channel::Testing, "42.20260513.0").expect("a release");
    assert_eq!(r.tag_name, "testing-20260513");
}

#[test]
fn github_falls_back_to_newest_in_channel_on_version_miss() {
    let releases = load_releases();
    let r = pick_github_release(&releases, &Channel::Stable, "42.20260901.0").expect("a release");
    // Newest stable release in fixture.
    assert_eq!(r.tag_name, "stable-20260512");
}

#[test]
fn github_other_channel_no_filter() {
    let releases = load_releases();
    let r =
        pick_github_release(&releases, &Channel::Other("nightly".into()), "x").expect("a release");
    // First in the list (newest overall).
    assert_eq!(r.tag_name, "testing-20260513");
}

#[test]
fn discourse_stable_titled_topic() {
    let topics = load_topics();
    let t = pick_discourse_topic(&topics, &Channel::Stable).unwrap();
    assert!(t.title.contains("Stable"), "{}", t.title);
}

#[test]
fn discourse_testing_titled_topic() {
    let topics = load_topics();
    let t = pick_discourse_topic(&topics, &Channel::Testing).unwrap();
    assert!(t.title.contains("Testing"), "{}", t.title);
}

#[test]
fn discourse_unstable_titled_topic() {
    let topics = load_topics();
    let t = pick_discourse_topic(&topics, &Channel::Unstable).unwrap();
    assert!(t.title.contains("Unstable"), "{}", t.title);
}

#[test]
fn discourse_url_round_trip() {
    let topic = DiscourseTopic {
        id: 42,
        title: "foo".into(),
        slug: "bar-baz".into(),
    };
    assert_eq!(
        build_discourse_url("https://example.com", &topic),
        "https://example.com/t/bar-baz/42"
    );
    // Trailing slash is normalized.
    assert_eq!(
        build_discourse_url("https://example.com/", &topic),
        "https://example.com/t/bar-baz/42"
    );
}
