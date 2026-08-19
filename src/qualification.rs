use std::collections::{HashMap, HashSet};

use crate::curated::Curated;
use crate::model::RaceRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatBasis {
    RaceRank,
    GrandSlamChampion,
}

/// Seat-8 flavor lives only in `Selection::eighth_basis`; the eighth
/// player's state is just `Eighth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provisional {
    TopSeven,
    Eighth,
    FirstAlternate,
    NotSelected,
    Withdrawn,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub eighth_basis: SeatBasis,
    pub eighth_code: Option<String>,
    pub alternate_code: Option<String>,
    pub by_code: HashMap<String, Provisional>,
    /// Signed points distance to the provisional qualification line.
    /// Empty when the Grand Slam champion provision is active: a single
    /// contiguous points line would be misleading then.
    pub margins: HashMap<String, i64>,
}

impl Selection {
    pub fn state(&self, code: &str) -> Provisional {
        self.by_code
            .get(code)
            .copied()
            .unwrap_or(Provisional::NotSelected)
    }

    pub fn margin(&self, code: &str) -> Option<i64> {
        self.margins.get(code).copied()
    }
}

/// Apply the Turin selection rule provisionally to the current table
/// ("if the season ended now"):
///
/// 1. the top seven players by race rank;
/// 2. up to two current-year Grand Slam champions ranked 8–20, by rank;
/// 3. everyone else in rank order, skipping duplicates.
///
/// The first eight unique players are the provisional selections; the
/// ninth is the first alternate. Officially announced status is stored
/// separately (curated) and never inferred from points. This function is
/// pure: no clock, network, or storage.
pub fn select(rows: &[RaceRow], curated: &Curated) -> Selection {
    let withdrawn = curated.withdrawal_codes();
    let champions = curated.slam_champion_codes();

    let active: Vec<&RaceRow> = rows
        .iter()
        .filter(|r| !withdrawn.contains(r.player_code.as_str()))
        .collect();

    let top_seven: Vec<&RaceRow> = active.iter().copied().take(7).collect();
    let top_seven_codes: HashSet<&str> =
        top_seven.iter().map(|r| r.player_code.as_str()).collect();

    let champs_8_20: Vec<&RaceRow> = active
        .iter()
        .copied()
        .filter(|r| (8..=20).contains(&r.rank))
        .filter(|r| champions.contains(r.player_code.as_str()))
        .filter(|r| !top_seven_codes.contains(r.player_code.as_str()))
        .take(2)
        .collect();

    let mut order: Vec<&RaceRow> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for row in top_seven
        .iter()
        .chain(champs_8_20.iter())
        .chain(active.iter().skip(7))
    {
        if seen.insert(row.player_code.as_str()) {
            order.push(row);
        }
    }

    let eighth = order.get(7).copied();
    let eighth_basis = match eighth {
        Some(r) if champs_8_20.iter().any(|c| c.player_code == r.player_code) => {
            SeatBasis::GrandSlamChampion
        }
        _ => SeatBasis::RaceRank,
    };
    let alternate = order.get(8).copied();

    let mut by_code: HashMap<String, Provisional> = HashMap::new();
    for p in &curated.withdrawals {
        by_code.insert(p.code.clone(), Provisional::Withdrawn);
    }
    for (i, row) in order.iter().enumerate() {
        let state = match i {
            0..=6 => Provisional::TopSeven,
            7 => Provisional::Eighth,
            8 => Provisional::FirstAlternate,
            _ => Provisional::NotSelected,
        };
        by_code.insert(row.player_code.clone(), state);
    }

    let mut margins: HashMap<String, i64> = HashMap::new();
    if eighth_basis == SeatBasis::RaceRank {
        if let (Some(eighth), Some(alternate)) = (eighth, alternate) {
            // Selected players show their cushion over the first player out;
            // chasers show their deficit to the current seat-8 holder.
            let cushion_line = alternate.race_points as i64;
            let deficit_line = eighth.race_points as i64;
            for (i, row) in order.iter().enumerate() {
                let margin = if i < 8 {
                    row.race_points as i64 - cushion_line
                } else {
                    row.race_points as i64 - deficit_line
                };
                margins.insert(row.player_code.clone(), margin);
            }
        }
    }

    Selection {
        eighth_basis,
        eighth_code: eighth.map(|r| r.player_code.clone()),
        alternate_code: alternate.map(|r| r.player_code.clone()),
        by_code,
        margins,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curated::CuratedPlayer;

    fn row(rank: u32, code: &str, points: u32) -> RaceRow {
        RaceRow {
            rank,
            movement: Some(0),
            player_code: code.to_string(),
            player_name: format!("Player {code}"),
            country: "USA".to_string(),
            race_points: points,
        }
    }

    fn table() -> Vec<RaceRow> {
        (1..=12)
            .map(|i| row(i, &format!("p{i:03}"), 10_000 - i * 500))
            .collect()
    }

    fn curated_with(champs: &[&str], withdrawals: &[&str]) -> Curated {
        let player = |code: &&str| CuratedPlayer {
            code: code.to_string(),
            source: None,
        };
        Curated {
            season: 2026,
            ruleset: "test".to_string(),
            notice: String::new(),
            slam_champions: champs.iter().map(player).collect(),
            withdrawals: withdrawals.iter().map(player).collect(),
        }
    }

    #[test]
    fn ordinary_case_seat_eight_by_rank() {
        let selection = select(&table(), &curated_with(&["p001"], &[]));
        assert_eq!(selection.eighth_basis, SeatBasis::RaceRank);
        assert_eq!(selection.eighth_code.as_deref(), Some("p008"));
        assert_eq!(selection.alternate_code.as_deref(), Some("p009"));
        assert_eq!(selection.state("p001"), Provisional::TopSeven);
        assert_eq!(selection.state("p008"), Provisional::Eighth);
        assert_eq!(selection.state("p009"), Provisional::FirstAlternate);
        assert_eq!(selection.state("p010"), Provisional::NotSelected);
    }

    #[test]
    fn ordinary_margins_have_correct_signs() {
        let selection = select(&table(), &curated_with(&[], &[]));
        // p008 has 6000, p009 has 5500: cushion +500 / deficit -500.
        assert_eq!(selection.margin("p008"), Some(500));
        assert_eq!(selection.margin("p009"), Some(-500));
        assert_eq!(selection.margin("p001"), Some(4000));
        assert_eq!(selection.margin("p010"), Some(-1000));
    }

    #[test]
    fn slam_champion_ranked_8_to_20_takes_seat_eight() {
        let selection = select(&table(), &curated_with(&["p010"], &[]));
        assert_eq!(selection.eighth_basis, SeatBasis::GrandSlamChampion);
        assert_eq!(selection.eighth_code.as_deref(), Some("p010"));
        assert_eq!(selection.state("p010"), Provisional::Eighth);
        // Rank 8 by points is now the first alternate, not selected.
        assert_eq!(selection.alternate_code.as_deref(), Some("p008"));
        assert_eq!(selection.state("p008"), Provisional::FirstAlternate);
        // No contiguous points line exists: margins are suppressed.
        assert!(selection.margins.is_empty());
    }

    #[test]
    fn champion_already_in_top_seven_triggers_no_provision() {
        let selection = select(&table(), &curated_with(&["p003"], &[]));
        assert_eq!(selection.eighth_basis, SeatBasis::RaceRank);
        assert_eq!(selection.eighth_code.as_deref(), Some("p008"));
    }

    #[test]
    fn champion_ranked_below_20_is_not_eligible() {
        let rows: Vec<RaceRow> = (1..=22)
            .map(|i| row(i, &format!("p{i:03}"), 10_000 - i * 100))
            .collect();
        let selection = select(&rows, &curated_with(&["p021"], &[]));
        assert_eq!(selection.eighth_basis, SeatBasis::RaceRank);
    }

    #[test]
    fn two_champions_first_takes_seat_second_is_alternate() {
        let selection = select(&table(), &curated_with(&["p012", "p010"], &[]));
        assert_eq!(selection.eighth_basis, SeatBasis::GrandSlamChampion);
        // Ordered by rank, not curated-file order.
        assert_eq!(selection.eighth_code.as_deref(), Some("p010"));
        assert_eq!(selection.alternate_code.as_deref(), Some("p012"));
        assert_eq!(selection.state("p012"), Provisional::FirstAlternate);
        assert_eq!(selection.state("p008"), Provisional::NotSelected);
    }

    #[test]
    fn withdrawal_shifts_seats_down() {
        let selection = select(&table(), &curated_with(&[], &["p003"]));
        assert_eq!(selection.state("p003"), Provisional::Withdrawn);
        assert_eq!(selection.state("p008"), Provisional::TopSeven);
        assert_eq!(selection.eighth_code.as_deref(), Some("p009"));
        assert_eq!(selection.alternate_code.as_deref(), Some("p010"));
        assert!(selection.margin("p003").is_none());
    }

    #[test]
    fn short_tables_do_not_panic() {
        let rows: Vec<RaceRow> = (1..=3).map(|i| row(i, &format!("p{i:03}"), 1000)).collect();
        let selection = select(&rows, &curated_with(&[], &[]));
        assert_eq!(selection.eighth_code, None);
        assert_eq!(selection.alternate_code, None);
        assert!(selection.margins.is_empty());
    }
}
