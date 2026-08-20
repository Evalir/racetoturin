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

/// The response contract a shared or cached page depends on: a CDN-cacheable
/// header, a link preview carrying the actual news, and a stylesheet URL that
/// changes with its content (a stale one renders new markup wrong).
#[tokio::test]
async fn response_carries_caching_and_link_metadata() {
    let response = app("live/curated.toml")
        .await
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cache = response
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cache.contains("max-age="), "not cacheable: {cache:?}");
    assert!(
        cache.contains("stale-while-revalidate"),
        "cannot be served stale while refreshing: {cache:?}"
    );
    assert_eq!(
        response
            .headers()
            .get("x-snapshot-version")
            .unwrap()
            .to_str()
            .unwrap(),
        "1"
    );

    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    assert!(body.contains(&format!("/static/app.css?v={}", racetoturin::web::css_version())));
    assert!(body.contains("og:title") && body.contains("twitter:card"));
    // The preview names who holds the last seat and who is closest out.
    assert!(body.contains("Seat 8: Novak Djokovic."));
    assert!(body.contains("First alternate: Félix Auger-Aliassime."));

    let (status, robots) = get_body(app("live/curated.toml").await, "/robots.txt").await;
    assert_eq!(status, StatusCode::OK);
    assert!(robots.contains("Disallow: /health/"));
}

/// Served immutable, which is only safe because the URL is content-addressed.
#[tokio::test]
async fn css_is_served_immutably() {
    let response = app("live/curated.toml")
        .await
        .oneshot(Request::get("/static/app.css").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cache = response
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(cache.contains("immutable"), "expected immutable: {cache:?}");
}

/// Scrolling the table is preferred over shortening status labels, so the
/// labels must render in full.
#[tokio::test]
async fn status_labels_are_not_abbreviated() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    assert!(body.contains(">First alternate<"));
    assert!(body.contains(">Official<"));
    assert!(body.contains(">Top 7<"));
}

#[tokio::test]
async fn player_names_link_to_their_atp_profile() {
    let (status, body) = get_body(app("live/curated.toml").await, "/").await;
    assert_eq!(status, StatusCode::OK);

    // Every player in the fixture has a Wikidata-sourced id, so every name in
    // the table should be a link — a regression that drops the map would
    // silently render plain text instead.
    for (name, id) in [
        ("Jannik Sinner", "S0AG"),
        ("Carlos Alcaraz", "A0E2"),
        // Accented and disambiguated titles are the ones a hand-built slug
        // would get wrong; the id-only URL sidesteps the problem entirely.
        ("Félix Auger-Aliassime", "AG37"),
        ("Jakub Menšík", "M0NI"),
    ] {
        let expected =
            format!("<a href=\"https://www.atptour.com/en/players/-/{id}/overview\">{name}</a>");
        assert!(body.contains(&expected), "missing profile link: {expected}");
    }
}

#[tokio::test]
async fn a_player_with_no_atp_id_is_rendered_unlinked() {
    // Nothing links to a guessed URL: an unmapped title yields no link at all.
    assert_eq!(
        racetoturin::atp::profile_url("Someone Not On Wikidata"),
        None
    );

    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    // And no link is ever built from an id that fails P536's format check.
    for bad in ["/players/-//overview", "/players/-/overview"] {
        assert!(!body.contains(bad), "malformed profile URL in page: {bad}");
    }
}

/// The breakdown expands with no JavaScript at all: a native <details> per
/// player. The whole point of the page is that it ships no script, so a
/// regression that reached for one has to fail here.
#[tokio::test]
async fn the_points_breakdown_expands_without_javascript() {
    let (status, body) = get_body(app("live/curated.toml").await, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("<script"), "the page must ship no script");
    assert!(!body.contains("onclick"), "no inline handlers either");

    // One disclosure per player. The qualification-rules panel carries a class,
    // so this counts ledgers only.
    assert_eq!(body.matches("<details>").count(), 12);

    // The summary states how much of the season the breakdown accounts for,
    // rather than implying it lists everything the player entered.
    assert!(body.contains("15 of 19 tournaments count · 1 title"));
    assert!(body.contains("6 of 6 tournaments count · no titles"));
    // And it names the player for a screen reader meeting twelve of these.
    assert!(body.contains("Points breakdown for Flavio Cobolli:"));

    // A result, its round, and a link to the draw it came from.
    assert!(body.contains(">Australian Open</a>"));
    assert!(body.contains("https://en.wikipedia.org/wiki/2026_Australian_Open"));
    assert!(body.contains(">1,300</span>"));
}

