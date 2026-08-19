//! Outbound links from a player's name to their ATP profile.
//!
//! This project never *fetches* `atptour.com` — it refuses automated clients,
//! and we don't disguise ourselves to get around that. Linking to it is a
//! different act, and a reader reasonably expects a name to lead to the
//! profile.
//!
//! The ids come from Wikidata property P536 ("Association of Tennis
//! Professionals player ID"): machine-readable, CC0, publicly queryable, and
//! keyed by the same English Wikipedia article titles this app already uses as
//! player identity. So the map is derived, not hand-typed. It is resolved
//! offline and compiled in, which means serving a page needs no extra request
//! and no file on the deploy volume.
//!
//! Regenerate with `cargo run --bin refresh-atp-ids` (see the module docs on
//! that binary).

use std::collections::BTreeMap;

/// Generated, checked in, and compiled into the binary.
const IDS_TOML: &str = include_str!("../live/atp_ids.toml");

/// P536's formatter URL. The name segment is a throwaway `-`: the id alone
/// identifies the player, so a title never has to be transliterated into a
/// slug we would have to keep correct.
fn url_for(id: &str) -> String {
    format!("https://www.atptour.com/en/players/-/{id}/overview")
}

/// Wikidata's own format constraint on P536 (`[A-Z][0-9A-Z]{3}`). Checked
/// rather than trusted: a malformed id would render a link that cannot be a
/// profile, and a wrong link is worse than no link.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let first = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    first && id.len() == 4 && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Article title -> ATP id. Parsed once; the file is a compiled-in asset, so a
/// parse failure is a build-time authoring error and is covered by a test.
fn ids() -> &'static BTreeMap<String, String> {
    static IDS: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        let parsed: BTreeMap<String, String> =
            toml::from_str(IDS_TOML).expect("live/atp_ids.toml is malformed");
        parsed.into_iter().filter(|(_, id)| valid_id(id)).collect()
    })
}

/// The player's ATP profile, or `None` when we have no id for them — a new
/// entrant links nowhere rather than linking somewhere wrong.
pub fn profile_url(player_code: &str) -> Option<String> {
    ids().get(player_code).map(|id| url_for(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_in_map_parses_and_every_id_is_well_formed() {
        let parsed: BTreeMap<String, String> =
            toml::from_str(IDS_TOML).expect("live/atp_ids.toml must parse");
        assert!(!parsed.is_empty(), "the map should not be empty");
        for (title, id) in &parsed {
            assert!(valid_id(id), "{title} has malformed ATP id {id:?}");
        }
        // Nothing was dropped by the validity filter above.
        assert_eq!(parsed.len(), ids().len());
    }

    #[test]
    fn ids_are_checked_before_they_become_urls() {
        assert!(valid_id("S0AG"));
        assert!(valid_id("A0E2"));
        assert!(!valid_id("s0ag"), "lowercase is not the P536 format");
        assert!(!valid_id("S0A"), "too short");
        assert!(!valid_id("S0AGX"), "too long");
        assert!(!valid_id("0SAG"), "must start with a letter");
        assert!(!valid_id("S0A/"), "no path separators");
        assert!(!valid_id(""));
    }

    #[test]
    fn a_known_player_resolves_and_an_unknown_one_does_not() {
        assert_eq!(
            profile_url("Jannik Sinner").as_deref(),
            Some("https://www.atptour.com/en/players/-/S0AG/overview")
        );
        assert_eq!(profile_url("Not A Real Player"), None);
    }
}
