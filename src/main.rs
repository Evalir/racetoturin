use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use racetoturin::{ingest, refresh_loop, storage::Store, web, Config};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn secs(key: &str, default: &str) -> Result<Duration> {
    Ok(Duration::from_secs(
        env_or(key, default)
            .parse()
            .with_context(|| format!("{key} must be an integer number of seconds"))?,
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Arc::new(Config {
        wiki_page: env_or("RTT_WIKI_PAGE", "2026_ATP_Finals"),
        fixture: std::env::var("RTT_FIXTURE").ok().map(PathBuf::from),
        fetch_enabled: env_or("RTT_FETCH", "1") != "0",
        curated: PathBuf::from(env_or("RTT_CURATED", "live/curated.toml")),
        db: env_or("RTT_DB", "data/racetoturin.db"),
        // A week plus slack: the source itself only publishes weekly.
        stale_after: secs("RTT_STALE_AFTER_SECS", "691200")?, // 8 days
        // We poll every 6h, so a day without a successful check means broken.
        check_stale_after: secs("RTT_CHECK_STALE_AFTER_SECS", "86400")?, // 1 day
        poll: secs("RTT_POLL_SECS", "21600")?,                           // 6 h
        base_url: env_or("RTT_BASE_URL", "https://racetotur.in"),
    });
    let bind = env_or("RTT_BIND", "127.0.0.1:8080");

    let store = Arc::new(Store::open(&config.db).await?);
    let state = Arc::new(ArcSwap::from_pointee(ingest(&config, &store).await?));

    tokio::spawn(refresh_loop(
        Arc::clone(&config),
        Arc::clone(&store),
        Arc::clone(&state),
    ));

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("cannot bind {bind}"))?;
    eprintln!(
        "serving http://{bind}/ · source {} · refresh every {}s",
        if config.fetch_enabled && config.fixture.is_none() {
            config.source_url()
        } else {
            "local (collection disabled)".to_string()
        },
        config.poll.as_secs(),
    );
    axum::serve(listener, web::router(state)).await?;
    Ok(())
}
