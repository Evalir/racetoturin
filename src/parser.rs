use anyhow::{anyhow, bail, Context, Result};
use scraper::{ElementRef, Html, Selector};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::model::{EventState, RaceRow, Snapshot, PARSER_VERSION};

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector must be valid")
}

fn cell_text(row: ElementRef, selector: &Selector) -> Option<String> {
    row.select(selector)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
}

/// "8,760" / "8760" -> 8760. Rejects anything else.
fn parse_points(text: &str) -> Result<u32> {
    let cleaned: String = text.chars().filter(|c| *c != ',' && *c != '\u{202f}').collect();
    cleaned
        .parse::<u32>()
        .with_context(|| format!("cannot parse points value {text:?}"))
}

/// Empty / dash cells mean "not displayed by the source".
fn parse_optional_points(text: Option<String>) -> Result<Option<u32>> {
    match text.as_deref().map(str::trim) {
        None | Some("") | Some("-") | Some("–") | Some("—") => Ok(None),
        Some(v) => parse_points(v).map(Some),
    }
}

fn parse_movement(text: Option<String>) -> Option<i32> {
    let t = text?;
    let t = t.trim();
    if t.is_empty() || t == "-" || t == "–" || t == "—" {
        return None;
    }
    t.parse::<i32>().ok()
}

/// Extract the stable ATP player code from a profile href like
/// `/en/players/jannik-sinner/s0ag/overview`.
fn player_code_from_href(href: &str) -> Result<String> {
    let segments: Vec<&str> = href.split('/').filter(|s| !s.is_empty()).collect();
    let players_idx = segments
        .iter()
        .position(|s| *s == "players")
        .ok_or_else(|| anyhow!("player href {href:?} has no /players/ segment"))?;
    let code = segments
        .get(players_idx + 2)
        .ok_or_else(|| anyhow!("player href {href:?} has no code segment"))?;
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("player href {href:?} has implausible code {code:?}");
    }
    Ok(code.to_lowercase())
}

struct RowSelectors {
    row: Selector,
    rank: Selector,
    movement: Selector,
    player_link: Selector,
    country: Selector,
    points: Selector,
    tournament: Selector,
    event_link: Selector,
    round: Selector,
    idle: Selector,
    next: Selector,
    max: Selector,
}

impl RowSelectors {
    fn new() -> Self {
        Self {
            row: sel("table.race-table tbody tr.race-row"),
            rank: sel("td.rank"),
            movement: sel("td.move"),
            player_link: sel("td.player a"),
            country: sel("td.player .country"),
            points: sel("td.points"),
            tournament: sel("td.tournament"),
            event_link: sel("td.tournament a"),
            round: sel("td.tournament .round"),
            idle: sel("td.tournament .idle"),
            next: sel("td.next"),
            max: sel("td.max"),
        }
    }
}

fn parse_row(tr: ElementRef, s: &RowSelectors) -> Result<RaceRow> {
    let rank = cell_text(tr, &s.rank)
        .ok_or_else(|| anyhow!("rank cell missing"))?
        .parse::<u32>()
        .context("cannot parse rank")?;

    let player_el = tr
        .select(&s.player_link)
        .next()
        .ok_or_else(|| anyhow!("player link missing"))?;
    let player_name = player_el.text().collect::<String>().trim().to_string();
    if player_name.is_empty() {
        bail!("player name empty");
    }
    let href = player_el
        .value()
        .attr("href")
        .ok_or_else(|| anyhow!("player link has no href"))?;
    let player_code = player_code_from_href(href)?;

    let country = cell_text(tr, &s.country).unwrap_or_default();

    let live_points = parse_points(
        &cell_text(tr, &s.points).ok_or_else(|| anyhow!("points cell missing"))?,
    )?;

    let (event, mut unavailable_reason) = match tr.select(&s.event_link).next() {
        Some(link) => {
            let name = link.text().collect::<String>().trim().to_string();
            let round = cell_text(tr, &s.round).unwrap_or_default();
            (Some(EventState { name, round }), None)
        }
        None => {
            let reason = tr
                .select(&s.idle)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .or_else(|| cell_text(tr, &s.tournament))
                .filter(|t| !t.is_empty() && t != "-" && t != "–" && t != "—")
                .unwrap_or_else(|| "not playing this week".to_string());
            (None, Some(reason))
        }
    };

    let next_points = parse_optional_points(cell_text(tr, &s.next))?;
    let max_this_week = parse_optional_points(cell_text(tr, &s.max))?;
    if next_points.is_some() && max_this_week.is_some() {
        unavailable_reason = None;
    }

    Ok(RaceRow {
        rank,
        movement: parse_movement(cell_text(tr, &s.movement)),
        player_code,
        player_name,
        country,
        live_points,
        event,
        next_points,
        max_this_week,
        unavailable_reason,
    })
}

