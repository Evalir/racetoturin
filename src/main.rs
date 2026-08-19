use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => serve().await,
        Some("parse") => {
            let path = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env_or("RTT_FIXTURE", "fixtures/race.html")));
            let html = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let snapshot = racetoturin::parser::parse(&html, &path.display().to_string())?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            Ok(())
        }
        Some(other) => {
            bail!("unknown command {other:?}\nusage: racetoturin [serve | parse <fixture.html>]")
        }
    }
}

async fn serve() -> Result<()> {
    let fixture = PathBuf::from(env_or("RTT_FIXTURE", "fixtures/race.html"));
    let curated = PathBuf::from(env_or("RTT_CURATED", "config/curated.toml"));
    let bind = env_or("RTT_BIND", "127.0.0.1:8080");
    let stale_after = Duration::from_secs(
        env_or("RTT_STALE_AFTER_SECS", "900")
            .parse()
            .context("RTT_STALE_AFTER_SECS must be an integer number of seconds")?,
    );

    let state = racetoturin::load_state(&fixture, &curated, stale_after)?;
    eprintln!(
        "loaded {} rows from {} · ruleset {} · eighth-seat basis {:?}",
        state.snapshot.rows.len(),
        state.snapshot.source,
        state.curated.ruleset,
        state.selection.eighth_basis,
    );

    let app = racetoturin::web::router(Arc::new(state));
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("cannot bind {bind}"))?;
    eprintln!("serving http://{bind}/  (local build: no upstream requests are ever made)");
    axum::serve(listener, app).await?;
    Ok(())
}
