# racetotur.in

Who would qualify for the ATP Finals in Turin if selection happened now?

The app fetches the standings itself: parse Wikipedia's `<season> ATP Finals`
article → validate the candidate → publish to SQLite (append-only, content-hash
dedup, atomic pointer) → apply the versioned Turin qualification rule → serve
from memory as server-rendered HTML. No JavaScript is served. It refreshes every
six hours and never claims to be fresher than its source.

## Run

```sh
cargo run                # → http://127.0.0.1:8080/
docker compose up        # same, SQLite persisted in a volume
```

Prebuilt image: `ghcr.io/evalir/racetoturin`.

## The data

Standings, per-player countries, and **official qualifications with the URL that
announced each one** come from the English Wikipedia article
[2026 ATP Finals](https://en.wikipedia.org/wiki/2026_ATP_Finals), read through
Wikipedia's public API. Content there is CC BY-SA 4.0 — explicitly licensed for
reuse with attribution, which the footer and `/methodology` provide.

Why not the ATP directly: `atptour.com` refuses automated clients, and this
project does not disguise itself or bypass access controls to get around that.
tennisexplorer was evaluated and rejected — its terms (§2.11) explicitly forbid
scraping and aggregating. Wikipedia is also simply the better source: it carries
a per-tournament points ledger and a citation for every announced qualification.

Two consequences, both deliberate:

- **This is weekly data, not live.** Wikipedia states the date its standings are
  current to, and the page shows exactly that date. Past `RTT_STALE_AFTER_SECS`
  it labels itself stale rather than implying freshness it doesn't have.
- **No "next points" / "max this week" columns.** Only the ATP publishes
  in-tournament projections. A column of permanent blanks is worse than none.

Rank movement is derived by diffing our own consecutive stored snapshots, not
taken from any source.

| Env var | Default | Meaning |
|---|---|---|
| `RTT_WIKI_PAGE` | `2026_ATP_Finals` | Source article (season rollover is config) |
| `RTT_FETCH` | `1` | `0` = kill switch: zero outbound requests, serve stored |
| `RTT_FIXTURE` | *unset* | Parse a local file instead of fetching |
| `RTT_POLL_SECS` | `21600` | Refresh cadence (6h; the source is weekly) |
| `RTT_STALE_AFTER_SECS` | `691200` | Source-date age before the table reads stale (8 days; the source is weekly) |
| `RTT_CHECK_STALE_AFTER_SECS` | `86400` | Time since our last successful fetch before warning collection is broken |
| `RTT_CURATED` | `live/curated.toml` | Grand Slam champions + withdrawals |
| `RTT_DB` | `data/racetoturin.db` | SQLite file (`/data/…` in Docker) |
| `RTT_BIND` | `127.0.0.1:8080` | Listen address (`0.0.0.0:8080` in Docker) |
| `RTT_BASE_URL` | `https://racetotur.in` | Origin for canonical and `og:` URLs |

Routes: `/`, `/methodology`, `/robots.txt`, `/health/ready`, `/health/fresh`, `/static/app.css`.

Responses are gzipped (13.8 KB → 2.4 KB) and carry
`Cache-Control: max-age=120, stale-while-revalidate=600` plus an
`X-Snapshot-Version` header. Shared links (Reddit, Discord, Slack) get an `og:`
preview naming who holds seat 8 and who is first alternate.

## Test

```sh
cargo test                          # offline: fixture-driven
cargo test -- --ignored             # also hits Wikipedia once
```

Parser tests run against **real trimmed wikitext** in `fixtures/`, covering the
`Alternates` separator row, neutral-status players (`{{flagicon|}}` → no
country), disambiguated article titles, and the stated as-of date; plus rejection
paths (missing table, missing as-of, truncated table, duplicate players,
non-monotonic points, garbage input never panics). Also: qualifier ingestion with
source URLs, both qualification branches, derived movement across two snapshots,
storage dedup/versioning, and HTTP rendering.

## Design notes

- Identity is the Wikipedia article title; display names strip disambiguators.
- A candidate failing validation is rejected wholesale — it can never replace the
  stored snapshot, so a bad edit upstream cannot corrupt the served page.
- Official status is never inferred from points; it arrives with its citation.
- The database is written by the worker and never sits on the request path;
  handlers read an `ArcSwap` snapshot.
- Deployment is one Fly machine with one volume; see `deploy/RUNBOOK.md`.
