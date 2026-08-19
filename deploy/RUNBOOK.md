# Deploying to Fly

One machine, one volume, one region. SQLite cannot be shared, so this app is
deliberately not scaled out.

## First deploy

```sh
brew install flyctl
fly auth login                                  # interactive
fly launch --no-deploy --copy-config --ha=false # reuses the committed fly.toml
fly volumes create data --size 1 --region ams   # 1 GB is generous
fly deploy
fly open
```

`--ha=false` matters: the default would create two machines, and two writers on
one SQLite file is not a thing. Verify you have exactly one:

```sh
fly status          # expect 1 machine, state "started"
fly logs            # expect "snapshot vN (new) · 20 rows · 2 qualifiers"
```

## Custom domain

```sh
fly certs add racetotur.in
fly ips list                       # point A/AAAA records at these
fly certs show racetotur.in        # wait for "Certificate issued"
```

Optionally put Cloudflare in front (proxied DNS, cache everything except
`/health/*`). Origin response is already fast; the CDN mostly absorbs bursts.

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

There is no alerting yet. The cheapest useful thing is an external cron hitting
`/` and asserting the source date is recent, or `fly logs` piped somewhere. Worth
adding before the standings start deciding who flies to Turin.
