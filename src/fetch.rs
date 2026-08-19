//! Polite HTTP collection: one identified client, hard limits, no evasion.

use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Wikimedia's User-Agent policy requires a descriptive agent naming the tool
/// and a way to reach its operator.
const USER_AGENT: &str = "racetoturin/0.1 (+https://racetotur.in; independent noncommercial fan project; contact: evalir@init4.technology)";

/// The article response is a few hundred KB; far larger is not what we asked for.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

pub struct Fetcher {
    client: reqwest::Client,
}

impl Fetcher {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .context("cannot build HTTP client")?;
        Ok(Self { client })
    }

    /// One GET with a bounded body and no retry loop: the caller decides when
    /// to try again, and a failure means the stored snapshot keeps serving.
    pub async fn get(&self, url: &str) -> Result<String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(|v| format!(" (Retry-After: {v})"))
                .unwrap_or_default();
            bail!("{url} returned HTTP {status}{retry_after}");
        }
        if let Some(len) = response.content_length() {
            if len as usize > MAX_BODY_BYTES {
                bail!("{url} body of {len} bytes exceeds the {MAX_BODY_BYTES}-byte cap");
            }
        }

        let body = response
            .text()
            .await
            .with_context(|| format!("cannot read body of {url}"))?;
        if body.len() > MAX_BODY_BYTES {
            bail!(
                "{url} body of {} bytes exceeds the {MAX_BODY_BYTES}-byte cap",
                body.len()
            );
        }
        Ok(body)
    }
}
