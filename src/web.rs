use std::{sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use askama::Template;
use axum::{
    extract::State,
    http::{header, HeaderName, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use time::{macros::format_description, OffsetDateTime};
use tower_http::compression::CompressionLayer;

use crate::{
    curated::Curated,
    model::{Played, RaceRow, Snapshot},
    qualification::{Provisional, SeatBasis, Selection},
};

pub struct AppState {
    pub snapshot: Snapshot,
    /// Monotonic published-snapshot version from the store.
    pub version: i64,
    pub curated: Curated,
    pub selection: Selection,
    pub stale_after: Duration,
    pub check_stale_after: Duration,
    /// Public origin, used for canonical and shared-link metadata.
    pub base_url: String,
}

/// Handlers read a snapshot of the state; the worker swaps in a new one
/// atomically, so a request never blocks on collection.
pub type SharedState = Arc<ArcSwap<AppState>>;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/methodology", get(methodology))
        .route("/health/ready", get(health_ready))
        .route("/health/fresh", get(health_fresh))
        .route("/robots.txt", get(robots))
        .route("/static/app.css", get(app_css))
        // Compression matters more than it looks: the page is mostly repeated
        // markup, so it shrinks ~4x, which is the whole egress bill under load.
        .layer(CompressionLayer::new())
        .with_state(state)
}

/// Short enough that a refresh shows up promptly, long enough that a CDN
/// absorbs a traffic spike instead of the single machine.
const CACHE_PAGE: &str = "public, max-age=120, stale-while-revalidate=600";
/// The methodology page only changes when the code does.
const CACHE_STATIC: &str = "public, max-age=3600";
/// Safe to cache forever because the URL carries a content hash: a changed
/// stylesheet is a different URL.
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

const APP_CSS: &str = include_str!("../static/app.css");

/// Content hash of the stylesheet, used to bust its cache. Markup and styles
/// ship together, so a cached-but-stale stylesheet renders new markup wrong —
/// versioning the URL makes that impossible.
pub fn css_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let digest = <sha2::Sha256 as sha2::Digest>::digest(APP_CSS.as_bytes());
        format!("{digest:x}")[..12].to_string()
    })
}

// ---------------------------------------------------------------------------
// View models: templates stay dumb; every displayed string is computed here.
// ---------------------------------------------------------------------------

struct FreshView {
    accuracy_label: String,
    is_stale: bool,
    /// True when our own collection has not succeeded recently — a different
    /// problem from the source itself being old, and the one that means
    /// something is broken on our side.
    collection_stale: bool,
    checked_age: String,
    source_dt: String,
    source_human: String,
    generated_dt: String,
    generated_human: String,
    age: String,
    source: String,
    parser_version: String,
    ruleset: String,
    version: i64,
}

struct SummaryView {
    officials: String,
    top_seven: String,
    eighth: String,
    alternate: String,
    basis_sentence: String,
}

/// One line of a player's points breakdown.
struct ResultView {
    /// The event played, or the mandatory event that was not.
    event: String,
    /// Wikipedia article for the draw, so a reader can check the number.
    event_url: Option<String>,
    /// Visible qualifier after the event name: "for Miami" on a substitution.
    qualifier: String,
    round: String,
    points: String,
    /// Spoken in place of a symbol that is not self-explanatory.
    sr_note: String,
    class: String,
}

struct LedgerGroup {
    title: String,
    results: Vec<ResultView>,
}

struct LedgerView {
    /// "15 of 19 tournaments count · 1 title"
    headline: String,
    /// Names the player: a screen reader meets twelve of these summaries.
    sr_intro: String,
    groups: Vec<LedgerGroup>,
    total: String,
    /// Set only when some result stands in for a mandatory Masters 1000.
    substitution_note: String,
}

