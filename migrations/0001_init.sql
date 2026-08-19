-- Append-only snapshot store. Snapshots are immutable once written;
-- corrections are new snapshots and the pointer moves atomically.

CREATE TABLE snapshots (
    id             INTEGER PRIMARY KEY AUTOINCREMENT, -- monotonic version
    created_at     TEXT    NOT NULL,                  -- RFC 3339 UTC
    source_as_of   TEXT    NOT NULL,                  -- RFC 3339 UTC
    source         TEXT    NOT NULL,
    parser_version TEXT    NOT NULL,
    content_hash   TEXT    NOT NULL                   -- sha256 of normalized content
);

CREATE TABLE snapshot_rows (
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    rank        INTEGER NOT NULL,
    player_code TEXT    NOT NULL,
    player_name TEXT    NOT NULL,
    country     TEXT    NOT NULL,
    race_points INTEGER NOT NULL CHECK (race_points >= 0),
    PRIMARY KEY (snapshot_id, rank)
);

-- Announced qualifications, always stored with the announcing source.
CREATE TABLE snapshot_qualifiers (
    snapshot_id  INTEGER NOT NULL REFERENCES snapshots(id),
    player_code  TEXT    NOT NULL,
    qualified_on TEXT    NOT NULL,                    -- YYYY-MM-DD
    source_url   TEXT    NOT NULL,
    PRIMARY KEY (snapshot_id, player_code)
);

-- Transactional pointer to the snapshot the web process serves.
CREATE TABLE current_snapshot (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id)
);
