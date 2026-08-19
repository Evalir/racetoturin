use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Tiny, auditable manual input: officially announced facts that must
/// never be inferred from points (qualifiers, slam titles, withdrawals).
#[derive(Debug, Clone, Deserialize)]
pub struct Curated {
    pub season: u16,
    pub ruleset: String,
    #[serde(default)]
    pub slam_champions: Vec<CuratedPlayer>,
    #[serde(default)]
    pub official_qualifiers: Vec<CuratedPlayer>,
    #[serde(default)]
    pub withdrawals: Vec<CuratedPlayer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CuratedPlayer {
    /// ATP player code — the identity key, matching parsed rows.
    pub code: String,
    /// Display name for provenance/review only.
    pub name: String,
    #[serde(default)]
    pub source: Option<String>,
}

impl Curated {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read curated file {}", path.display()))?;
        let curated: Curated = toml::from_str(&text)
            .with_context(|| format!("invalid curated file {}", path.display()))?;
        Ok(curated)
    }

    pub fn slam_champion_codes(&self) -> HashSet<&str> {
        self.slam_champions.iter().map(|p| p.code.as_str()).collect()
    }

    pub fn official_qualifier_codes(&self) -> HashSet<&str> {
        self.official_qualifiers
            .iter()
            .map(|p| p.code.as_str())
            .collect()
    }

    pub fn withdrawal_codes(&self) -> HashSet<&str> {
        self.withdrawals.iter().map(|p| p.code.as_str()).collect()
    }
}
