# racetotur.in — local MVP

Who would qualify for the ATP Finals in Turin if selection happened now?

The pipeline runs once at startup: parse `live/race.html` into a validated
candidate → publish it to SQLite (append-only snapshots, content-hash dedup,
atomic current pointer) → apply the versioned Turin qualification rule → serve
the table from memory as server-rendered HTML. The process makes **zero network
requests** and serves **no JavaScript**.

## Run

```sh
cargo run                # → http://127.0.0.1:8080/
docker compose up        # same thing in a container, SQLite in a volume
```

The prebuilt image is at `ghcr.io/evalir/racetoturin`.

## The data, honestly

`live/race.html` holds real Race to Turin standings — a small, attributed,
normalized extract as displayed by perfect-tennis.com (community source), with
the check time in the file. `live/curated.toml` holds the season's official
facts (Grand Slam champions with source links, official qualifiers,
withdrawals) — these are never inferred from points.

There is no automated fetcher yet because atptour.com (and most aggregators)
refuse non-browser clients, and this project does not disguise itself to get
past that. Refreshing is manual: update the rows and the `source-as-of` time in
`live/race.html`, restart, and the store dedupes unchanged content or publishes
a new immutable version. The page labels itself stale once the data is older
than `RTT_STALE_AFTER_SECS` (default one day, matching the manual cadence).

| Env var | Default |
|---|---|
| `RTT_FIXTURE` | `live/race.html` |
| `RTT_CURATED` | `live/curated.toml` |
| `RTT_DB` | `data/racetoturin.db` (`/data/…` in Docker) |
| `RTT_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in Docker) |
| `RTT_STALE_AFTER_SECS` | `86400` |

Routes: `/` (the product), `/methodology`, `/health/ready`, `/static/app.css`.

## Test

```sh
cargo test
```

The synthetic pages under `fixtures/` exist only for tests: they exercise both
qualification branches deterministically (Grand Slam champion provision and
ordinary 8/9 cutoff), ties, withdrawals, idle players, accents, and every
parser rejection path. A dedicated test also guards that the committed `live/`
data parses, validates, and renders.

## Design notes

- Identity is the ATP player code from profile links; names are display data.
- A candidate that fails validation is rejected wholesale — it can never
  replace the last verified snapshot.
- Unavailable values (next/max/movement not shown by the source) render as
  unavailable with a reason, never as estimates.
- The database is written once at startup and never sits on the request path.
