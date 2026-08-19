pub mod atp;
pub mod curated;
pub mod fetch;
pub mod model;
pub mod qualification;
pub mod source;
pub mod storage;
pub mod web;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use arc_swap::ArcSwap;

pub struct Config {
    /// Wikipedia article carrying this season's standings.
    pub wiki_page: String,
    /// Parse this local file instead of fetching (tests and offline runs).
    pub fixture: Option<PathBuf>,
    /// Kill switch: false makes zero outbound requests.
    pub fetch_enabled: bool,
    pub curated: PathBuf,
    pub db: String,
    /// How old the source's own stated date may get before the table is
    /// labelled stale. Must accommodate a weekly source.
    pub stale_after: Duration,
    /// How long since our last *successful* collection before we warn that
    /// collection itself is failing. Should be a small multiple of `poll`.
    pub check_stale_after: Duration,
    pub poll: Duration,
    /// Public origin, for canonical and shared-link metadata.
    pub base_url: String,
}

impl Config {
    pub fn source_url(&self) -> String {
        format!(
            "https://en.wikipedia.org/w/api.php?action=parse&format=json&formatversion=2\
             &prop=wikitext&page={}",
            self.wiki_page
        )
    }
}

/// Pull the wikitext out of an `action=parse` response.
fn wikitext_from_api(body: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("API response is not JSON")?;
    if let Some(info) = value.get("error").and_then(|e| e.get("info")) {
        anyhow::bail!("Wikipedia API error: {info}");
    }
    value
        .get("parse")
        .and_then(|p| p.get("wikitext"))
        .and_then(|w| w.as_str())
        .map(str::to_string)
        .context("API response has no parse.wikitext")
}

/// Fetch (or read) the source, parse and validate it, publish if the content
/// changed, then build the state the web process serves from whatever the
/// current pointer designates.
pub async fn ingest(config: &Config, store: &storage::Store) -> Result<web::AppState> {
    let curated = curated::Curated::load(&config.curated)?;

    let candidate = match (&config.fixture, config.fetch_enabled) {
        (Some(path), _) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            Some(source::parse(
                &text,
                &path.display().to_string(),
                curated.season as i32,
            )?)
        }
        (None, true) => {
            let url = config.source_url();
            let body = fetch::Fetcher::new()?.get(&url).await?;
            let wikitext = wikitext_from_api(&body)?;
            Some(source::parse(
                &wikitext,
                &format!("https://en.wikipedia.org/wiki/{}", config.wiki_page),
                curated.season as i32,
            )?)
        }
        (None, false) => None, // kill switch: serve whatever is stored
    };

    if let Some(candidate) = &candidate {
        let outcome = store.publish_if_changed(candidate).await?;
        eprintln!(
            "snapshot v{} ({}) · {} rows · {} qualifiers · source as of {}",
            outcome.version,
            if outcome.created {
                "new"
            } else {
                "content unchanged"
            },
            candidate.rows.len(),
            candidate.qualifiers.len(),
            candidate.source_as_of.date(),
        );
    }

    let (version, snapshot) = store
        .load_current()
        .await?
        .context("no snapshot available: the store is empty and collection is disabled or failed")?;
    // A new entrant renders unlinked until someone reruns `refresh-atp-ids`.
    // That degrades correctly but invisibly, so name them in the log.
    let unlinked: Vec<&str> = snapshot
        .rows
        .iter()
        .filter(|r| atp::profile_url(&r.player_code).is_none())
        .map(|r| r.player_name.as_str())
        .collect();
    if !unlinked.is_empty() {
        eprintln!(
            "no ATP profile id for {}: {} — run `cargo run --bin refresh-atp-ids`",
            unlinked.len(),
            unlinked.join(", "),
        );
    }

    let selection = qualification::select(&snapshot.rows, &curated);
    Ok(web::AppState {
        snapshot,
        version,
        curated,
        selection,
        stale_after: config.stale_after,
        check_stale_after: config.check_stale_after,
        base_url: config.base_url.trim_end_matches('/').to_string(),
    })
}

/// Refresh on a schedule. A failed cycle logs and leaves the served snapshot
/// untouched, so an upstream outage degrades to visibly aged data.
pub async fn refresh_loop(
    config: Arc<Config>,
    store: Arc<storage::Store>,
    state: Arc<ArcSwap<web::AppState>>,
) {
    if !config.fetch_enabled || config.fixture.is_some() {
        return; // nothing to poll
    }
    loop {
        tokio::time::sleep(config.poll).await;
        match ingest(&config, &store).await {
            Ok(fresh) => state.store(Arc::new(fresh)),
            Err(err) => eprintln!("refresh failed, keeping snapshot v{}: {err:#}", state.load().version),
        }
    }
}
