# racetotur.in — local MVP

Who would qualify for the ATP Finals in Turin if selection happened now?

The pipeline runs once at startup: parse an ATP-shaped HTML fixture into a
validated candidate → publish it to SQLite (append-only snapshots, content-hash
dedup, atomic current pointer) → apply the Turin qualification rule → serve the
table from memory as server-rendered HTML. **Zero network requests, no
JavaScript.** Every name, point total, and slam title in the fixture is
fictional sample data, and the site says so.

## Run

```sh
cargo run                # → http://127.0.0.1:8080/
```

or with Docker:

```sh
docker compose up        # builds the image, persists SQLite in a volume
```

The prebuilt image is published by CI: `docker run -p 8080:8080 ghcr.io/evalir/racetoturin`
(mount a volume at `/data` to keep the database).

The default fixture world has a Grand Slam champion at race rank 9, so the
champion provision decides seat 8 (highlighted row, no 8/9 cutoff). Same table,
ordinary branch (8/9 line + points margins):

```sh
RTT_CURATED=config/curated_ordinary.toml cargo run
```

| Env var | Default | Meaning |
|---|---|---|
| `RTT_FIXTURE` | `fixtures/race.html` | Race page HTML parsed at startup. |
| `RTT_CURATED` | `config/curated.toml` | Curated official facts (slam titles, official qualifiers, withdrawals). |
| `RTT_DB` | `data/racetoturin.db` (`/data/…` in Docker) | SQLite database. |
| `RTT_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in Docker) | Listen address. |
| `RTT_STALE_AFTER_SECS` | `900` | Source age beyond which the page is labelled stale. |

Routes: `/` (the product), `/methodology`, `/health/ready`, `/static/app.css`.

## Test

```sh
cargo test
```

Parser fixtures and rejection paths (bad heading, duplicate ranks/identities,
unparseable or non-monotonic points, maintenance-sized tables, garbage input),
both qualification branches plus the two-champion and withdrawal cases, storage
dedup/versioning/roundtrip, and HTTP rendering of both branches.

## Design notes

- Identity is the ATP player code from profile links; names are display data.
- A candidate that fails validation is rejected wholesale — it can never
  replace the last verified snapshot.
- Official status only ever comes from the curated file, never from points.
- The database is written once at startup and never sits on the request path.
  For deployment, back up `/data` (e.g. Litestream) and put any CDN in front.
- A live fetcher would slot in front of `parser::parse(html, source)`; going
  live against the real ATP page is the legal/operational decision documented
  in the product doc, deliberately not part of this build.
