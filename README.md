# racetotur.in

Who would qualify for the ATP Finals in Turin if selection happened now?

The app fetches the standings itself: parse Wikipedia's `<season> ATP Finals`
article → validate the candidate → publish to SQLite (append-only, content-hash
dedup, atomic pointer) → apply the versioned Turin qualification rule → serve
from memory as server-rendered HTML. Every row expands to the tournaments
behind its points. No JavaScript is served — the expansion is a native
`<details>`. It refreshes every six hours and never claims to be fresher than
its source.

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
Player names still *link* to their ATP profile — linking is not fetching. Those
ids come from [Wikidata P536](https://www.wikidata.org/wiki/Property:P536) (CC0,
keyed by the same article titles), resolved offline into `live/atp_ids.toml` and
compiled in, so serving a page makes no extra request:

```sh
cargo run --bin refresh-atp-ids          # top up the map from the live article
```
tennisexplorer was evaluated and rejected — its terms (§2.11) explicitly forbid
scraping and aggregating. Wikipedia is also simply the better source: it carries
a per-tournament points ledger and a citation for every announced qualification.

### The per-player breakdown

The article records each player's points as a grid — four Grand Slam columns,
eight mandatory Masters 1000, up to six "best other" — and each row on the page
expands to show it. Cells are identified by their **own wikilink, never by the
column they sit in**: the rulebook lets a player swap up to three mandatory
Masters results for other next-best ones, so a "Miami" column can legitimately
hold an ASB Classic quarter-final. Reading the column would report tournaments
the player never entered; reading the link names the event played and the slot
it filled. Column *blocks* come from the header's `colspan` groups, so a season
with a different number of "best other" columns still parses.

A breakdown is shown only when its points sum to exactly the total the article
states for that row — verified against all 20 rows of the live article. One that
does not reconcile is dropped **on its own**: the row keeps its published total
and offers no breakdown. That check is per row, not per article, so a cosmetic
change upstream cannot withdraw the whole table for the sake of a secondary
feature; the number of rows affected is logged instead.

Two things it does not claim: it is not a full schedule (only a player's best
results count, so the summary states both counts — "15 of 19 tournaments
count"), and a mandatory event skipped (`A`, zero points) is never rendered like
one not yet held (a dash, no points).

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

Responses are gzipped (102.6 KB → 6.2 KB; the breakdowns are highly repetitive
markup, so they cost about 3.8 KB on the wire) and carry
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

For the breakdown specifically: every row's ledger reconciles against its stated
total (asserted for the fixture *and*, under `--ignored`, for the live article);
a substituted Masters result names the event played and the slot it replaced;
`A` and not-yet-played stay distinct; unused "best other" columns produce no
entries; an inflated cell drops that one row's breakdown while leaving the row
and every other breakdown intact; and the rendered page ships no `<script>`.

## Design notes

- Identity is the Wikipedia article title; display names strip disambiguators.
- A candidate failing validation is rejected wholesale — it can never replace the
  stored snapshot, so a bad edit upstream cannot corrupt the served page.
- The exception is the points breakdown, which is validated per row: it is
  secondary to the standings, so it fails alone rather than taking the table with
  it. Every total on the page is one a reader can add up and check.
- Official status is never inferred from points; it arrives with its citation.
- The database is written by the worker and never sits on the request path;
  handlers read an `ArcSwap` snapshot.
- Deployment is one Fly machine with one volume; see `deploy/RUNBOOK.md`.
