//! Parser for the English Wikipedia "<season> ATP Finals" article wikitext.
//!
//! Wikitext is parsed rather than rendered HTML because the markup is far more
//! stable across edits. Everything is located by structural markers — the
//! `!`-prefixed total cell, the `align="left"` player cell — rather than by
//! column index, so an editor adding a tournament column cannot silently shift
//! the numbers.
//!
//! Source content is available under CC BY-SA 4.0; see /methodology.

use anyhow::{anyhow, bail, Context, Result};
use time::{Date, Month, OffsetDateTime, Time, UtcOffset};

use crate::model::{OfficialQualifier, RaceRow, Snapshot};

/// Bumped whenever extraction below changes; recorded on every snapshot.
pub const PARSER_VERSION: &str = "wikipedia-wikitext-1";

/// Fewer rows than this is a truncated or restructured article, never a
/// publishable table. The qualification rule reaches rank 20, so a healthy
/// article carries at least this many.
const MIN_ROWS: usize = 10;

fn strip_markup(s: &str) -> String {
    let mut out = s.to_string();
    // HTML comments first: they can contain anything.
    while let (Some(a), Some(b)) = (out.find("<!--"), out.find("-->")) {
        if a < b {
            out.replace_range(a..b + 3, "");
        } else {
            break;
        }
    }
    out = out
        .replace("'''", "")
        .replace("''", "")
        .replace("<br/>", " ")
        .replace("<br />", " ");
    // Remaining tags such as <sup>†</sup>.
    let mut clean = String::with_capacity(out.len());
    let mut depth = 0usize;
    for c in out.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => clean.push(c),
            _ => {}
        }
    }
    clean.trim().to_string()
}

/// `[[Tommy Paul (tennis)|Tommy Paul]]` or `[[Jannik Sinner]]` -> article title.
fn first_wikilink_target(cell: &str) -> Option<String> {
    let start = cell.find("[[")? + 2;
    let rest = &cell[start..];
    let end = rest.find("]]")?;
    let inner = &rest[..end];
    let target = inner.split('|').next()?.trim();
    if target.is_empty() {
        None
    } else {
        Some(target.to_string())
    }
}

/// Article titles disambiguate with a trailing parenthetical, which is not
/// part of the person's name.
fn display_name(title: &str) -> String {
    match title.find(" (") {
        Some(i) if title.ends_with(')') => title[..i].to_string(),
        _ => title.to_string(),
    }
}

/// `{{flagicon|ITA}}` -> "ITA"; `{{flagicon|}}` (neutral status) -> "".
fn flag_code(cell: &str) -> String {
    let Some(i) = cell.find("{{flagicon|") else {
        return String::new();
    };
    let rest = &cell[i + "{{flagicon|".len()..];
    let Some(end) = rest.find("}}") else {
        return String::new();
    };
    let code = rest[..end].split('|').next().unwrap_or("").trim();
    if code.len() == 3 && code.chars().all(|c| c.is_ascii_alphabetic()) {
        code.to_uppercase()
    } else {
        String::new()
    }
}

fn parse_int(s: &str) -> Option<u32> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// The standings table's total-points column is the first `!`-prefixed numeric
/// cell in a row (tournaments played and titles follow it).
fn header_cell_numbers(row: &str) -> Vec<u32> {
    row.lines()
        .filter_map(|l| l.trim().strip_prefix('!'))
        .filter_map(|v| {
            let v = v.trim();
            let ok = !v.is_empty()
                && v.chars().all(|c| c.is_ascii_digit() || c == ',')
                && v.chars().any(|c| c.is_ascii_digit());
            ok.then(|| parse_int(v)).flatten()
        })
        .collect()
}

/// Cells within a wikitext row: split on newline-`|` and inline `||`.
fn row_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    for line in row.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("| ").or_else(|| line.strip_prefix('|')) {
            if rest.starts_with('-') || rest.starts_with('}') {
                continue;
            }
            for part in rest.split("||") {
                cells.push(part.trim().to_string());
            }
        }
    }
    cells
}

