use std::{path::Path, str::FromStr, time::Duration};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow},
    Row, SqlitePool,
};
use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime};

use crate::model::{OfficialQualifier, Played, RaceRow, Slot, Snapshot, TournamentResult};

/// Pass as the db path to run against a throwaway in-memory database.
pub const MEMORY: &str = ":memory:";

pub struct Store {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy)]
pub struct PublishOutcome {
    pub version: i64,
    /// False when the content hash matched the current snapshot and no new
    /// version was written.
    pub created: bool,
}

fn fmt(t: OffsetDateTime) -> Result<String> {
    t.format(&Rfc3339).context("cannot format timestamp")
}

fn parse_ts(s: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).with_context(|| format!("bad stored timestamp {s:?}"))
}

/// Hash of the normalized content only: a re-fetch whose visible facts are
/// identical creates no new snapshot, even with a fresher page timestamp.
/// Qualifiers are included so an announcement alone publishes a new version.
fn content_hash(snapshot: &Snapshot) -> Result<String> {
    let rows = serde_json::to_string(&snapshot.rows).context("cannot serialize rows")?;
    let mut hasher = Sha256::new();
    hasher.update(rows.as_bytes());
    for q in &snapshot.qualifiers {
        hasher.update(
            format!("|{}@{}={}", q.player_code, q.qualified_on, q.source_url).as_bytes(),
        );
    }
    Ok(format!("{:x}", hasher.finalize()))
}

impl Store {
    pub async fn open(db_path: &str) -> Result<Self> {
        let options = if db_path == MEMORY {
            SqliteConnectOptions::from_str("sqlite::memory:")?
        } else {
            if let Some(parent) = Path::new(db_path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("cannot create {}", parent.display()))?;
                }
            }
            SqliteConnectOptions::new()
                .filename(db_path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
        }
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

