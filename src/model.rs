use serde::Serialize;
use time::{Date, OffsetDateTime};

// Rows derive Serialize because the store's content hash is computed over
// their canonical JSON.
#[derive(Debug, Clone, Serialize)]
pub struct RaceRow {
    /// Displayed race rank; unique and contiguous after validation.
    pub rank: u32,
    /// Places gained since our previous stored snapshot. Derived, not taken
    /// from a source, so it is None until two snapshots exist.
    #[serde(skip)]
    pub movement: Option<i32>,
    /// Wikipedia article title — the canonical identity key.
    pub player_code: String,
    /// Article title with any disambiguator stripped; display only.
    pub player_name: String,
    /// ISO 3166-1 alpha-3, or empty when the source shows no flag
    /// (neutral-status players).
    pub country: String,
    pub race_points: u32,
}

/// An announced qualification, always carried with the source that announced
/// it. Never inferred from points.
#[derive(Debug, Clone)]
pub struct OfficialQualifier {
    pub player_code: String,
    /// Date as displayed by the source; the year is the season.
    pub qualified_on: Date,
    pub source_url: String,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The date the source itself states the standings are current to.
    pub source_as_of: OffsetDateTime,
    /// When we fetched and published it.
    pub generated_at: OffsetDateTime,
    /// URL or file the rows were parsed from.
    pub source: String,
    pub parser_version: String,
    pub rows: Vec<RaceRow>,
    pub qualifiers: Vec<OfficialQualifier>,
}