/// A next-best result counted in a mandatory Masters slot has to name the event
/// actually played and the slot it replaced. Reporting the column's own event
/// would claim a tournament the player never entered.
#[tokio::test]
async fn a_substituted_masters_result_is_labelled_as_one() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    assert!(body.contains(">ASB Classic</a> <span class=\"qual\">for Miami</span>"));
    assert!(body.contains("<li class=\"sub\">"));
    assert!(body.contains("counted in place of a mandatory Masters 1000"));
    // The column is Miami's, but Shelton played no Miami: it must not appear as
    // a result of his.
    assert!(!body.contains(">Miami Open</a>"));
}

/// Skipping an event and waiting for one to be played are different facts, and
/// a symbol alone does not convey either to a screen reader.
#[tokio::test]
async fn skipped_and_unplayed_events_read_differently() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    assert!(body.contains("did not play"));
    assert!(body.contains("not played yet"));
    assert!(body.contains("<li class=\"absent\">"));
    assert!(body.contains("<li class=\"pending\">"));
}

/// The breakdown is shown only when it accounts for the total exactly, so the
/// displayed total is always one the reader can verify by adding up the rows.
#[tokio::test]
async fn every_displayed_breakdown_adds_up_to_its_total() {
    let (_, body) = get_body(app("live/curated.toml").await, "/").await;
    let mut checked = 0;
    for block in body.split("<details>").skip(1) {
        let Some(end) = block.find("</details>") else {
            continue;
        };
        let block = &block[..end];
        if !block.contains("lg-total") {
            continue; // the qualification-rules disclosure, not a ledger
        }
        let sum: u32 = block
            .split("<span class=\"pt\">")
            .skip(1)
            .filter_map(|cell| cell.split('<').next())
            .filter(|cell| !cell.contains('–'))
            .filter_map(|cell| cell.replace(',', "").parse::<u32>().ok())
            .sum();
        let total: u32 = block
            .rsplit("<span class=\"pt\">")
            .next()
            .and_then(|c| c.split('<').next())
            .and_then(|c| c.replace(',', "").parse().ok())
            .expect("a ledger states a total");
        // The stated total is itself one of the `pt` cells, so the entries sum
        // to exactly half of what the scan collected.
        assert_eq!(sum, total * 2, "a breakdown does not add up to its total");
        checked += 1;
    }
    assert_eq!(checked, 12, "every player should carry a breakdown");
}

/// A snapshot stored before ledgers existed has no breakdowns, and the page
/// must not advertise an expansion that is not there — while still serving
/// every row and every point total.
#[tokio::test]
async fn a_snapshot_without_breakdowns_still_serves_and_promises_nothing() {
    use racetoturin::{model::Snapshot, source, storage::Store};

    let mut snapshot: Snapshot = source::parse(
        &std::fs::read_to_string(root("fixtures/race.wikitext")).unwrap(),
        "fixtures/race.wikitext",
        2026,
    )
    .unwrap();
    for row in &mut snapshot.rows {
        row.results.clear(); // as a pre-ledger snapshot loads back
    }

    let store = Store::open(racetoturin::storage::MEMORY).await.unwrap();
    store.publish_if_changed(&snapshot).await.unwrap();
    let (version, loaded) = store.load_current().await.unwrap().unwrap();
    let curated = racetoturin::curated::Curated::load(&root("live/curated.toml")).unwrap();
    let selection = racetoturin::qualification::select(&loaded.rows, &curated);
    let state = racetoturin::web::AppState {
        snapshot: loaded,
        version,
        curated,
        selection,
        stale_after: Duration::from_secs(691_200),
        check_stale_after: Duration::from_secs(86_400),
        base_url: "https://racetotur.in".to_string(),
    };
    let app = racetoturin::web::router(Arc::new(ArcSwap::from_pointee(state)));

    let (status, body) = get_body(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("<details>"), "no ledger, so no disclosure");
    assert!(
        !body.contains("Every row expands"),
        "the page must not offer an expansion it cannot deliver"
    );
    // The standings themselves are untouched: this is a secondary feature.
    assert!(body.contains("7,950"));
    assert!(body.contains("Novak Djokovic — by race rank"));
}
