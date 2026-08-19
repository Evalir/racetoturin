use std::path::Path;

use racetoturin::{
    model::Snapshot,
    parser,
    storage::{Store, MEMORY},
};

fn fixture_snapshot() -> Snapshot {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let html = std::fs::read_to_string(root.join("fixtures/race.html")).unwrap();
    parser::parse(&html, "fixtures/race.html").unwrap()
}

#[tokio::test]
async fn publish_dedupes_versions_and_roundtrips() {
    let store = Store::open(MEMORY).await.unwrap();

    let first = store.publish_if_changed(&fixture_snapshot()).await.unwrap();
    assert!(first.created);
    assert_eq!(first.version, 1);

    // Identical content: nothing new is written.
    let second = store.publish_if_changed(&fixture_snapshot()).await.unwrap();
    assert!(!second.created);
    assert_eq!(second.version, 1);

    // Changed content: a new immutable version, and the pointer moves.
    let mut changed = fixture_snapshot();
    changed.rows[0].live_points += 10;
    changed.rows[0].next_points = changed.rows[0].next_points.map(|v| v + 10);
    changed.rows[0].max_this_week = changed.rows[0].max_this_week.map(|v| v + 10);
    let third = store.publish_if_changed(&changed).await.unwrap();
    assert!(third.created);
    assert_eq!(third.version, 2);

    let (version, loaded) = store.load_current().await.unwrap().unwrap();
    assert_eq!(version, 2);
    assert_eq!(loaded.rows.len(), changed.rows.len());
    assert_eq!(loaded.rows[0].live_points, changed.rows[0].live_points);

    // Option fields and accented names survive the roundtrip.
    let felix = loaded.rows.iter().find(|r| r.player_code == "ag37").unwrap();
    assert_eq!(felix.player_name, "Félix Auger-Aliassime");
    assert_eq!(felix.next_points, None);
    assert_eq!(felix.unavailable_reason.as_deref(), Some("Eliminated R16"));
    let fritz = loaded.rows.iter().find(|r| r.player_code == "fb98").unwrap();
    assert_eq!(fritz.event.as_ref().unwrap().round, "QF");
    assert_eq!(fritz.next_points, Some(4320));
}

#[tokio::test]
async fn empty_store_has_no_current_snapshot() {
    let store = Store::open(MEMORY).await.unwrap();
    assert!(store.load_current().await.unwrap().is_none());
}
