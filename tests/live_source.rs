//! Hits the real Wikipedia API. Ignored by default so the suite — and CI —
//! stay offline; run with `cargo test -- --ignored`.

use std::{path::PathBuf, time::Duration};

use racetoturin::{storage::MEMORY, Config};

#[tokio::test]
#[ignore = "network"]
async fn live_wikipedia_article_parses_and_validates() {
    let config = Config {
        wiki_page: "2026_ATP_Finals".to_string(),
        fixture: None,
        fetch_enabled: true,
        curated: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("live/curated.toml"),
        db: MEMORY.to_string(),
        stale_after: Duration::from_secs(864_000),
        poll: Duration::from_secs(21_600),
        base_url: "https://racetotur.in".to_string(),
    };
    let store = racetoturin::storage::Store::open(&config.db).await.unwrap();
    let state = racetoturin::ingest(&config, &store)
        .await
        .expect("live article must parse and validate");

    // Enough of the table to cover the qualification rule's full reach.
    assert!(
        state.snapshot.rows.len() >= 15,
        "expected a full table, got {}",
        state.snapshot.rows.len()
    );
    assert_eq!(state.snapshot.rows[0].rank, 1);
    assert!(state.snapshot.rows[0].race_points > 0);
    // The article always states the date its standings are current to.
    assert!(state.snapshot.source_as_of.year() >= 2026);
    // Whoever holds seat 8 must be someone in the table.
    let eighth = state.selection.eighth_code.as_deref().unwrap();
    assert!(state
        .snapshot
        .rows
        .iter()
        .any(|r| r.player_code == eighth));
}