/// Drop a leading `bgcolor=… |` / `align="left" |` style prefix from a cell.
fn cell_value(cell: &str) -> &str {
    match cell.rfind('|') {
        Some(i) => cell[i + 1..].trim(),
        None => cell.trim(),
    }
}

fn table_at<'a>(wikitext: &'a str, open: &str, from: usize) -> Option<&'a str> {
    let start = wikitext[from..].find(open)? + from;
    let end = wikitext[start..].find("\n|}")? + start;
    Some(&wikitext[start..end])
}

fn parse_standings(wikitext: &str) -> Result<Vec<RaceRow>> {
    let table = table_at(
        wikitext,
        "{|class=\"wikitable nowrap\" style=font-size:90%;text-align:center",
        0,
    )
    .ok_or_else(|| anyhow!("standings table not found; article structure changed"))?;

    let mut rows = Vec::new();
    // Skip the table opener and the second header row.
    for chunk in table.split("\n|-").skip(2) {
        if chunk.contains("Alternates") && chunk.contains("colspan") {
            continue; // section separator, not a player
        }
        let cells = row_cells(chunk);
        if cells.len() < 3 {
            continue;
        }
        let Some(rank) = parse_int(&strip_markup(cell_value(&cells[0]))) else {
            continue;
        };
        let player_cell = cells
            .iter()
            .find(|c| c.contains("[[") && c.contains("align=\"left\""))
            .or_else(|| cells.iter().find(|c| c.contains("[[")))
            .ok_or_else(|| anyhow!("rank {rank}: no player cell"))?;
        let title = first_wikilink_target(player_cell)
            .ok_or_else(|| anyhow!("rank {rank}: no article link in player cell"))?;
        let race_points = *header_cell_numbers(chunk)
            .first()
            .ok_or_else(|| anyhow!("rank {rank}: no total-points cell"))?;

        rows.push(RaceRow {
            rank,
            movement: None, // derived later from stored snapshots
            player_name: display_name(&title),
            player_code: title,
            country: flag_code(player_cell),
            race_points,
        });
    }
    Ok(rows)
}

fn month_from_name(name: &str) -> Option<Month> {
    Some(match name.to_ascii_lowercase().as_str() {
        "january" => Month::January,
        "february" => Month::February,
        "march" => Month::March,
        "april" => Month::April,
        "may" => Month::May,
        "june" => Month::June,
        "july" => Month::July,
        "august" => Month::August,
        "september" => Month::September,
        "october" => Month::October,
        "november" => Month::November,
        "december" => Month::December,
        _ => return None,
    })
}

/// `{{dts|10 July}}` in the qualifier table; the season supplies the year.
fn parse_dts(cell: &str, season: i32) -> Option<Date> {
    let i = cell.find("{{dts|")? + "{{dts|".len();
    let rest = &cell[i..];
    let end = rest.find("}}")?;
    let mut parts = rest[..end].split_whitespace();
    let day: u8 = parts.next()?.parse().ok()?;
    let month = month_from_name(parts.next()?)?;
    Date::from_calendar_date(season, month, day).ok()
}

