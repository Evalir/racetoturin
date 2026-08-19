-- Append-only snapshot store. Snapshots are immutable once written;
-- corrections are new snapshots and the pointer moves atomically.

CREATE TABLE snapshots (
    id             INTEGER PRIMARY KEY AUTOINCREMENT, -- monotonic version
    created_at     TEXT    NOT NULL,                  -- RFC 3339 UTC
    source_as_of   TEXT    NOT NULL,                  -- RFC 3339 UTC
    source         TEXT    NOT NULL,
    parser_version TEXT    NOT NULL,
    content_hash   TEXT    NOT NULL,                  -- sha256 of normalized rows
    row_count      INTEGER NOT NULL
);

CREATE TABLE snapshot_rows (
    snapshot_id        INTEGER NOT NULL REFERENCES snapshots(id),
    rank               INTEGER NOT NULL,
    movement           INTEGER,
    player_code        TEXT    NOT NULL,
    player_name        TEXT    NOT NULL,
    country            TEXT    NOT NULL,
    live_points        INTEGER NOT NULL CHECK (live_points >= 0),
    event_name         TEXT,
    event_round        TEXT,
    next_points        INTEGER,
    max_this_week      INTEGER,
    unavailable_reason TEXT,
    PRIMARY KEY (snapshot_id, rank)
);

-- Transactional pointer to the snapshot the web process serves.
CREATE TABLE current_snapshot (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id)
);
