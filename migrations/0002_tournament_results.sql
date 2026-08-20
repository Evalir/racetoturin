-- The per-tournament ledger behind each row's race_points, and the two count
-- columns the source states next to the total. Snapshots stay immutable: these
-- are written once, with the snapshot they belong to.

ALTER TABLE snapshot_rows ADD COLUMN tournaments_played INTEGER;
ALTER TABLE snapshot_rows ADD COLUMN titles INTEGER;

CREATE TABLE snapshot_results (
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    player_code TEXT    NOT NULL,
    -- Position in the source's own column order, which is how it is displayed.
    ordinal     INTEGER NOT NULL,
    slot        TEXT    NOT NULL,          -- 'slam' | 'masters' | 'other'
    -- The mandatory event this column stands for; empty for 'other'.
    slot_label  TEXT    NOT NULL,
    played      TEXT    NOT NULL,          -- 'result' | 'absent' | 'pending'
    -- Wikipedia article title of the event played; empty unless played='result'.
    event_code  TEXT    NOT NULL,
    event_name  TEXT    NOT NULL,
    round       TEXT    NOT NULL,
    points      INTEGER NOT NULL CHECK (points >= 0),
    -- A next-best result counted in place of a mandatory Masters 1000.
    substituted INTEGER NOT NULL CHECK (substituted IN (0, 1)),
    PRIMARY KEY (snapshot_id, player_code, ordinal)
);
