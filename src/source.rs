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

use crate::model::{OfficialQualifier, Played, RaceRow, Slot, Snapshot, TournamentResult};

/// Bumped whenever extraction below changes; recorded on every snapshot.
pub const PARSER_VERSION: &str = "wikipedia-wikitext-2";

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

/// `[[Tommy Paul (tennis)|Tommy Paul]]` -> "Tommy Paul"; the *label*, not the
/// target. In the standings ledger the label is the round reached.
fn first_wikilink_label(cell: &str) -> Option<String> {
    let start = cell.find("[[")? + 2;
    let rest = &cell[start..];
    let end = rest.find("]]")?;
    let inner = &rest[..end];
    let label = strip_markup(inner.split('|').nth(1).unwrap_or(inner));
    (!label.is_empty()).then_some(label)
}

/// `colspan="4"` / `colspan=4` -> 4.
fn attr_number(s: &str, name: &str) -> Option<usize> {
    let i = s.find(name)? + name.len();
    let rest = s[i..].trim_start().strip_prefix('=')?.trim_start();
    let digits: String = rest
        .trim_start_matches('"')
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// The blocks of tournament columns, read from the first header row's
/// `colspan` groups: `! colspan="4" | [[…|Grand Slam]]` declares four Grand
/// Slam columns. Taking the widths from the header rather than hardcoding them
/// means a season with a different number of "best other" columns still parses.
fn header_groups(header: &str) -> Vec<(Slot, usize)> {
    let mut groups = Vec::new();
    for line in header.lines() {
        let Some(rest) = line.trim().strip_prefix('!') else {
            continue;
        };
        let Some(span) = attr_number(rest, "colspan") else {
            continue; // a rowspan column: Rank, Player, Total, Tourn, Titles
        };
        let label = strip_markup(rest).to_ascii_lowercase();
        let slot = if label.contains("grand slam") {
            Slot::GrandSlam
        } else if label.contains("masters 1000") {
            Slot::Mandatory1000
        } else if label.contains("best other") {
            Slot::BestOther
        } else {
            continue;
        };
        groups.push((slot, span));
    }
    groups
}

/// Drop a leading `bgcolor=… |` style prefix from a ledger cell. Unlike
/// `cell_value` this cannot be a plain `rfind('|')`: a ledger cell's body is a
/// wikilink, which carries its own pipe.
fn ledger_body(cell: &str) -> &str {
    let body_at = ["<!--", "[[", "''"]
        .iter()
        .filter_map(|m| cell.find(m))
        .min()
        .unwrap_or(cell.len());
    match cell[..body_at].rfind('|') {
        Some(i) => cell[i + 1..].trim(),
        None => cell.trim(),
    }
}

/// `<!--Miami--> …` — the source labels each mandatory column with the event
/// that column stands for. That is the *slot*, which for a substituted result
/// is not the event the player actually played. "Best other" columns are
/// labelled `<!---1--->` and stand for no particular event.
fn split_slot_label(body: &str) -> (String, &str) {
    let Some(rest) = body.strip_prefix("<!--") else {
        return (String::new(), body);
    };
    let Some(end) = rest.find("-->") else {
        return (String::new(), body);
    };
    let inner = rest[..end].trim().trim_matches('-').trim();
    let label = if inner.chars().any(char::is_alphabetic) {
        inner.to_string()
    } else {
        String::new()
    };
    (label, rest[end + 3..].trim())
}

/// Points are the number the source puts after the cell's `<br/>`. Read from
/// that marker rather than by scanning for digits, which would otherwise pick
/// up the season in the wikilink target.
fn cell_points(body: &str) -> Option<u32> {
    let i = body.rfind("<br")?;
    let rest = &body[i..];
    let j = rest.find('>')?;
    parse_int(&rest[j + 1..])
}

/// `2026 Australian Open – Men's singles` -> "Australian Open". The season is
/// already stated on the page and the draw suffix is noise in a ledger.
fn event_name(title: &str) -> String {
    let head = title
        .split(" – ")
        .next()
        .unwrap_or(title)
        .split(" - ")
        .next()
        .unwrap_or(title)
        .trim();
    let bytes = head.as_bytes();
    if bytes.len() > 5 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b' ' {
        head[5..].to_string()
    } else {
        head.to_string()
    }
}

fn parse_result_cell(cell: &str, slot: Slot) -> TournamentResult {
    let (slot_label, body) = split_slot_label(ledger_body(cell));
    match first_wikilink_target(body) {
        Some(title) => TournamentResult {
            slot,
            slot_label,
            played: Played::Result,
            event_name: event_name(&title),
            event_code: title,
            round: first_wikilink_label(body).unwrap_or_default(),
            points: cell_points(body).unwrap_or(0),
            // The source italicises a next-best result standing in for a
            // mandatory Masters 1000.
            substituted: body.starts_with("''"),
        },
        // No link: either an explicit `A` for an event the player skipped, or
        // an empty cell for one that has not been played yet.
        None => TournamentResult {
            slot,
            slot_label,
            played: if strip_markup(body).is_empty() {
                Played::Pending
            } else {
                Played::Absent
            },
            event_code: String::new(),
            event_name: String::new(),
            round: String::new(),
            points: 0,
            substituted: false,
        },
    }
}

/// The per-tournament ledger for one data row: the cells after the player cell,
/// walked group by group so each one knows which block it belongs to.
fn parse_ledger(cells: &[String], player_at: usize, groups: &[(Slot, usize)]) -> Vec<TournamentResult> {
    let mut out = Vec::new();
    let mut cursor = player_at + 1;
    for &(slot, span) in groups {
        for _ in 0..span {
            let Some(cell) = cells.get(cursor) else {
                return out; // row is short: reconciliation will reject it
            };
            cursor += 1;
            let result = parse_result_cell(cell, slot);
            // An unused "best other" column means the player has fewer than
            // six counting results, which is not a fact worth a row. An unused
            // *mandatory* column is, so it is kept.
            if slot == Slot::BestOther && result.played == Played::Pending {
                continue;
            }
            out.push(result);
        }
    }
    out
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

    // The first chunk is the table opener plus the first header row, which
    // declares how wide each block of tournament columns is.
    let groups = table
        .split("\n|-")
        .next()
        .map(header_groups)
        .unwrap_or_default();

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
        let player_at = cells
            .iter()
            .position(|c| c.contains("[[") && c.contains("align=\"left\""))
            .or_else(|| cells.iter().position(|c| c.contains("[[")))
            .ok_or_else(|| anyhow!("rank {rank}: no player cell"))?;
        let player_cell = &cells[player_at];
        let title = first_wikilink_target(player_cell)
            .ok_or_else(|| anyhow!("rank {rank}: no article link in player cell"))?;
        // Total, then tournaments entered, then titles — the row's three
        // `!`-prefixed cells, in the order the header declares them.
        let totals = header_cell_numbers(chunk);
        let race_points = *totals
            .first()
            .ok_or_else(|| anyhow!("rank {rank}: no total-points cell"))?;

        // A ledger is trusted only when it accounts for the stated total
        // exactly. One that does not reconcile is dropped: the row keeps its
        // total and simply offers no breakdown, which is always better than
        // offering a wrong one. `ingest` reports how often this happens, so
        // the feature cannot fail silently.
        let results = parse_ledger(&cells, player_at, &groups);
        let reconciles =
            !results.is_empty() && results.iter().map(|r| r.points).sum::<u32>() == race_points;

        rows.push(RaceRow {
            rank,
            movement: None, // derived later from stored snapshots
            player_name: display_name(&title),
            player_code: title,
            country: flag_code(player_cell),
            race_points,
            results: if reconciles { results } else { Vec::new() },
            tournaments_played: totals.get(1).copied(),
            titles: totals.get(2).copied(),
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

    // ---- per-tournament ledger ------------------------------------------

    /// The invariant the whole feature rests on, and the reason a breakdown can
    /// be trusted at all: the source's per-tournament cells account for the
    /// total it states, to the point.
    #[test]
    fn every_rows_ledger_accounts_for_its_stated_total() {
        let snap = fixture();
        for row in &snap.rows {
            assert!(
                !row.results.is_empty(),
                "{} has no ledger, so its cells did not reconcile",
                row.player_name
            );
            assert_eq!(
                row.ledger_points(),
                row.race_points,
                "{} ledger does not sum to its total",
                row.player_name
            );
        }
    }

    /// A cell is identified by its own wikilink, not by the column it sits in.
    /// Shelton's Miami and Madrid columns hold results from entirely different
    /// events — the rulebook's next-best substitution — and naming them by
    /// column would report tournaments he never played.
    #[test]
    fn a_substituted_result_names_the_event_played_and_the_slot_it_replaced() {
        let snap = fixture();
        let shelton = snap
            .rows
            .iter()
            .find(|r| r.player_code == "Ben Shelton")
            .expect("Shelton is in the fixture");

        let subs: Vec<&TournamentResult> =
            shelton.results.iter().filter(|r| r.substituted).collect();
        assert_eq!(subs.len(), 2, "Shelton has two substituted Masters slots");
        assert_eq!(subs[0].slot, Slot::Mandatory1000);
        assert_eq!(subs[0].slot_label, "Miami", "the slot the result replaces");
        assert_eq!(subs[0].event_name, "ASB Classic", "the event actually played");
        assert_eq!(subs[0].round, "QF");
        assert_eq!(subs[0].points, 50);

        // And an ordinary Masters cell is not marked as a substitution.
        let indian_wells = shelton
            .results
            .iter()
            .find(|r| r.slot_label == "Indian Wells")
            .expect("the Indian Wells column is present");
        assert!(!indian_wells.substituted);
        assert_eq!(indian_wells.event_name, "BNP Paribas Open");
    }

    /// Skipping an event that has happened and waiting for one that has not are
    /// different facts about a mandatory slot, so the parser keeps them apart.
    #[test]
    fn absent_and_not_yet_played_are_distinct() {
        let snap = fixture();
        let djokovic = snap
            .rows
            .iter()
            .find(|r| r.player_code == "Novak Djokovic")
            .expect("Djokovic is in the fixture");

        let slot = |label: &str| {
            djokovic
                .results
                .iter()
                .find(|r| r.slot_label == label)
                .unwrap_or_else(|| panic!("no {label} slot"))
        };
        // "A" in the source: the event happened, he did not play it.
        assert_eq!(slot("Miami").played, Played::Absent);
        assert_eq!(slot("Miami").points, 0);
        // Empty in the source: the event has not been played yet.
        assert_eq!(slot("Shanghai").played, Played::Pending);
        assert_eq!(slot("US Open").played, Played::Pending);
        assert_eq!(slot("Rome").played, Played::Result);

        // Unused "best other" columns say only that a player has fewer than six
        // counting results, which is not worth a ledger entry.
        assert!(
            !djokovic.results.iter().any(|r| r.slot == Slot::BestOther),
            "Djokovic's six empty best-other columns must not become entries"
        );
    }

    /// Column blocks come from the header's `colspan` groups, so the four Grand
    /// Slam and eight mandatory Masters columns are found structurally rather
    /// than counted off from a hardcoded offset.
    #[test]
    fn ledger_blocks_are_read_from_the_header_colspans() {
        let snap = fixture();
        let count = |row: &RaceRow, slot: Slot| {
            row.results.iter().filter(|r| r.slot == slot).count()
        };
        for row in &snap.rows {
            assert_eq!(count(row, Slot::GrandSlam), 4, "{}", row.player_name);
            assert_eq!(count(row, Slot::Mandatory1000), 8, "{}", row.player_name);
            assert!(count(row, Slot::BestOther) <= 6, "{}", row.player_name);
        }
    }

    /// Only a player's best results count, so the ledger is routinely shorter
    /// than the season played. The page must be able to say so.
    #[test]
    fn reads_the_tournaments_played_and_titles_counts() {
        let snap = fixture();
        let cobolli = snap
            .rows
            .iter()
            .find(|r| r.player_code == "Flavio Cobolli")
            .expect("Cobolli is in the fixture");
        assert_eq!(cobolli.tournaments_played, Some(19));
        assert_eq!(cobolli.titles, Some(1));
        // Fifteen of those nineteen carry points toward the total.
        assert_eq!(cobolli.counting_results(), 15);
        assert!(cobolli.counting_results() < cobolli.tournaments_played.unwrap() as usize);
    }

    /// A breakdown that does not add up is dropped on its own: the row keeps the
    /// total the source states, and simply offers no breakdown. Standings must
    /// never be held hostage to a secondary feature.
    #[test]
    fn an_unreconciled_ledger_is_dropped_without_losing_the_row() {
        // Inflate one of Sinner's cells, leaving his stated total alone.
        let text = FIXTURE.replace(
            "[[2026 Qatar ExxonMobil Open – Singles|QF]]<br/>100",
            "[[2026 Qatar ExxonMobil Open – Singles|QF]]<br/>110",
        );
        let snap = parse(&text, "test", 2026).expect("the snapshot itself still publishes");

        let sinner = &snap.rows[0];
        assert_eq!(sinner.player_code, "Jannik Sinner");
        assert_eq!(sinner.race_points, 7950, "the stated total is untouched");
        assert!(
            sinner.results.is_empty(),
            "a breakdown that does not sum to the total must not be shown"
        );
        // Every other row is unaffected: the check is per row, not per article.
        assert!(snap.rows[1..].iter().all(|r| !r.results.is_empty()));
    }

    #[test]
    fn event_names_drop_the_season_and_the_draw_suffix() {
        assert_eq!(event_name("2026 Australian Open – Men's singles"), "Australian Open");
        assert_eq!(event_name("2026 Monte-Carlo Masters – Singles"), "Monte-Carlo Masters");
        assert_eq!(event_name("2026 United Cup"), "United Cup");
        // Nothing to strip, and a hyphenated name must survive intact.
        assert_eq!(event_name("Monte-Carlo Masters"), "Monte-Carlo Masters");
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
