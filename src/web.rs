use std::{sync::Arc, time::Duration};

use askama::Template;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use time::{macros::format_description, OffsetDateTime};

use crate::{
    curated::Curated,
    model::Snapshot,
    qualification::{Provisional, SeatBasis, Selection},
};

pub struct AppState {
    pub snapshot: Snapshot,
    /// Monotonic published-snapshot version from the store.
    pub version: i64,
    pub curated: Curated,
    pub selection: Selection,
    pub stale_after: Duration,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/methodology", get(methodology))
        .route("/health/ready", get(health_ready))
        .route("/static/app.css", get(app_css))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// View models: templates stay dumb; every displayed string is computed here.
// ---------------------------------------------------------------------------

struct FreshView {
    accuracy_label: String,
    is_stale: bool,
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

struct RowView {
    rank: u32,
    movement: String,
    movement_class: String,
    movement_sr: String,
    name: String,
    country: String,
    official: bool,
    has_status: bool,
    status_label: String,
    status_class: String,
    points: String,
    event: String,
    next: String,
    next_title: String,
    max: String,
    margin: String,
    margin_class: String,
    row_class: String,
    cut_strong_after: bool,
    cut_ordinary_after: bool,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    season: u16,
    fresh: FreshView,
    summary: SummaryView,
    rows: Vec<RowView>,
    total: usize,
    slam_provision_active: bool,
}

#[derive(Template)]
#[template(path = "methodology.html")]
struct MethodologyPage {
    season: u16,
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

fn fmt_rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| t.to_string())
}

fn build_fresh(state: &AppState, now: OffsetDateTime) -> FreshView {
    let age_secs = (now - state.snapshot.source_as_of).whole_seconds();
    let is_stale = age_secs > state.stale_after.as_secs() as i64;
    FreshView {
        accuracy_label: if is_stale {
            "stale — last known good".to_string()
        } else {
            "scraped live (local fixture)".to_string()
        },
        is_stale,
        source_dt: fmt_rfc3339(state.snapshot.source_as_of),
        source_human: fmt_human(state.snapshot.source_as_of),
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

    let officials = if state.curated.official_qualifiers.is_empty() {
        "None announced yet".to_string()
    } else {
        state
            .curated
            .official_qualifiers
            .iter()
            .map(|p| p.name.clone())
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

fn build_rows(state: &AppState) -> Vec<RowView> {
    let officials = state.curated.official_qualifier_codes();
    let slam_active = state.selection.eighth_basis == SeatBasis::GrandSlamChampion;

    let mut views: Vec<RowView> = state
        .snapshot
        .rows
        .iter()
        .map(|row| {
            let (movement, movement_class, movement_sr) = match row.movement {
                Some(m) if m > 0 => (format!("▲{m}"), "up", format!("up {m} from last week")),
                Some(m) if m < 0 => {
                    (format!("▼{}", -m), "down", format!("down {} from last week", -m))
                }
                Some(_) => ("·".to_string(), "flat", "no change from last week".to_string()),
                None => ("–".to_string(), "flat", "movement not shown".to_string()),
            };

            let provisional = state.selection.state(&row.player_code);
            let (status_label, status_class) = match provisional {
                Provisional::TopSeven => ("Top 7", "in"),
                Provisional::EighthByRank => ("8th seat", "in"),
                Provisional::EighthBySlamRule => ("8th seat · Slam rule", "slam"),
                Provisional::FirstAlternate => ("First alternate", "alt"),
                Provisional::Withdrawn => ("Withdrawn", "out"),
                Provisional::NotSelected => ("", ""),
            };

            let event = match &row.event {
                Some(e) if e.round.is_empty() => e.name.clone(),
                Some(e) => format!("{} — {}", e.name, e.round),
                None => row
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "Not playing".to_string()),
            };

            let reason = row
                .unavailable_reason
                .clone()
                .unwrap_or_else(|| "not displayed by the source".to_string());
            let (next, next_title) = match row.next_points {
                Some(n) => (thousands(n), "Total after one more win".to_string()),
                None => ("–".to_string(), format!("Unavailable: {reason}")),
            };
            let max = match row.max_this_week {
                Some(m) => thousands(m),
                None => "–".to_string(),
            };

            let (margin, margin_class) = match state.selection.margin(&row.player_code) {
                Some(m) if m >= 0 => (signed_thousands(m), "pos"),
                Some(m) => (signed_thousands(m), "neg"),
                None => ("–".to_string(), ""),
            };

            let mut classes: Vec<&str> = Vec::new();
            match provisional {
                Provisional::TopSeven | Provisional::EighthByRank => classes.push("selected"),
                Provisional::EighthBySlamRule => {
                    classes.push("selected");
                    classes.push("slam-pick");
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
                country: row.country.clone(),
                official: officials.contains(row.player_code.as_str()),
                has_status: !status_label.is_empty(),
                status_label: status_label.to_string(),
                status_class: status_class.to_string(),
                points: thousands(row.live_points),
                event,
                next,
                next_title,
                max,
                margin,
                margin_class: margin_class.to_string(),
                row_class: classes.join(" "),
                cut_strong_after: false,
                cut_ordinary_after: false,
            }
        })
        .collect();

    // Boundary markers: a strong line after the seventh seat, and an
    // ordinary 8/9 line only when seat 8 comes from race rank. Skip both
    // if withdrawals have made the displayed order non-contiguous.
    let top_seven_contiguous = views
        .iter()
        .take(7)
        .filter(|v| v.status_label == "Top 7")
        .count()
        == 7;
    if top_seven_contiguous && views.len() > 7 {
        views[6].cut_strong_after = true;
        if !slam_active && views[7].status_label == "8th seat" {
            views[7].cut_ordinary_after = true;
        }
    }

    views
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn render<T: Template>(template: &T) -> Response {
    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {err}"),
        )
            .into_response(),
    }
}

async fn index(State(state): State<Arc<AppState>>) -> Response {
    let page = IndexPage {
        season: state.curated.season,
        fresh: build_fresh(&state, OffsetDateTime::now_utc()),
        summary: build_summary(&state),
        rows: build_rows(&state),
        total: state.snapshot.rows.len(),
        slam_provision_active: state.selection.eighth_basis == SeatBasis::GrandSlamChampion,
    };
    render(&page)
}

async fn methodology(State(state): State<Arc<AppState>>) -> Response {
    let page = MethodologyPage {
        season: state.curated.season,
        ruleset: state.curated.ruleset.clone(),
        parser_version: state.snapshot.parser_version.clone(),
        source: state.snapshot.source.clone(),
    };
    render(&page)
}

// Startup fails outright without a valid snapshot, so a running process is
// always ready to serve.
async fn health_ready() -> &'static str {
    "ready\n"
}

async fn app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../static/app.css"),
    )
}
