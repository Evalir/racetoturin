use serde::Serialize;
use time::OffsetDateTime;

/// Bumped whenever row extraction changes; recorded on every snapshot.
pub const PARSER_VERSION: &str = "fixture-html-1";

// Rows derive Serialize because the store's content hash is computed over
// their canonical JSON.
#[derive(Debug, Clone, Serialize)]
pub struct EventState {
    pub name: String,
    /// Attained/next round exactly as displayed by the source ("QF", "SF", ...).
    pub round: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RaceRow {
    /// Displayed live race rank; unique and contiguous after validation.
    pub rank: u32,
    /// Rank movement vs the weekly baseline, as displayed by the source.
    pub movement: Option<i32>,
    /// Stable ATP player code extracted from the profile link; names are
    /// display data, never identity keys.
    pub player_code: String,
    pub player_name: String,
    /// ISO 3166-1 alpha-3 code as displayed.
    pub country: String,
    pub live_points: u32,
    /// Present only while the player is active in an event this week.
    pub event: Option<EventState>,
    /// Source-displayed total after one more win.
    pub next_points: Option<u32>,
    /// Source-displayed maximum reachable in active events this week.
    pub max_this_week: Option<u32>,
    /// Why next/max are unavailable, when they are.
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub source_as_of: OffsetDateTime,
    pub generated_at: OffsetDateTime,
    /// Where the rows came from — in the local build, the fixture path.
    pub source: String,
    pub parser_version: String,
    pub rows: Vec<RaceRow>,
}