fn first_ref_url(cell: &str) -> Option<String> {
    let i = cell.find("url=")? + 4;
    let rest = &cell[i..];
    let end = rest
        .find(|c: char| c == '|' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    let url = rest[..end].trim();
    url.starts_with("http").then(|| url.to_string())
}

/// The singles qualifier table is the first sortable wikitable after the
/// `=== Singles ===` heading; the doubles table follows and must be ignored.
fn parse_qualifiers(wikitext: &str, season: i32) -> Vec<OfficialQualifier> {
    let singles = wikitext.find("=== Singles ===").unwrap_or(0);
    let Some(table) = table_at(wikitext, "{|class=\"sortable wikitable nowrap\"", singles) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for chunk in table.split("\n|-").skip(1) {
        let Some(title) = first_wikilink_target(chunk) else {
            continue;
        };
        // Skip the article links inside citation templates.
        if !chunk.contains("align=left") {
            continue;
        }
        let Some(qualified_on) = parse_dts(chunk, season) else {
            continue;
        };
        let Some(source_url) = first_ref_url(chunk) else {
            continue;
        };
        out.push(OfficialQualifier {
            player_code: title,
            qualified_on,
            source_url,
        });
    }
    out
}

/// `''Updated {{as of|2026|8|16|lc=yes}}.''` immediately above the table.
fn parse_as_of(wikitext: &str) -> Result<OffsetDateTime> {
    let i = wikitext
        .find("{{as of|")
        .ok_or_else(|| anyhow!("no '{{as of}}' date found; cannot state source freshness"))?
        + "{{as of|".len();
    let rest = &wikitext[i..];
    let end = rest
        .find("}}")
        .ok_or_else(|| anyhow!("unterminated 'as of' template"))?;
    let mut parts = rest[..end].split('|');
    let year: i32 = parts
        .next()
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow!("'as of' has no year"))?;
    let month: u8 = parts
        .next()
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow!("'as of' has no month"))?;
    let day: u8 = parts
        .next()
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow!("'as of' has no day"))?;
    let month = Month::try_from(month).map_err(|_| anyhow!("'as of' month {month} invalid"))?;
    let date = Date::from_calendar_date(year, month, day)
        .with_context(|| format!("'as of' date {year}-{month}-{day} invalid"))?;
    Ok(OffsetDateTime::new_in_offset(
        date,
        Time::MIDNIGHT,
        UtcOffset::UTC,
    ))
}

/// Parse an article into a validated snapshot. A candidate failing any check is
/// rejected wholesale so it can never replace a stored snapshot.
pub fn parse(wikitext: &str, source_label: &str, season: i32) -> Result<Snapshot> {
    let snapshot = Snapshot {
        source_as_of: parse_as_of(wikitext)?,
        generated_at: OffsetDateTime::now_utc(),
        source: source_label.to_string(),
        parser_version: PARSER_VERSION.to_string(),
        rows: parse_standings(wikitext)?,
        qualifiers: parse_qualifiers(wikitext, season),
    };
    validate(&snapshot)?;
    Ok(snapshot)
}

