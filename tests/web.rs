use std::{path::Path, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app(curated_file: &str) -> Router {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let loaded = racetoturin::ingest_and_load(
        &root.join("fixtures/race.html"),
        &root.join(curated_file),
        Duration::from_secs(900),
        racetoturin::storage::MEMORY,
    )
    .await
    .expect("state must load from checked-in fixtures");
    racetoturin::web::router(Arc::new(loaded.state))
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
async fn homepage_renders_slam_champion_branch() {
    let (status, body) = get_body(app("config/curated.toml").await, "/").await;
    assert_eq!(status, StatusCode::OK);
    // Qualification summary and the highlighted slam pick.
    assert!(body.contains("Novak Djokovic — Grand Slam champion provision"));
    assert!(body.contains("slam-pick"));
    assert!(body.contains("First alternate"));
    assert!(body.contains("Alex de Minaur"));
    // No contiguous 8/9 line in this branch, and margins are suppressed.
    assert!(!body.contains("Provisional qualification line"));
    // Strong boundary after the top seven is always drawn.
    assert!(body.contains("Top seven — qualify directly on race rank"));
    // Official status comes from the curated file, not points.
    assert!(body.contains("Official"));
    // Unofficial labelling and provenance are visible.
    assert!(body.contains("Independent and unofficial"));
    assert!(body.contains("source age"));
    assert!(body.contains("parser fixture-html-1"));
    assert!(body.contains("snapshot v1"));
}

#[tokio::test]
async fn homepage_renders_ordinary_cutoff_branch() {
    let (status, body) = get_body(app("config/curated_ordinary.toml").await, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Alex de Minaur — by race rank"));
    assert!(body.contains("Provisional qualification line"));
    // de Minaur (3310) cushion over Djokovic (3180) = +130; chasers show deficits.
    assert!(body.contains("+130"));
    assert!(body.contains("-130"));
    assert!(!body.contains("slam-pick"));
}

#[tokio::test]
async fn idle_players_show_reason_not_estimates() {
    let (_, body) = get_body(app("config/curated.toml").await, "/").await;
    assert!(body.contains("Not entered"));
    assert!(body.contains("Unavailable: Eliminated R32"));
}

#[tokio::test]
async fn methodology_and_health_endpoints_work() {
    let (status, body) = get_body(app("config/curated.toml").await, "/methodology").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("zero network requests"));

    let (status, body) = get_body(app("config/curated.toml").await, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ready\n");
}

#[tokio::test]
async fn css_is_served() {
    let (status, body) = get_body(app("config/curated.toml").await, "/static/app.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("cutline"));
}