/// Parse a race page into a candidate snapshot and validate it.
/// A candidate that fails validation is rejected wholesale — a bad parse
/// must never replace the last verified snapshot.
pub fn parse(html: &str, source_label: &str) -> Result<Snapshot> {
    let doc = Html::parse_document(html);

    let h1 = sel("h1");
    let heading_ok = doc
        .select(&h1)
        .map(|h| h.text().collect::<String>().to_lowercase())
        .any(|t| t.contains("race to turin"));
    if !heading_ok {
        bail!("required heading 'Race To Turin' not found; page structure changed or wrong document");
    }

    let time_sel = sel("time#source-as-of");
    let source_as_of = doc
        .select(&time_sel)
        .next()
        .and_then(|t| t.value().attr("datetime"))
        .ok_or_else(|| anyhow!("source timestamp (time#source-as-of) missing"))?;
    let source_as_of = OffsetDateTime::parse(source_as_of, &Rfc3339)
        .context("source timestamp is not RFC 3339")?;

    let selectors = RowSelectors::new();
    let mut rows = Vec::new();
    for tr in doc.select(&selectors.row) {
        let row = parse_row(tr, &selectors)
            .with_context(|| format!("row {} failed to parse", rows.len() + 1))?;
        rows.push(row);
    }

    let snapshot = Snapshot {
        source_as_of,
        generated_at: OffsetDateTime::now_utc(),
        source: source_label.to_string(),
        parser_version: PARSER_VERSION,
        rows,
    };
    validate(&snapshot)?;
    Ok(snapshot)
}

