pub mod curated;
pub mod model;
pub mod parser;
pub mod qualification;
pub mod storage;
pub mod web;

use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result};

pub struct Loaded {
    pub state: web::AppState,
    /// False when the store already held this content and nothing was written.
    pub created: bool,
}

/// The full local pipeline, once, at startup: parse the fixture into a
/// validated candidate, publish it to SQLite unless its content is
/// unchanged, then load whatever the current pointer designates into the
/// immutable in-memory state the web process serves. The database is never
/// on the request path, and the local build never touches the network.
pub async fn ingest_and_load(
    fixture: &Path,
    curated_path: &Path,
    stale_after: Duration,
    db_path: &str,
) -> Result<Loaded> {
    let html = fs::read_to_string(fixture)
        .with_context(|| format!("cannot read fixture {}", fixture.display()))?;
    let candidate = parser::parse(&html, &fixture.display().to_string())?;
    let curated = curated::Curated::load(curated_path)?;

    let store = storage::Store::open(db_path).await?;
    let outcome = store.publish_if_changed(&candidate).await?;
    let (version, snapshot) = store
        .load_current()
        .await?
        .context("no current snapshot after publish")?;
    store.close().await;

    let selection = qualification::select(&snapshot.rows, &curated);
    Ok(Loaded {
        state: web::AppState {
            snapshot,
            version,
            curated,
            selection,
            stale_after,
        },
        created: outcome.created,
    })
}