struct RowView {
    rank: u32,
    movement: String,
    movement_class: String,
    movement_sr: String,
    name: String,
    /// ATP profile, when Wikidata gives us an id for this player; `None`
    /// renders the name unlinked.
    profile_url: Option<String>,
    country: String,
    official: bool,
    status_label: String,
    status_class: String,
    points: String,
    margin: String,
    margin_class: String,
    row_class: String,
    /// The per-tournament breakdown, absent when the source's cells did not
    /// reconcile against this row's total.
    ledger: Option<LedgerView>,
    cut_strong_after: bool,
    cut_ordinary_after: bool,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    season: u16,
    css_version: &'static str,
    canonical: String,
    /// Doubles as meta description and shared-link preview text.
    summary_text: String,
    fresh: FreshView,
    summary: SummaryView,
    rows: Vec<RowView>,
    /// False when no row carries a breakdown — serving a snapshot written
    /// before ledgers existed, say — so the page does not offer an expansion
    /// that is not there.
    any_ledger: bool,
    slam_provision_active: bool,
}

#[derive(Template)]
#[template(path = "methodology.html")]
struct MethodologyPage {
    season: u16,
    css_version: &'static str,
    canonical: String,
    summary_text: String,
    notice: String,
    ruleset: String,
    parser_version: String,
    source: String,
}

fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn signed_thousands(n: i64) -> String {
    // Margins are differences of u32 point totals, so the cast is safe.
    let magnitude = thousands(n.unsigned_abs() as u32);
    if n >= 0 {
        format!("+{magnitude}")
    } else {
        format!("-{magnitude}")
    }
}

fn humanize_age(seconds: i64) -> String {
    if seconds < 0 {
        return "0 s".to_string();
    }
    if seconds < 90 {
        format!("{seconds} s")
    } else if seconds < 90 * 60 {
        format!("{} min", seconds / 60)
    } else if seconds < 48 * 3600 {
        format!("{} h", seconds / 3600)
    } else {
        format!("{} days", seconds / 86_400)
    }
}

fn fmt_human(t: OffsetDateTime) -> String {
    let fmt = format_description!("[day] [month repr:short] [year] [hour]:[minute] UTC");
    t.format(&fmt).unwrap_or_else(|_| t.to_string())
}

/// The source states a date, not a time, so render it as a date.
fn fmt_day(t: OffsetDateTime) -> String {
    let fmt = format_description!("[day] [month repr:long] [year]");
    t.format(&fmt).unwrap_or_else(|_| t.to_string())
}

fn fmt_rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| t.to_string())
}

fn build_fresh(state: &AppState, now: OffsetDateTime) -> FreshView {
    let age_secs = (now - state.snapshot.source_as_of).whole_seconds();
    let is_stale = age_secs > state.stale_after.as_secs() as i64;
    let checked_secs = (now - state.snapshot.generated_at).whole_seconds();
    let collection_stale = checked_secs > state.check_stale_after.as_secs() as i64;
    FreshView {
        collection_stale,
        checked_age: humanize_age(checked_secs),
        // The source publishes weekly with a stated date, so the page never
        // claims to be live.
        accuracy_label: if is_stale {
            "stale — last known good".to_string()
        } else {
            "official weekly".to_string()
        },
        is_stale,
        source_dt: fmt_rfc3339(state.snapshot.source_as_of),
        source_human: fmt_day(state.snapshot.source_as_of),
        generated_dt: fmt_rfc3339(state.snapshot.generated_at),
        generated_human: fmt_human(state.snapshot.generated_at),
        age: humanize_age(age_secs),
        source: state.snapshot.source.clone(),
        parser_version: state.snapshot.parser_version.clone(),
        ruleset: state.curated.ruleset.clone(),
        version: state.version,
    }
}

