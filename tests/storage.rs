use racetoturin::{
    model::Snapshot,
    source,
    storage::{Store, MEMORY},
};

fn fixture_snapshot() -> Snapshot {
    source::parse(
        include_str!("../fixtures/race.wikitext"),
        "fixtures/race.wikitext",
        2026,
    )
    .unwrap()
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
    changed.rows[0].race_points += 500;
    let third = store.publish_if_changed(&changed).await.unwrap();
    assert!(third.created);
    assert_eq!(third.version, 2);

    let (version, loaded) = store.load_current().await.unwrap().unwrap();
    assert_eq!(version, 2);
    assert_eq!(loaded.rows.len(), changed.rows.len());
    assert_eq!(loaded.rows[0].race_points, changed.rows[0].race_points);

    // Accents, empty country, and qualifiers survive the roundtrip.
    let jodar = loaded
        .rows
        .iter()
        .find(|r| r.player_code == "Rafael Jódar")
        .expect("accented name roundtrips");
    assert_eq!(jodar.country, "ESP");
    let med = loaded
        .rows
        .iter()
        .find(|r| r.player_code == "Daniil Medvedev")
        .unwrap();
    assert_eq!(med.country, "");
    assert_eq!(loaded.qualifiers.len(), 2);
    assert_eq!(loaded.qualifiers[0].player_code, "Jannik Sinner");
    assert_eq!(loaded.qualifiers[0].qualified_on.to_string(), "2026-07-10");
    assert!(loaded.qualifiers[0].source_url.starts_with("https://"));
}

/// An announcement alone must publish a new version, even if no points moved.
#[tokio::test]
async fn qualifier_change_alone_publishes_a_new_version() {
    let store = Store::open(MEMORY).await.unwrap();
    let mut base = fixture_snapshot();
    base.qualifiers.truncate(1);
    assert_eq!(store.publish_if_changed(&base).await.unwrap().version, 1);

    let full = fixture_snapshot(); // both qualifiers
    let out = store.publish_if_changed(&full).await.unwrap();
    assert!(out.created);
    assert_eq!(out.version, 2);
}

/// Movement is derived from our own history, not taken from a source.
#[tokio::test]
async fn movement_is_derived_from_the_previous_snapshot() {
    let store = Store::open(MEMORY).await.unwrap();
    store.publish_if_changed(&fixture_snapshot()).await.unwrap();

    // First snapshot has no predecessor, so no movement.
    let (_, first) = store.load_current().await.unwrap().unwrap();
    assert!(first.rows.iter().all(|r| r.movement.is_none()));

    // Swap ranks 1 and 2, keeping points consistent with the new order.
    let mut next = fixture_snapshot();
    let (a, b) = (next.rows[0].clone(), next.rows[1].clone());
    next.rows[0] = RaceRowSwap::with_rank(b, 1);
    next.rows[1] = RaceRowSwap::with_rank(a, 2);
    next.rows[0].race_points = 9000;
    next.rows[1].race_points = 8000;
    store.publish_if_changed(&next).await.unwrap();

    let (_, loaded) = store.load_current().await.unwrap().unwrap();
    let moved_up = loaded.rows.iter().find(|r| r.rank == 1).unwrap();
    let moved_down = loaded.rows.iter().find(|r| r.rank == 2).unwrap();
    assert_eq!(moved_up.movement, Some(1), "rank 2 -> 1 is up one place");
    assert_eq!(moved_down.movement, Some(-1), "rank 1 -> 2 is down one place");
}

struct RaceRowSwap;
impl RaceRowSwap {
    fn with_rank(mut row: racetoturin::model::RaceRow, rank: u32) -> racetoturin::model::RaceRow {
        row.rank = rank;
        row
    }
}

#[tokio::test]
async fn empty_store_has_no_current_snapshot() {
    let store = Store::open(MEMORY).await.unwrap();
    assert!(store.load_current().await.unwrap().is_none());
}
