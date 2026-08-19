pub mod curated;
pub mod model;
pub mod parser;
pub mod qualification;
pub mod web;

use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result};

/// Load everything the web process needs, once, at startup.
/// The local build never touches the network: the "scrape" is a
/// checked-in HTML fixture parsed by the same parser a live fetcher
/// would feed.
pub fn load_state(
    fixture: &Path,
    curated_path: &Path,
    stale_after: Duration,
) -> Result<web::AppState> {
    let html = fs::read_to_string(fixture)
        .with_context(|| format!("cannot read fixture {}", fixture.display()))?;
    let snapshot = parser::parse(&html, &fixture.display().to_string())?;
    let curated = curated::Curated::load(curated_path)?;
    let selection = qualification::select(&snapshot.rows, &curated);
    Ok(web::AppState {
        snapshot,
        curated,
        selection,
        stale_after,
    })
}