fn build_summary(state: &AppState) -> SummaryView {
    let name_of = |code: &str| -> String {
        state
            .snapshot
            .rows
            .iter()
            .find(|r| r.player_code == code)
            .map(|r| r.player_name.clone())
            .unwrap_or_else(|| code.to_string())
    };

    // Officially announced qualifications, ingested with the source that
    // announced each one. Never inferred from points.
    let officials = if state.snapshot.qualifiers.is_empty() {
        "None announced yet".to_string()
    } else {
        state
            .snapshot
            .qualifiers
            .iter()
            .map(|q| format!("{} ({})", name_of(&q.player_code), q.qualified_on))
            .collect::<Vec<_>>()
            .join(", ")
    };

    // Rows are already in rank order (validated), so filtering preserves it.
    let top_seven = state
        .snapshot
        .rows
        .iter()
        .filter(|r| state.selection.state(&r.player_code) == Provisional::TopSeven)
        .map(|r| r.player_name.clone())
        .collect::<Vec<_>>()
        .join(", ");

    let (eighth, basis_sentence) = match (&state.selection.eighth_code, state.selection.eighth_basis)
    {
        (Some(code), SeatBasis::GrandSlamChampion) => {
            let name = name_of(code);
            (
                format!("{name} — Grand Slam champion provision"),
                format!(
                    "Seat 8 goes to {name} as a {season} Grand Slam champion ranked 8–20, \
                     not to the player ranked 8 in the race. There is no ordinary 8/9 cutoff \
                     this week, so points margins to a single line are not shown.",
                    season = state.curated.season
                ),
            )
        }
        (Some(code), SeatBasis::RaceRank) => {
            let name = name_of(code);
            (
                format!("{name} — by race rank"),
                format!(
                    "Seat 8 goes to {name} by race rank: no eligible {season} Grand Slam \
                     champion is ranked 8–20, so the ordinary 8/9 cutoff applies.",
                    season = state.curated.season
                ),
            )
        }
        (None, _) => ("—".to_string(), String::new()),
    };

    let alternate = state
        .selection
        .alternate_code
        .as_deref()
        .map(name_of)
        .unwrap_or_else(|| "—".to_string());

    SummaryView {
        officials,
        top_seven,
        eighth,
        alternate,
        basis_sentence,
    }
}

