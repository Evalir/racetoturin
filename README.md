# racetotur.in — local MVP

A fast, single-purpose site answering: **who would qualify for the ATP Finals in
Turin if selection happened now?** This is the fully local build: the real
pipeline (HTML parser → snapshot validation → versioned qualification policy →
SSR web UI), fed by a checked-in synthetic fixture instead of a live fetch.

**The process makes zero network requests.** The fixture in `fixtures/race.html`
stands in for the public ATP Live Race to Turin page; every name, point total,
result, and Grand Slam title in it is fictional sample data.

## Run it

```sh
cargo run -- serve
# → http://127.0.0.1:8080/
```

The default fixture world has a Grand Slam champion ranked 9, so the
**champion provision** decides seat 8 (highlighted row, no contiguous 8/9
cutoff). To see the **ordinary cutoff** branch (8/9 line plus points margins)
with the same table:

```sh
RTT_CURATED=config/curated_ordinary.toml cargo run -- serve
```

Debug the parser output as JSON:

```sh
cargo run -- parse fixtures/race.html
```

### Configuration (env vars)

| Variable | Default | Meaning |
|---|---|---|
| `RTT_FIXTURE` | `fixtures/race.html` | Race page HTML to parse at startup. |
| `RTT_CURATED` | `config/curated.toml` | Curated official facts (slam titles, official qualifiers, withdrawals). |
| `RTT_BIND` | `127.0.0.1:8080` | Listen address. |
| `RTT_STALE_AFTER_SECS` | `900` | Source age beyond which the page is labelled stale. |

The fixture's source timestamp is fixed (`2026-08-19T18:42:07Z`), so the
freshness strip will honestly flip to **stale — last known good** once that
timestamp is older than the threshold. Edit the `<time id="source-as-of">`
value in the fixture to see the fresh state.

## Test it

```sh
cargo test
```

Covers: parser fixtures (identities from profile links, idle players,
accented names), candidate rejection (missing heading, duplicate
ranks/identities, unparseable or non-monotonic points, maintenance-sized
tables, never panicking on garbage), both qualification branches, the
two-champion and withdrawal cases, margin signs, and HTTP/template behavior.

## Routes

| Route | Purpose |
|---|---|
| `GET /` | Full SSR homepage (summary, freshness, race table). |
| `GET /race?limit=N` | Same page, N clamped to 8–50. |
| `GET /methodology` | Where every number comes from. |
| `GET /health/live`, `GET /health/ready` | Liveness/readiness. |
| `GET /static/app.css` | The only asset. No JavaScript is served at all. |

## Layout

```
src/model.rs          domain types (rows, snapshots); points are integers, codes are identity
src/parser.rs         fixture/ATP-shaped HTML → validated candidate snapshot
src/qualification.rs  pure Turin selection policy (top 7 + slam-champion provision + alternate)
src/curated.rs        curated official facts loader (TOML)
src/web.rs            Axum routes + view models (templates stay dumb)
templates/, static/   Askama SSR + one small CSS file
fixtures/race.html    synthetic ATP-shaped race page (the local "source")
config/*.toml         curated files for the two selection branches
```

## What the local MVP intentionally leaves out (vs the PRD)

- **No network fetcher.** `parser::parse` takes any race-page HTML string, so
  a polite `reqwest` fetcher can be added in front of it without touching the
  rest of the pipeline. Scraping the real ATP page is a legal/operational
  decision documented in the PRD — not something a local build should do.
- **No PostgreSQL.** One immutable in-memory snapshot loaded at startup
  replaces the snapshot store; publication history comes later.
- **No JavaScript.** The PRD's enhancement script only exists to poll for new
  snapshots; a static local snapshot has none.
- Mobile hides the Next/Max/Margin columns via CSS instead of the PRD's
  per-player disclosure widget.