/// Structural invariants from the PRD; violations quarantine the candidate.
fn validate(snapshot: &Snapshot) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let rows = &snapshot.rows;

    if rows.len() < 8 {
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
    let mut codes = std::collections::HashSet::new();
    for row in rows {
        if !codes.insert(row.player_code.as_str()) {
            problems.push(format!("duplicate player code {}", row.player_code));
        }
    }
    for pair in rows.windows(2) {
        if pair[1].live_points > pair[0].live_points {
            problems.push(format!(
                "points not non-increasing at rank {}: {} > {}",
                pair[1].rank, pair[1].live_points, pair[0].live_points
            ));
        }
    }
    for row in rows {
        if let Some(next) = row.next_points {
            if next < row.live_points {
                problems.push(format!(
                    "{}: next_points {} < live_points {}",
                    row.player_code, next, row.live_points
                ));
            }
            if let Some(max) = row.max_this_week {
                if max < next {
                    problems.push(format!(
                        "{}: max_this_week {} < next_points {}",
                        row.player_code, max, next
                    ));
                }
            }
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

    const FIXTURE: &str = include_str!("../fixtures/race.html");

    fn fixture_snapshot() -> Snapshot {
        parse(FIXTURE, "fixtures/race.html").expect("fixture must parse")
    }

    #[test]
    fn parses_all_fixture_rows() {
        let snap = fixture_snapshot();
        assert_eq!(snap.rows.len(), 20);
        assert_eq!(snap.parser_version, PARSER_VERSION);
    }

    #[test]
    fn extracts_identity_from_profile_links_not_names() {
        let snap = fixture_snapshot();
        let first = &snap.rows[0];
        assert_eq!(first.player_code, "a0e2");
        assert_eq!(first.player_name, "Carlos Alcaraz");
        assert_eq!(first.country, "ESP");
        assert_eq!(first.live_points, 8760);
    }

    #[test]
    fn active_player_has_event_and_projections() {
        let snap = fixture_snapshot();
        let fritz = snap.rows.iter().find(|r| r.player_code == "fb98").unwrap();
        let event = fritz.event.as_ref().expect("Fritz is active in fixture");
        assert_eq!(event.name, "Cincinnati");
        assert_eq!(event.round, "QF");
        assert_eq!(fritz.next_points, Some(4320));
        assert_eq!(fritz.max_this_week, Some(4920));
        assert!(fritz.unavailable_reason.is_none());
    }

    #[test]
    fn idle_player_gets_reason_instead_of_estimates() {
        let snap = fixture_snapshot();
        let zverev = snap.rows.iter().find(|r| r.player_code == "z355").unwrap();
        assert!(zverev.event.is_none());
        assert_eq!(zverev.next_points, None);
        assert_eq!(zverev.max_this_week, None);
        assert_eq!(zverev.unavailable_reason.as_deref(), Some("Eliminated R32"));
    }

    #[test]
    fn source_timestamp_is_parsed() {
        let snap = fixture_snapshot();
        assert_eq!(snap.source_as_of.year(), 2026);
        assert_eq!(snap.source_as_of.hour(), 18);
    }

    fn wrap_rows(rows: &str) -> String {
        format!(
            r#"<html><body><h1>ATP Live Race To Turin</h1>
            <time id="source-as-of" datetime="2026-08-19T18:42:07Z">now</time>
            <table class="race-table"><tbody>{rows}</tbody></table></body></html>"#
        )
    }

    fn simple_row(rank: u32, code: &str, points: &str) -> String {
        format!(
            r#"<tr class="race-row"><td class="rank">{rank}</td><td class="move">0</td>
            <td class="player"><a href="/en/players/x/{code}/overview">Player {code}</a>
            <span class="country">USA</span></td><td class="points">{points}</td>
            <td class="tournament"><span class="idle">Not entered</span></td>
            <td class="next">–</td><td class="max">–</td></tr>"#
        )
    }

    fn eight_rows_with(mutator: impl Fn(u32) -> String) -> String {
        (1..=8).map(mutator).collect::<Vec<_>>().join("")
    }

    #[test]
    fn rejects_missing_heading() {
        let html = wrap_rows("").replace("Race To Turin", "Maintenance");
        let err = parse(&html, "test").unwrap_err().to_string();
        assert!(err.contains("heading"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_duplicate_ranks() {
        let html = wrap_rows(&eight_rows_with(|i| {
            let rank = if i == 5 { 4 } else { i };
            simple_row(rank, &format!("p{i:03}"), &format!("{}", 9000 - i * 100))
        }));
        let err = parse(&html, "test").unwrap_err().to_string();
        assert!(err.contains("rank not contiguous"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_duplicate_player_codes() {
        let html = wrap_rows(&eight_rows_with(|i| {
            let code = if i == 6 { "p001".to_string() } else { format!("p{i:03}") };
            simple_row(i, &code, &format!("{}", 9000 - i * 100))
        }));
        let err = parse(&html, "test").unwrap_err().to_string();
        assert!(err.contains("duplicate player code"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_unparseable_points() {
        let html = wrap_rows(&eight_rows_with(|i| {
            let points = if i == 3 { "lots".to_string() } else { format!("{}", 9000 - i * 100) };
            simple_row(i, &format!("p{i:03}"), &points)
        }));
        assert!(parse(&html, "test").is_err());
    }

    #[test]
    fn rejects_points_out_of_order() {
        let html = wrap_rows(&eight_rows_with(|i| {
            let points = if i == 4 { 9500 } else { 9000 - i * 100 };
            simple_row(i, &format!("p{i:03}"), &points.to_string())
        }));
        let err = parse(&html, "test").unwrap_err().to_string();
        assert!(err.contains("non-increasing"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_short_tables_like_maintenance_pages() {
        let html = wrap_rows(&simple_row(1, "p001", "9000"));
        let err = parse(&html, "test").unwrap_err().to_string();
        assert!(err.contains("row count"), "unexpected error: {err}");
    }

    #[test]
    fn never_panics_on_arbitrary_html() {
        for garbage in [
            "",
            "<html>",
            "<h1>Race To Turin</h1>",
            "<h1>Race To Turin</h1><table class=\"race-table\"><tbody><tr class=\"race-row\"></tr></tbody></table>",
            "\u{0}\u{1}<<<>>>",
            "<h1>race to turin</h1><time id=\"source-as-of\" datetime=\"junk\"></time>",
        ] {
            let _ = parse(garbage, "test"); // must return Err, not panic
        }
    }
}