        // One connection: SQLite is single-writer, the database is never on the
        // request path, and this keeps :memory: databases coherent.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .with_context(|| format!("cannot open database {db_path}"))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("migrations failed")?;
        Ok(Self { pool })
    }

    /// Insert a validated candidate as a new immutable snapshot and move the
    /// current pointer, unless its content matches what is already current.
    pub async fn publish_if_changed(&self, snapshot: &Snapshot) -> Result<PublishOutcome> {
        let hash = content_hash(snapshot)?;
        let mut tx = self.pool.begin().await?;

        let current: Option<(i64, String)> = sqlx::query_as(
            "SELECT s.id, s.content_hash
             FROM snapshots s JOIN current_snapshot c ON c.snapshot_id = s.id",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((version, existing)) = current {
            if existing == hash {
                return Ok(PublishOutcome {
                    version,
                    created: false,
                });
            }
        }

        let version: i64 = sqlx::query_scalar(
            "INSERT INTO snapshots
               (created_at, source_as_of, source, parser_version, content_hash)
             VALUES (?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(fmt(snapshot.generated_at)?)
        .bind(fmt(snapshot.source_as_of)?)
        .bind(&snapshot.source)
        .bind(&snapshot.parser_version)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await?;

        for row in &snapshot.rows {
            sqlx::query(
                "INSERT INTO snapshot_rows
                   (snapshot_id, rank, player_code, player_name, country, race_points,
                    tournaments_played, titles)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(version)
            .bind(row.rank as i64)
            .bind(&row.player_code)
            .bind(&row.player_name)
            .bind(&row.country)
            .bind(row.race_points as i64)
            .bind(row.tournaments_played.map(i64::from))
            .bind(row.titles.map(i64::from))
            .execute(&mut *tx)
            .await?;

            for (ordinal, result) in row.results.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO snapshot_results
                       (snapshot_id, player_code, ordinal, slot, slot_label, played,
                        event_code, event_name, round, points, substituted)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(version)
                .bind(&row.player_code)
                .bind(ordinal as i64)
                .bind(result.slot.code())
                .bind(&result.slot_label)
                .bind(result.played.code())
                .bind(&result.event_code)
                .bind(&result.event_name)
                .bind(&result.round)
                .bind(result.points as i64)
                .bind(result.substituted as i64)
                .execute(&mut *tx)
                .await?;
            }
        }

        for q in &snapshot.qualifiers {
            sqlx::query(
                "INSERT INTO snapshot_qualifiers
                   (snapshot_id, player_code, qualified_on, source_url)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(version)
            .bind(&q.player_code)
            .bind(q.qualified_on.to_string())
            .bind(&q.source_url)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO current_snapshot (id, snapshot_id) VALUES (1, ?)
             ON CONFLICT(id) DO UPDATE SET snapshot_id = excluded.snapshot_id",
        )
        .bind(version)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(PublishOutcome {
            version,
            created: true,
        })
    }

    /// The snapshot the current pointer designates, or None on a fresh
    /// database. Row movement is filled in from the preceding snapshot.
    pub async fn load_current(&self) -> Result<Option<(i64, Snapshot)>> {
        let Some(meta) = sqlx::query(
            "SELECT s.id, s.created_at, s.source_as_of, s.source, s.parser_version
             FROM snapshots s JOIN current_snapshot c ON c.snapshot_id = s.id",
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let version: i64 = meta.get("id");

        let mut rows = self.rows_of(version).await?;
        let previous = self.previous_ranks(version).await?;
        for row in &mut rows {
            if let Some(prev) = previous.get(row.player_code.as_str()) {
                row.movement = Some(*prev - row.rank as i32);
            }
        }

        let qualifiers = sqlx::query(
            "SELECT player_code, qualified_on, source_url
             FROM snapshot_qualifiers WHERE snapshot_id = ? ORDER BY qualified_on",
        )
        .bind(version)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| {
            let raw: String = r.get("qualified_on");
            Ok(OfficialQualifier {
                player_code: r.get("player_code"),
                qualified_on: Date::parse(
                    &raw,
                    &time::macros::format_description!("[year]-[month]-[day]"),
                )
                .with_context(|| format!("bad stored date {raw:?}"))?,
                source_url: r.get("source_url"),
            })
        })
        .collect::<Result<Vec<_>>>()?;

        Ok(Some((
            version,
            Snapshot {
                source_as_of: parse_ts(meta.get("source_as_of"))?,
                generated_at: parse_ts(meta.get("created_at"))?,
                source: meta.get("source"),
                parser_version: meta.get("parser_version"),
                rows,
                qualifiers,
            },
        )))
    }

    async fn rows_of(&self, version: i64) -> Result<Vec<RaceRow>> {
        let rows = sqlx::query(
            "SELECT rank, player_code, player_name, country, race_points,
                    tournaments_played, titles
             FROM snapshot_rows WHERE snapshot_id = ? ORDER BY rank",
        )
        .bind(version)
        .fetch_all(&self.pool)
        .await?;
        let mut ledgers = self.results_of(version).await?;
        Ok(rows
            .iter()
            .map(|r| {
                let mut row = row_from_db(r);
                row.results = ledgers.remove(row.player_code.as_str()).unwrap_or_default();
                row
            })
            .collect())
    }

    /// Every stored ledger for a snapshot, keyed by player and kept in the
    /// source's column order.
    async fn results_of(
        &self,
        version: i64,
    ) -> Result<std::collections::HashMap<String, Vec<TournamentResult>>> {
        let rows = sqlx::query(
            "SELECT player_code, slot, slot_label, played, event_code, event_name,
                    round, points, substituted
             FROM snapshot_results WHERE snapshot_id = ? ORDER BY player_code, ordinal",
        )
        .bind(version)
        .fetch_all(&self.pool)
        .await?;

        let mut out: std::collections::HashMap<String, Vec<TournamentResult>> = Default::default();
        for r in &rows {
            let slot_code: String = r.get("slot");
            let played_code: String = r.get("played");
            // An unreadable code means a ledger written by a different build;
            // skipping the entry breaks reconciliation, so the row shows no
            // breakdown rather than a partial one.
            let (Some(slot), Some(played)) =
                (Slot::from_code(&slot_code), Played::from_code(&played_code))
            else {
                continue;
            };
            out.entry(r.get("player_code")).or_default().push(TournamentResult {
                slot,
                slot_label: r.get("slot_label"),
                played,
                event_code: r.get("event_code"),
                event_name: r.get("event_name"),
                round: r.get("round"),
                points: r.get::<i64, _>("points").max(0) as u32,
                substituted: r.get::<i64, _>("substituted") != 0,
            });
        }
        Ok(out)
    }

    /// Ranks in the newest snapshot older than `version`, for derived movement.
    async fn previous_ranks(
        &self,
        version: i64,
    ) -> Result<std::collections::HashMap<String, i32>> {
        let previous: Option<i64> =
            sqlx::query_scalar("SELECT MAX(id) FROM snapshots WHERE id < ?")
                .bind(version)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let Some(previous) = previous else {
            return Ok(Default::default());
        };
        let rows = sqlx::query("SELECT rank, player_code FROM snapshot_rows WHERE snapshot_id = ?")
            .bind(previous)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get::<String, _>("player_code"), r.get::<i64, _>("rank") as i32))
            .collect())
    }
}

fn row_from_db(r: &SqliteRow) -> RaceRow {
    let as_u32 = |v: i64| -> u32 { v.max(0) as u32 };
    RaceRow {
        rank: as_u32(r.get::<i64, _>("rank")),
        movement: None,
        player_code: r.get("player_code"),
        player_name: r.get("player_name"),
        country: r.get("country"),
        race_points: as_u32(r.get::<i64, _>("race_points")),
        // Filled in by `rows_of`, which reads every ledger in one query.
        results: Vec::new(),
        tournaments_played: r.get::<Option<i64>, _>("tournaments_played").map(as_u32),
        titles: r.get::<Option<i64>, _>("titles").map(as_u32),
    }
}
