use std::{path::Path, str::FromStr, time::Duration};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow},
    Row, SqlitePool,
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::model::{EventState, RaceRow, Snapshot};

/// Pass as the db path to run against a throwaway in-memory database.
pub const MEMORY: &str = ":memory:";

pub struct Store {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy)]
pub struct PublishOutcome {
    pub version: i64,
    /// False when the content hash matched the current snapshot and no
    /// new version was written.
    pub created: bool,
}

fn fmt(t: OffsetDateTime) -> Result<String> {
    t.format(&Rfc3339).context("cannot format timestamp")
}

fn parse_ts(s: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).with_context(|| format!("bad stored timestamp {s:?}"))
}

/// Hash of the normalized rows only: a re-fetch whose visible content is
/// identical (even with a fresher page timestamp) creates no new snapshot.
fn content_hash(rows: &[RaceRow]) -> Result<String> {
    let canonical = serde_json::to_string(rows).context("cannot serialize rows for hashing")?;
    Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
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

        // One connection: SQLite is single-writer, the database is never on
        // the request path, and this keeps :memory: databases coherent.
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

    pub async fn close(self) {
        self.pool.close().await;
    }

    /// Insert a validated candidate as a new immutable snapshot and move the
    /// current pointer, unless its content hash matches what is already
    /// current — then the stored snapshot stands and nothing is written.
    pub async fn publish_if_changed(&self, snapshot: &Snapshot) -> Result<PublishOutcome> {
        let hash = content_hash(&snapshot.rows)?;
        let mut tx = self.pool.begin().await?;

        let current: Option<(i64, String)> = sqlx::query_as(
            "SELECT s.id, s.content_hash
             FROM snapshots s JOIN current_snapshot c ON c.snapshot_id = s.id",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((version, existing_hash)) = current {
            if existing_hash == hash {
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
                   (snapshot_id, rank, movement, player_code, player_name, country,
                    live_points, event_name, event_round, next_points, max_this_week,
                    unavailable_reason)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(version)
            .bind(row.rank as i64)
            .bind(row.movement)
            .bind(&row.player_code)
            .bind(&row.player_name)
            .bind(&row.country)
            .bind(row.live_points as i64)
            .bind(row.event.as_ref().map(|e| e.name.as_str()))
            .bind(row.event.as_ref().map(|e| e.round.as_str()))
            .bind(row.next_points.map(|v| v as i64))
            .bind(row.max_this_week.map(|v| v as i64))
            .bind(row.unavailable_reason.as_deref())
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
    /// database.
    pub async fn load_current(&self) -> Result<Option<(i64, Snapshot)>> {
        let meta = sqlx::query(
            "SELECT s.id, s.created_at, s.source_as_of, s.source, s.parser_version
             FROM snapshots s JOIN current_snapshot c ON c.snapshot_id = s.id",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(meta) = meta else {
            return Ok(None);
        };
        let version: i64 = meta.get("id");

        let rows = sqlx::query(
            "SELECT rank, movement, player_code, player_name, country, live_points,
                    event_name, event_round, next_points, max_this_week, unavailable_reason
             FROM snapshot_rows WHERE snapshot_id = ? ORDER BY rank",
        )
        .bind(version)
        .fetch_all(&self.pool)
        .await?;

        let rows: Vec<RaceRow> = rows.iter().map(row_from_db).collect();
        let snapshot = Snapshot {
            source_as_of: parse_ts(meta.get("source_as_of"))?,
            generated_at: parse_ts(meta.get("created_at"))?,
            source: meta.get("source"),
            parser_version: meta.get("parser_version"),
            rows,
        };
        Ok(Some((version, snapshot)))
    }

}

fn row_from_db(r: &SqliteRow) -> RaceRow {
    let event_name: Option<String> = r.get("event_name");
    let event_round: Option<String> = r.get("event_round");
    let as_u32 = |v: i64| -> u32 { v.max(0) as u32 };
    RaceRow {
        rank: as_u32(r.get::<i64, _>("rank")),
        movement: r.get("movement"),
        player_code: r.get("player_code"),
        player_name: r.get("player_name"),
        country: r.get("country"),
        live_points: as_u32(r.get::<i64, _>("live_points")),
        event: event_name.map(|name| EventState {
            name,
            round: event_round.unwrap_or_default(),
        }),
        next_points: r.get::<Option<i64>, _>("next_points").map(as_u32),
        max_this_week: r.get::<Option<i64>, _>("max_this_week").map(as_u32),
        unavailable_reason: r.get("unavailable_reason"),
    }
}
