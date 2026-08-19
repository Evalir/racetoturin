# Deploying to Fly

One machine, one volume, one region. SQLite cannot be shared, so this app is
deliberately not scaled out.

Running cost is about **$2.20/month**: $2.02 for an always-on shared-cpu-1x/256MB
machine plus $0.15 for a 1 GB volume.

## Why traffic cannot blow up the bill

Fly has **no spending caps or budget alerts**, so the protection here is
structural rather than a setting:

- **Compute is fixed price.** A machine bills per second it is *running*, by size
  — not by CPU consumed. A traffic spike costs nothing extra in compute; at worst
  the shared CPU throttles.
- **Egress is the only variable, and it is tiny.** The page is 2.4 KB gzipped.
  At $0.02/GB: 100k views ≈ $0.005, 1M views ≈ $0.05, 10M views ≈ $0.50.
- **Horizontal scale-out cannot happen by accident.** One SQLite volume means one
  machine; a second machine has nothing to mount. `auto_stop_machines = "off"`
  and `min_machines_running = 1` pin it there. Never run `fly scale count`.
- **No per-request upstream work.** Pages render from an in-memory snapshot;
  Wikipedia is polled once every 6h regardless of traffic, so viral traffic
  cannot turn into a thundering herd against the source.

The realistic failure under a front-page spike is therefore *slowness*, not cost —
and one shared CPU serving a 2.4 KB page from memory has a lot of headroom before
that bites. No CDN is needed; don't add one pre-emptively.

If something does surprise you, Fly's billing docs say they will discuss a refund
for unexpected traffic or accidental resources.

## First deploy

```sh
brew install flyctl
fly auth login                                       # interactive
fly apps create racetoturin                          # not `fly launch`
fly volumes create data --size 1 --region ams --yes   # 1 GB is generous
fly deploy --ha=false                                # uses the committed fly.toml
fly open
```

Avoid `fly launch` and the web "deploy from GitHub" flow for the first deploy:
launch runs a wizard that rewrites `fly.toml`, and the web flow has no volume
step at all, so `[[mounts]]` would land on an ephemeral disk. `fly apps create`
plus `fly deploy` uses the config exactly as committed.

`--ha=false` matters: the default would create two machines, and two writers on
one SQLite file is not a thing. Verify you have exactly one:

```sh
fly status          # expect 1 machine, state "started"
fly logs            # expect "snapshot vN (new) · 20 rows · 2 qualifiers"
```

## Custom domain

Fly issues and renews the certificate itself; nothing else is required.

```sh
fly certs add racetotur.in
fly ips list                       # note the shared IPv4 and the IPv6
```

At the registrar — an apex domain needs both records:

| Type | Name | Value |
|---|---|---|
| A | `@` | the IPv4 from `fly ips list` |
| AAAA | `@` | the IPv6 from `fly ips list` |

```sh
fly certs show racetotur.in        # wait for "Certificate issued"
curl -sI https://racetotur.in/     # expect 200
```

Use the **shared** IPv4 every app gets for free — it routes HTTPS by SNI, which is
all this site serves. Do not run `fly ips allocate-v4`: a dedicated IPv4 is $2/mo
and would nearly double the bill for no benefit here.

That is the whole deployment. No CDN, no proxy, no extra services. Responses carry
`Cache-Control: max-age=120, stale-while-revalidate=600`, so browsers cache
correctly on their own. If the site ever does get slow under real load, adding a
CDN later is a DNS change with no code edit — but measure first, and add it only
*after* the Fly certificate is issued, since an ACME challenge behind a proxy is
the classic way issuance fails.

## Continuous deploys (optional, after the first deploy works)

Once the app exists with its volume, connecting GitHub is safe and gives you
deploy-on-push:

```sh
fly tokens create deploy -x 999999h    # add as GH secret FLY_API_TOKEN
```

Then a workflow step running `flyctl deploy --remote-only`. Do this *after* the
CLI deploy, never instead of it.