fn validate(snapshot: &Snapshot) -> Result<()> {
    let rows = &snapshot.rows;
    let mut problems: Vec<String> = Vec::new();

    if rows.len() < MIN_ROWS {
        problems.push(format!("implausible row count {}", rows.len()));
    }
    for (i, row) in rows.iter().enumerate() {
        let expected = (i + 1) as u32;
        if row.rank != expected {
            problems.push(format!(
                "rank not contiguous at position {}: expected {expected}, got {}",
                i + 1,
                row.rank
            ));
            break;
        }
    }
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        if !seen.insert(row.player_code.as_str()) {
            problems.push(format!("duplicate player {}", row.player_code));
        }
    }
    for pair in rows.windows(2) {
        if pair[1].race_points > pair[0].race_points {
            problems.push(format!(
                "points not non-increasing at rank {}: {} > {}",
                pair[1].rank, pair[1].race_points, pair[0].race_points
            ));
        }
    }
    for q in &snapshot.qualifiers {
        if !seen.contains(q.player_code.as_str()) {
            problems.push(format!(
                "qualifier {} is absent from the standings table",
                q.player_code
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        bail!("candidate snapshot rejected: {}", problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/race.wikitext");

    fn fixture() -> Snapshot {
        parse(FIXTURE, "fixtures/race.wikitext", 2026).expect("fixture must parse")
    }

    #[test]
    fn parses_players_and_skips_the_alternates_separator() {
        let snap = fixture();
        assert_eq!(snap.rows.len(), 12);
        assert!(snap.rows.iter().all(|r| !r.player_name.contains("Alternates")));
        assert_eq!(snap.parser_version, PARSER_VERSION);
    }

    #[test]
    fn identity_is_the_article_title_and_points_come_from_the_total_cell() {
        let snap = fixture();
        assert_eq!(snap.rows[0].player_code, "Jannik Sinner");
        assert_eq!(snap.rows[0].race_points, 7950);
        assert_eq!(snap.rows[0].country, "ITA");
        assert_eq!(snap.rows[1].race_points, 6650);
    }

    #[test]
    fn disambiguated_titles_keep_the_code_but_clean_the_display_name() {
        let snap = fixture();
        let paul = snap
            .rows
            .iter()
            .find(|r| r.player_code.starts_with("Tommy Paul"));
        if let Some(paul) = paul {
            assert_eq!(paul.player_code, "Tommy Paul (tennis)");
            assert_eq!(paul.player_name, "Tommy Paul");
        }
        assert_eq!(display_name("Tommy Paul (tennis)"), "Tommy Paul");
        assert_eq!(display_name("Jannik Sinner"), "Jannik Sinner");
    }

    #[test]
    fn neutral_status_player_has_empty_country_not_an_error() {
        let snap = fixture();
        let med = snap
            .rows
            .iter()
            .find(|r| r.player_code == "Daniil Medvedev")
            .expect("Medvedev is in the fixture");
        assert_eq!(med.country, "");
        assert_eq!(med.race_points, 2580);
    }

    #[test]
    fn reads_the_articles_stated_as_of_date() {
        let snap = fixture();
        assert_eq!(snap.source_as_of.date().to_string(), "2026-08-16");
    }

    #[test]
    fn ingests_official_qualifiers_with_their_sources() {
        let snap = fixture();
        assert_eq!(snap.qualifiers.len(), 2);
        let sinner = &snap.qualifiers[0];
        assert_eq!(sinner.player_code, "Jannik Sinner");
        assert_eq!(sinner.qualified_on.to_string(), "2026-07-10");
        assert!(sinner.source_url.starts_with("https://"));
        let zverev = &snap.qualifiers[1];
        assert_eq!(zverev.player_code, "Alexander Zverev");
        assert_eq!(zverev.qualified_on.to_string(), "2026-08-06");
    }

    #[test]
    fn rejects_article_without_the_standings_table() {
        let html = FIXTURE.replace("{|class=\"wikitable nowrap\"", "{|class=\"other\"");
        let err = parse(&html, "test", 2026).unwrap_err().to_string();
        assert!(err.contains("standings table"), "unexpected: {err}");
    }

    #[test]
    fn rejects_missing_as_of_date() {
        let text = FIXTURE.replace("{{as of|", "{{whenever|");
        let err = parse(&text, "test", 2026).unwrap_err().to_string();
        assert!(err.contains("as of"), "unexpected: {err}");
    }

    #[test]
    fn rejects_non_monotonic_points() {
        let text = FIXTURE.replace("! 3,650", "! 9,650");
        let err = parse(&text, "test", 2026).unwrap_err().to_string();
        assert!(err.contains("non-increasing"), "unexpected: {err}");
    }

    #[test]
    fn rejects_duplicate_players() {
        let text = FIXTURE.replace("[[Carlos Alcaraz]]", "[[Jannik Sinner]]");
        let err = parse(&text, "test", 2026).unwrap_err().to_string();
        assert!(err.contains("duplicate player"), "unexpected: {err}");
    }

    #[test]
    fn rejects_truncated_table() {
        let cut = FIXTURE.find("[[Rafael Jódar]]").unwrap();
        let text = format!("{}\n|}}\n", &FIXTURE[..cut]);
        let err = parse(&text, "test", 2026).unwrap_err().to_string();
        assert!(err.contains("row count"), "unexpected: {err}");
    }

    #[test]
    fn never_panics_on_arbitrary_input() {
        for garbage in [
            "",
            "{{as of|2026|8|16}}",
            "{|class=\"wikitable nowrap\" style=font-size:90%;text-align:center\n|}",
            "{{as of|",
            "{{as of|x|y|z}}{|class=\"wikitable nowrap\" style=font-size:90%;text-align:center\n|-\n|-\n| 1\n|}",
            "\u{0}\u{1}[[[[|||}}{{",
        ] {
            let _ = parse(garbage, "test", 2026); // must be Err, never a panic
        }
    }
}
