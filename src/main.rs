use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let fixture = PathBuf::from(env_or("RTT_FIXTURE", "fixtures/race.html"));
    let curated = PathBuf::from(env_or("RTT_CURATED", "config/curated.toml"));
    let db = env_or("RTT_DB", "data/racetoturin.db");
    let bind = env_or("RTT_BIND", "127.0.0.1:8080");
    let stale_after = Duration::from_secs(
        env_or("RTT_STALE_AFTER_SECS", "900")
            .parse()
            .context("RTT_STALE_AFTER_SECS must be an integer number of seconds")?,
    );

    let loaded = racetoturin::ingest_and_load(&fixture, &curated, stale_after, &db).await?;
    eprintln!(
        "snapshot v{} ({}) · {} rows from {} · eighth-seat basis {:?}",
        loaded.state.version,
        if loaded.created { "new" } else { "content unchanged" },
        loaded.state.snapshot.rows.len(),
        loaded.state.snapshot.source,
        loaded.state.selection.eighth_basis,
    );

    let app = racetoturin::web::router(Arc::new(loaded.state));
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("cannot bind {bind}"))?;
    eprintln!("serving http://{bind}/  (local build: no network requests are ever made)");
    axum::serve(listener, app).await?;
    Ok(())
}