## How the database lives here

A Fly Machine's root filesystem is ephemeral: `fly deploy` replaces the machine
wholesale, so everything except the mounted volume is rebuilt from the image. The
volume is **local NVMe on one host**, not network storage, which is why it is fast
and why it attaches to exactly one machine. `/data` therefore holds the only
durable state — `racetoturin.db` plus its `-wal` and `-shm` files (WAL mode).

Two consequences worth expecting:

- **Deploys blip for a few seconds.** One volume means no rolling deploy: the old
  machine must release it before the new one mounts it. Acceptable here; if it ever
  is not, the fix is a stateless read replica, not a bigger database.
- **One volume is one failure domain.** If its host has problems the site is down
  until Fly recovers or you restore a snapshot onto a new volume. Tolerable because
  the database is reconstructible — see Backups.

## Routine operations

```sh
fly deploy                         # ship a new image
fly logs                           # watch refresh cycles
fly ssh console                    # poke around; DB is at /data
fly machine restart <id>           # forces a fresh ingest at boot
```

**Kill switch.** To stop all outbound collection immediately while continuing to
serve the last stored snapshot:

```sh
fly secrets set RTT_FETCH=0        # triggers a restart; page keeps serving
fly secrets unset RTT_FETCH        # resume collection
```

**Season rollover.** No code change:

```sh
fly secrets set RTT_WIKI_PAGE=2027_ATP_Finals
```

Also update `live/curated.toml` (season, ruleset, that year's Grand Slam
champions) and redeploy.

## Backups

Deliberately minimal, because the database is **reconstructible**: it holds a
cache of public Wikipedia data plus what is already committed in
`live/curated.toml`. Losing the volume costs only snapshot history — the derived
rank-movement arrows — not the product itself, which repopulates on next boot.

Fly takes daily volume snapshots automatically (5-day retention), which comfortably
covers this. To restore:

```sh
fly volumes snapshots list <volume-id>
fly volumes create data --snapshot-id <id> --size 1 --region ams
```

Litestream would be over-engineering here. Add it only if snapshot history
becomes something people rely on.

## Failure modes to expect

| Symptom | Cause | Action |
|---|---|---|
| Crashloop on very first boot | Empty DB *and* the fetch failed, so there is nothing to serve | Check `fly logs` for the upstream error; it self-heals once Wikipedia is reachable |
| Page shows "stale — last known good" | Refresh failing, or the article's as-of date stopped moving | `fly logs` shows the rejection reason; the served snapshot is intact |
| "candidate snapshot rejected" in logs | Wikipedia's table was restructured or vandalised | Working as intended — the last good snapshot keeps serving. Fix the parser at leisure |
| Health check failing | No snapshot available at all | `fly ssh console`, confirm `/data` is mounted |

## Monitoring

`GET /health/fresh` is the alarm hook. It returns **503** when our own collection
has stopped succeeding (default: no successful fetch for 24h, while we poll every
6h), and **200** otherwise. Body carries the raw numbers:

```
fresh
checked_age_seconds=3577
source_age_seconds=302862
snapshot_version=1
```

Any uptime checker pointed at it will alarm on the status code alone — no
log parsing:

```sh
curl -fsS https://racetotur.in/health/fresh || notify-me
```

**Two clocks, deliberately separate.** `source_age` is how old Wikipedia says its
standings are; since it publishes weekly, several days old is *normal* and only
`RTT_STALE_AFTER_SECS` (8 days) worth of age means the source itself has stalled.
`checked_age` is how long since *we* last fetched successfully, governed by
`RTT_CHECK_STALE_AFTER_SECS` (1 day) — that is the one that means something is
broken on our side. Do not set the source threshold to a day; the page would
declare itself stale every day while being perfectly current.

The homepage shows each condition separately: "Stale: showing the last verified
snapshot" for an old source, and "Last successful update N ago — automatic
collection may be failing" for a broken fetcher.
