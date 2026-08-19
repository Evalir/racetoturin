use std::{path::PathBuf, sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use racetoturin::{storage::Store, Config};
use tower::ServiceExt;

fn root(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Builds the app the way `main` does, but from the checked-in fixture with
/// collection disabled — the whole suite runs offline.
async fn app(curated: &str) -> Router {
    let config = Config {
        wiki_page: "2026_ATP_Finals".to_string(),
        fixture: Some(root("fixtures/race.wikitext")),
        fetch_enabled: false,
        curated: root(curated),
        db: racetoturin::storage::MEMORY.to_string(),
        stale_after: Duration::from_secs(691_200),
        check_stale_after: Duration::from_secs(86_400),
        poll: Duration::from_secs(21_600),
        base_url: "https://racetotur.in".to_string(),
    };
    let store = Store::open(&config.db).await.unwrap();
    let state = racetoturin::ingest(&config, &store).await.unwrap();
    racetoturin::web::router(Arc::new(ArcSwap::from_pointee(state)))
}

async fn get_body(app: Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn homepage_shows_the_real_ordinary_cutoff() {
    let (status, body) = get_body(app("live/curated.toml").await, "/").await;
    assert_eq!(status, StatusCode::OK);
    // 2026's slam champions are all top-3, so seat 8 falls to race rank.
    assert!(body.contains("Novak Djokovic — by race rank"));
    assert!(body.contains("Provisional qualification line"));
    assert!(body.contains("Top seven — qualify directly on race rank"));
    assert!(!body.contains("slam-pick"));
    // Djokovic 2,320 over Auger-Aliassime 2,315 is a 5-point cushion.
    assert!(body.contains("+5"));
    assert!(body.contains("-5"));
    // Ingested official qualifications, with dates.
    assert!(body.contains("Jannik Sinner (2026-07-10)"));
    assert!(body.contains("Alexander Zverev (2026-08-06)"));
    assert!(body.contains("Official"));
    // Freshness is stated as the source's own date and never claims "live".
    assert!(body.contains("official weekly"));
    assert!(body.contains("16 August 2026"));
    assert!(!body.to_lowercase().contains("scraped live"));
    // Attribution is required by CC BY-SA.
    assert!(body.contains("Wikipedia"));
    assert!(body.contains("CC BY-SA"));
}

#[tokio::test]
async fn slam_champion_branch_still_renders() {
    let (status, body) = get_body(app("fixtures/curated_slam_branch.toml").await, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Grand Slam champion provision"));
    assert!(body.contains("slam-pick"));
    // No contiguous line exists in this branch, so margins are suppressed.
    assert!(!body.contains("Provisional qualification line"));
}

#[tokio::test]
async fn dropped_columns_are_gone() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    for absent in ["Max", "Total after one more win", "Tournament"] {
        assert!(!body.contains(absent), "{absent} should no longer render");
    }
}

#[tokio::test]
async fn methodology_and_health_endpoints_work() {
    let (status, body) = get_body(app("live/curated.toml").await, "/methodology").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("CC BY-SA"));
    assert!(body.contains("2026_ATP_Finals") || body.contains("2026 ATP Finals"));

    let (status, body) = get_body(app("live/curated.toml").await, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ready\n");
}

/// The monitoring hook: 200 when collection is current, 503 when it has
/// stopped succeeding, so a cron can alarm on the status code alone.
#[tokio::test]
async fn health_fresh_reports_collection_age() {
    let (status, body) = get_body(app("live/curated.toml").await, "/health/fresh").await;
    // Just ingested, so collection is current even though the source date is
    // several days old — the two clocks are deliberately separate.
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("fresh"), "unexpected: {body}");
    assert!(body.contains("checked_age_seconds="));
    assert!(body.contains("source_age_seconds="));
    assert!(body.contains("snapshot_version=1"));
}

/// A source that is old but freshly checked must not read as stale: Wikipedia
/// publishes weekly, so its stated date is routinely days old.
#[tokio::test]
async fn a_weekly_source_is_not_labelled_stale() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    assert!(
        !body.contains("Stale: showing the last verified snapshot"),
        "3-day-old weekly data must not be flagged stale"
    );
    assert!(
        !body.contains("collection may be failing"),
        "a snapshot we just ingested must not warn about collection"
    );
}

/// Cache headers are the cost control: they let a CDN absorb a traffic spike
/// instead of the single machine serving every hit.
#[tokio::test]
async fn pages_are_cacheable_and_declare_their_snapshot() {
    let response = app("live/curated.toml")
        .await
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let headers = response.headers();
    let cache = headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(cache.contains("max-age="), "no max-age: {cache:?}");
    assert!(
        cache.contains("stale-while-revalidate"),
        "no stale-while-revalidate: {cache:?}"
    );
    assert_eq!(
        headers.get("x-snapshot-version").unwrap().to_str().unwrap(),
        "1"
    );
}

/// A shared link should carry the actual news, not a bare URL.
#[tokio::test]
async fn shared_link_preview_and_discovery_tags_are_present() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    assert!(body.contains("og:title"));
    assert!(body.contains("og:url"));
    assert!(body.contains("twitter:card"));
    assert!(body.contains("rel=\"canonical\""));
    assert!(body.contains("https://racetotur.in"));
    assert!(body.contains("rel=\"icon\""));
    // The description names who holds the last seat and who is closest out.
    assert!(body.contains("Seat 8: Novak Djokovic."));
    assert!(body.contains("First alternate: Félix Auger-Aliassime."));

    let (status, robots) = get_body(app("live/curated.toml").await, "/robots.txt").await;
    assert_eq!(status, StatusCode::OK);
    assert!(robots.contains("Allow: /"));
    assert!(robots.contains("Disallow: /health/"));
}

#[tokio::test]
async fn css_is_served() {
    let (status, body) = get_body(app("live/curated.toml").await, "/static/app.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("cutline"));
}