fn plural(n: u32, singular: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

fn wikipedia_url(article: &str) -> String {
    format!("https://en.wikipedia.org/wiki/{}", article.replace(' ', "_"))
}

/// The expandable per-player breakdown. Returns None when the row carries no
/// ledger, which is how a breakdown that failed to reconcile is suppressed.
fn build_ledger(row: &RaceRow) -> Option<LedgerView> {
    if row.results.is_empty() {
        return None;
    }

    let counting = row.counting_results() as u32;
    // Only a player's best results count, so the ledger is routinely shorter
    // than the season they played. Say so in the summary rather than letting
    // the list imply a full schedule.
    let mut headline = match row.tournaments_played {
        Some(played) if played >= counting => {
            format!("{counting} of {played} tournaments count")
        }
        _ => plural(counting, "counting result"),
    };
    if let Some(titles) = row.titles {
        headline.push_str(" · ");
        headline.push_str(&match titles {
            0 => "no titles".to_string(),
            n => plural(n, "title"),
        });
    }

    let mut groups: Vec<LedgerGroup> = Vec::new();
    for result in &row.results {
        let view = match result.played {
            // A mandatory column's label is the name the reader knows —
            // "Indian Wells", not "BNP Paribas Open" — so prefer it. On a
            // substitution it names the event *replaced*, so the event played
            // has to be named instead, with the slot as a qualifier.
            Played::Result => ResultView {
                event: match (result.substituted, result.slot_label.is_empty()) {
                    (false, false) => result.slot_label.clone(),
                    _ => result.event_name.clone(),
                },
                event_url: Some(wikipedia_url(&result.event_code)),
                qualifier: if result.substituted && !result.slot_label.is_empty() {
                    format!("for {}", result.slot_label)
                } else {
                    String::new()
                },
                round: result.round.clone(),
                points: thousands(result.points),
                sr_note: String::new(),
                class: if result.substituted { "sub" } else { "" }.to_string(),
            },
            // A mandatory event that has happened and was skipped scores zero;
            // one that has not happened yet scores nothing at all. Different
            // facts, so they never render alike.
            Played::Absent => ResultView {
                event: result.slot_label.clone(),
                event_url: None,
                qualifier: String::new(),
                round: "A".to_string(),
                points: "0".to_string(),
                sr_note: "did not play".to_string(),
                class: "absent".to_string(),
            },
            Played::Pending => ResultView {
                event: result.slot_label.clone(),
                event_url: None,
                qualifier: String::new(),
                round: "–".to_string(),
                points: "–".to_string(),
                sr_note: "not played yet".to_string(),
                class: "pending".to_string(),
            },
        };
        match groups.last_mut() {
            Some(group) if group.title == result.slot.title() => group.results.push(view),
            _ => groups.push(LedgerGroup {
                title: result.slot.title().to_string(),
                results: vec![view],
            }),
        }
    }

    Some(LedgerView {
        headline,
        sr_intro: format!("Points breakdown for {}: ", row.player_name),
        groups,
        // Equal to the row's total by construction — a ledger is only kept
        // when it reconciles — so showing it lets a reader add up and check.
        total: thousands(row.ledger_points()),
        substitution_note: if row.results.iter().any(|r| r.substituted) {
            "Italicised results were counted in place of a mandatory Masters 1000, \
             which the rulebook allows for up to three of them."
                .to_string()
        } else {
            String::new()
        },
    })
}

fn build_rows(state: &AppState) -> Vec<RowView> {
    let officials: std::collections::HashSet<&str> = state
        .snapshot
        .qualifiers
        .iter()
        .map(|q| q.player_code.as_str())
        .collect();
    let slam_active = state.selection.eighth_basis == SeatBasis::GrandSlamChampion;

    let mut views: Vec<RowView> = state
        .snapshot
        .rows
        .iter()
        .map(|row| {
            let (movement, movement_class, movement_sr) = match row.movement {
                Some(m) if m > 0 => (format!("▲{m}"), "up", format!("up {m} places")),
                Some(m) if m < 0 => (format!("▼{}", -m), "down", format!("down {} places", -m)),
                Some(_) => ("·".to_string(), "flat", "no change since last update".to_string()),
                None => ("–".to_string(), "flat", "no previous snapshot".to_string()),
            };

            let provisional = state.selection.state(&row.player_code);
            let (status_label, status_class) = match provisional {
                Provisional::TopSeven => ("Top 7", "in"),
                Provisional::Eighth if slam_active => ("8th seat · Slam rule", "slam"),
                Provisional::Eighth => ("8th seat", "in"),
                Provisional::FirstAlternate => ("First alternate", "alt"),
                Provisional::Withdrawn => ("Withdrawn", "out"),
                Provisional::NotSelected => ("", ""),
            };

            let (margin, margin_class) = match state.selection.margin(&row.player_code) {
                Some(m) => (signed_thousands(m), if m >= 0 { "pos" } else { "neg" }),
                None => ("–".to_string(), ""),
            };

            let ledger = build_ledger(row);

            let mut classes: Vec<&str> = Vec::new();
            // The breakdown renders as its own row directly beneath, so the
            // pair needs one closing border, not two.
            if ledger.is_some() {
                classes.push("has-ledger");
            }
            match provisional {
                Provisional::TopSeven => classes.push("selected"),
                Provisional::Eighth => {
                    classes.push("selected");
                    if slam_active {
                        classes.push("slam-pick");
                    }
                }
                Provisional::FirstAlternate => classes.push("alternate"),
                Provisional::Withdrawn => classes.push("withdrawn"),
                Provisional::NotSelected => {}
            }

            RowView {
                rank: row.rank,
                movement,
                movement_class: movement_class.to_string(),
                movement_sr,
                name: row.player_name.clone(),
                profile_url: crate::atp::profile_url(&row.player_code),
                country: row.country.clone(),
                official: officials.contains(row.player_code.as_str()),
                status_label: status_label.to_string(),
                status_class: status_class.to_string(),
                points: thousands(row.race_points),
                margin,
                margin_class: margin_class.to_string(),
                row_class: classes.join(" "),
                ledger,
                cut_strong_after: false,
                cut_ordinary_after: false,
            }
        })
        .collect();

    // Boundary markers: a strong line after the seventh seat, and an
    // ordinary 8/9 line only when seat 8 comes from race rank. Skip both
    // if withdrawals have made the displayed order non-contiguous.
    let rows = &state.snapshot.rows;
    let seat = |i: usize| state.selection.state(&rows[i].player_code);
    let top_seven_contiguous =
        rows.len() > 7 && (0..7).all(|i| seat(i) == Provisional::TopSeven);
    if top_seven_contiguous {
        views[6].cut_strong_after = true;
        if !slam_active && seat(7) == Provisional::Eighth {
            views[7].cut_ordinary_after = true;
        }
    }

    views
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// One sentence naming who is in and who is closest out — the text someone sees
/// when the link is shared, so it should carry the actual news.
fn shared_link_text(state: &AppState, summary: &SummaryView) -> String {
    let cut = state
        .selection
        .eighth_code
        .as_deref()
        .map(|code| {
            state
                .snapshot
                .rows
                .iter()
                .find(|r| r.player_code == code)
                .map(|r| r.player_name.clone())
                .unwrap_or_else(|| code.to_string())
        })
        .unwrap_or_else(|| "—".to_string());
    format!(
        "Who would qualify for the {season} ATP Finals in Turin if selection happened now. \
         Seat 8: {cut}. First alternate: {alt}. Standings as of {as_of}.",
        season = state.curated.season,
        alt = summary.alternate,
        as_of = fmt_day(state.snapshot.source_as_of),
    )
}

fn render<T: Template>(template: &T, cache_control: &'static str, version: i64) -> Response {
    match template.render() {
        Ok(body) => (
            [
                (header::CACHE_CONTROL, cache_control.to_string()),
                // Lets an operator (or a monitor) see which snapshot a response
                // came from without parsing the page.
                (
                    HeaderName::from_static("x-snapshot-version"),
                    version.to_string(),
                ),
            ],
            Html(body),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {err}"),
        )
            .into_response(),
    }
}

async fn index(State(shared): State<SharedState>) -> Response {
    let state = shared.load();
    let summary = build_summary(&state);
    let rows = build_rows(&state);
    let page = IndexPage {
        season: state.curated.season,
        css_version: css_version(),
        canonical: state.base_url.clone(),
        summary_text: shared_link_text(&state, &summary),
        fresh: build_fresh(&state, OffsetDateTime::now_utc()),
        summary,
        any_ledger: rows.iter().any(|r| r.ledger.is_some()),
        rows,
        slam_provision_active: state.selection.eighth_basis == SeatBasis::GrandSlamChampion,
    };
    render(&page, CACHE_PAGE, state.version)
}

async fn methodology(State(shared): State<SharedState>) -> Response {
    let state = shared.load();
    let page = MethodologyPage {
        season: state.curated.season,
        css_version: css_version(),
        canonical: format!("{}/methodology", state.base_url),
        summary_text: "Where racetotur.in's standings come from, how the Turin \
                       qualification rule is applied, and what the freshness labels mean."
            .to_string(),
        notice: state.curated.notice.clone(),
        ruleset: state.curated.ruleset.clone(),
        parser_version: state.snapshot.parser_version.clone(),
        source: state.snapshot.source.clone(),
    };
    render(&page, CACHE_STATIC, state.version)
}

// Startup fails outright without a valid snapshot, so a running process is
// always ready to serve.
async fn health_ready() -> &'static str {
    "ready\n"
}

/// Monitoring hook: 503 when our collection has stopped succeeding, so an
/// external cron can alarm on a plain HTTP status. Distinct from readiness —
/// serving an aged snapshot is intended behaviour, not an outage.
async fn health_fresh(State(shared): State<SharedState>) -> Response {
    let state = shared.load();
    let now = OffsetDateTime::now_utc();
    let checked = (now - state.snapshot.generated_at).whole_seconds();
    let source = (now - state.snapshot.source_as_of).whole_seconds();
    let body = format!(
        "checked_age_seconds={checked}\nsource_age_seconds={source}\nsnapshot_version={}\n",
        state.version
    );
    if checked > state.check_stale_after.as_secs() as i64 {
        (StatusCode::SERVICE_UNAVAILABLE, format!("collection stale\n{body}")).into_response()
    } else {
        (StatusCode::OK, format!("fresh\n{body}")).into_response()
    }
}

async fn robots() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, CACHE_STATIC),
        ],
        "User-agent: *\nAllow: /\nDisallow: /health/\n",
    )
}

async fn app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, CACHE_IMMUTABLE),
        ],
        APP_CSS,
    )
}
