use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Tiny, auditable manual input for facts no permitted source publishes in a
/// machine-readable form. Official qualification is *not* here — it is ingested
/// with its announcing source. Unknown keys are rejected so a typo cannot
/// silently drop a fact.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curated {
    pub season: u16,
    pub ruleset: String,
    /// One honest sentence about where this configuration's data comes from,
    /// shown in the page header.
    pub notice: String,
    /// Current-year Grand Slam champions, who can claim the eighth seat.
    #[serde(default)]
    pub slam_champions: Vec<CuratedPlayer>,
    #[serde(default)]
    pub withdrawals: Vec<CuratedPlayer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuratedPlayer {
    /// Wikipedia article title — the identity key, matching parsed rows.
    pub code: String,
    /// Provenance of the curated fact (URL or note); audit aid.
    #[serde(default)]
    pub source: Option<String>,
}

impl Curated {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read curated file {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid curated file {}", path.display()))
    }

    pub fn slam_champion_codes(&self) -> HashSet<&str> {
        codes(&self.slam_champions)
    }

    pub fn withdrawal_codes(&self) -> HashSet<&str> {
        codes(&self.withdrawals)
    }
}

fn codes(list: &[CuratedPlayer]) -> HashSet<&str> {
    list.iter().map(|p| p.code.as_str()).collect()
}
