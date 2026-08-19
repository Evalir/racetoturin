//! Regenerates `live/atp_ids.toml`: the article-title -> ATP-player-id map that
//! `crate::atp` compiles in.
//!
//! Only ever run by a human, never by the server. It reads the player titles
//! out of the standings (live article, or a fixture with `--fixture PATH`),
//! then asks Wikidata for each one's P536 value. Wikidata is used because it
//! publishes these ids as structured, CC0, API-accessible data keyed by the
//! same Wikipedia titles we already treat as identity — so the map is derived
//! from a permitted source rather than typed by hand.
//!
//! Existing entries are kept: a player who drops off the table keeps their id,
//! so the file only ever grows and a bad network day cannot silently empty it.
//! Titles that resolve to no id are reported, not guessed.
//!
//!     cargo run --bin refresh-atp-ids
//!     cargo run --bin refresh-atp-ids -- --fixture fixtures/race.wikitext

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result};
use racetoturin::{atp, curated::Curated, fetch::Fetcher, source};

const OUT: &str = "live/atp_ids.toml";
/// `wbgetentities` accepts 50 titles per request; one round trip covers the
/// whole field.
const BATCH: usize = 50;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut fixture: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => fixture = args.next().map(PathBuf::from),
            other => anyhow::bail!("unknown argument {other:?}"),
        }
    }

    let curated_path = std::env::var("RTT_CURATED").unwrap_or_else(|_| "live/curated.toml".into());
    let season = Curated::load(&PathBuf::from(&curated_path))?.season as i32;
    let fetcher = Fetcher::new()?;

    let (wikitext, origin) = match &fixture {
        Some(path) => (
            std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?,
            path.display().to_string(),
        ),
        None => {
            let page = std::env::var("RTT_WIKI_PAGE").unwrap_or_else(|_| "2026_ATP_Finals".into());
            let url = format!(
                "https://en.wikipedia.org/w/api.php?action=parse&format=json&formatversion=2\
                 &prop=wikitext&page={page}"
            );
            let body = fetcher.get(&url).await?;
            let value: serde_json::Value = serde_json::from_str(&body)?;
            let text = value
                .get("parse")
                .and_then(|p| p.get("wikitext"))
                .and_then(|w| w.as_str())
                .context("API response has no parse.wikitext")?
                .to_string();
            (text, format!("https://en.wikipedia.org/wiki/{page}"))
        }
    };

    let snapshot = source::parse(&wikitext, &origin, season)?;
    let mut titles: Vec<String> = snapshot
        .rows
        .iter()
        .map(|r| r.player_code.clone())
        .chain(snapshot.qualifiers.iter().map(|q| q.player_code.clone()))
        .collect();
    titles.sort();
    titles.dedup();

    let mut map = load_existing()?;
    let before = map.len();
    let wanted: Vec<&String> = titles.iter().filter(|t| !map.contains_key(*t)).collect();
    eprintln!(
        "{} players in {origin}; {} already mapped, resolving {}",
        titles.len(),
        titles.len() - wanted.len(),
        wanted.len()
    );

    let mut unresolved = Vec::new();
    for chunk in wanted.chunks(BATCH) {
        let resolved = resolve(&fetcher, chunk).await?;
        for title in chunk {
            match resolved.get(*title) {
                Some(id) if atp::valid_id(id) => {
                    map.insert((*title).clone(), id.clone());
                }
                Some(id) => unresolved.push(format!("{title} (rejected id {id:?})")),
                None => unresolved.push((*title).clone()),
            }
        }
    }

    write_out(&map, &origin)?;
    eprintln!("{OUT}: {} entries (+{})", map.len(), map.len() - before);
    if !unresolved.is_empty() {
        eprintln!(
            "no ATP id on Wikidata for {} title(s) — they will render unlinked:",
            unresolved.len()
        );
        for title in &unresolved {
            eprintln!("  - {title}");
        }
    }
    Ok(())
}

fn load_existing() -> Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(OUT) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("{OUT} is malformed")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("cannot read {OUT}")),
    }
}

/// One `wbgetentities` call: enwiki titles in, P536 values out. Statements
/// marked deprecated are skipped, and a `preferred` one wins over `normal` —
/// that is how Wikidata records a superseded id.
async fn resolve(fetcher: &Fetcher, titles: &[&String]) -> Result<BTreeMap<String, String>> {
    let joined = titles
        .iter()
        .map(|t| percent_encode(t))
        .collect::<Vec<_>>()
        .join("%7C");
    let url = format!(
        "https://www.wikidata.org/w/api.php?action=wbgetentities&sites=enwiki&titles={joined}\
         &props=claims%7Csitelinks&format=json&formatversion=2"
    );
    let body = fetcher.get(&url).await?;
    let value: serde_json::Value = serde_json::from_str(&body).context("Wikidata gave non-JSON")?;
    if let Some(info) = value.get("error").and_then(|e| e.get("info")) {
        anyhow::bail!("Wikidata API error: {info}");
    }

    let mut out = BTreeMap::new();
    let entities = value
        .get("entities")
        .and_then(|e| e.as_object())
        .context("Wikidata response has no entities")?;
    for entity in entities.values() {
        let Some(title) = entity
            .pointer("/sitelinks/enwiki/title")
            .and_then(|t| t.as_str())
        else {
            continue; // no English article: not one of the titles we asked about
        };
        let statements = entity
            .pointer("/claims/P536")
            .and_then(|c| c.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let pick = statements
            .iter()
            .filter(|s| s.get("rank").and_then(|r| r.as_str()) != Some("deprecated"))
            .max_by_key(|s| s.get("rank").and_then(|r| r.as_str()) == Some("preferred"));
        if let Some(id) = pick
            .and_then(|s| s.pointer("/mainsnak/datavalue/value"))
            .and_then(|v| v.as_str())
        {
            out.insert(title.to_string(), id.to_string());
        }
    }
    Ok(out)
}

fn write_out(map: &BTreeMap<String, String>, origin: &str) -> Result<()> {
    let mut text = String::new();
    text.push_str(
        "# GENERATED — do not edit by hand.\n\
         #\n\
         # Wikipedia article title -> ATP player id (Wikidata property P536, CC0),\n\
         # used to link a player's name to https://www.atptour.com/en/players/-/<id>/overview.\n\
         # Regenerate with `cargo run --bin refresh-atp-ids`.\n\
         #\n",
    );
    text.push_str(&format!("# Titles last read from: {origin}\n\n"));
    for (title, id) in map {
        // Titles carry spaces and disambiguating parentheses, so keys are
        // always quoted.
        text.push_str(&format!("{} = {}\n", quote(title), quote(id)));
    }

    std::fs::write(OUT, text).with_context(|| format!("cannot write {OUT}"))?;
    Ok(())
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Enough of RFC 3986 for an article title in a query string.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
