use serde::Serialize;
use time::{Date, OffsetDateTime};

/// Which block of the source's standings table a result was counted in. The
/// blocks are read from the header's `colspan` groups rather than hardcoded, so
/// a season carrying a different number of "best other" columns still parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Slot {
    GrandSlam,
    /// One of the mandatory ATP Masters 1000 events.
    Mandatory1000,
    /// A next-best result, filling out the total below the mandatory events.
    BestOther,
}

impl Slot {
    /// Stable code used as the stored value; parsing it back must round-trip.
    pub fn code(self) -> &'static str {
        match self {
            Slot::GrandSlam => "slam",
            Slot::Mandatory1000 => "masters",
            Slot::BestOther => "other",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "slam" => Slot::GrandSlam,
            "masters" => Slot::Mandatory1000,
            "other" => Slot::BestOther,
            _ => return None,
        })
    }

    /// Heading shown above the block in the expanded breakdown.
    pub fn title(self) -> &'static str {
        match self {
            Slot::GrandSlam => "Grand Slam",
            Slot::Mandatory1000 => "Masters 1000 (mandatory)",
            Slot::BestOther => "Best other",
        }
    }
}

/// What the source shows in a cell. "Did not play" and "not played yet" are
/// different facts about a mandatory event, so they never render alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Played {
    /// A result carrying points.
    Result,
    /// The source shows `A`: the player did not play a mandatory event that
    /// has already taken place.
    Absent,
    /// The source leaves the cell empty: the event has not happened yet.
    Pending,
}

impl Played {
    pub fn code(self) -> &'static str {
        match self {
            Played::Result => "result",
            Played::Absent => "absent",
            Played::Pending => "pending",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "result" => Played::Result,
            "absent" => Played::Absent,
            "pending" => Played::Pending,
            _ => return None,
        })
    }
}

/// One tournament in the ledger behind a player's race total.
///
/// Identity comes from the cell's own wikilink, never from its column: the
/// rulebook lets a next-best result be counted in a mandatory Masters 1000
/// slot, so the event a cell names and the slot it occupies can differ.
#[derive(Debug, Clone, Serialize)]
pub struct TournamentResult {
    pub slot: Slot,
    /// The mandatory event this column stands for ("US Open", "Miami"), as the
    /// source labels it. Empty for a "best other" column, which stands for no
    /// particular event.
    pub slot_label: String,
    pub played: Played,
    /// Wikipedia article title of the event actually played; empty unless
    /// `played` is `Result`.
    pub event_code: String,
    /// That title with the season and the "– Men's singles" suffix removed.
    pub event_name: String,
    /// Round reached: `W`, `F`, `SF`, `QF`, `R16`, … or `RR` for a round robin.
    pub round: String,
    pub points: u32,
    /// True when this result is counted in place of a mandatory Masters 1000
    /// event, which the source marks by italicising the cell. `slot_label` is
    /// then the event replaced, not the one played.
    pub substituted: bool,
}

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
    /// The per-tournament ledger behind `race_points`, in the source's own
    /// order. Empty when the breakdown did not reconcile against the stated
    /// total: a row always keeps its total, and never shows a wrong breakdown.
    pub results: Vec<TournamentResult>,
    /// Tournaments the source says the player has entered this season. Usually
    /// larger than the ledger, because only a player's best results count.
    pub tournaments_played: Option<u32>,
    pub titles: Option<u32>,
}

impl RaceRow {
    /// Points itemised by the ledger. Equal to `race_points` whenever a ledger
    /// survived reconciliation, which is the condition for keeping one.
    pub fn ledger_points(&self) -> u32 {
        self.results.iter().map(|r| r.points).sum()
    }

    /// Ledger entries that actually carry points, as opposed to the mandatory
    /// slots shown to record that the player did not play them.
    pub fn counting_results(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.played == Played::Result)
            .count()
    }
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
