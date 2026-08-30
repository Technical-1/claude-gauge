//! The one network call: GET /api/oauth/usage for a single account.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub name: String,
    pub pct: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// True for windows that represent a real consumable limit worth gauging.
    /// The response also carries several opaque, always-0.0 keys (`nimbus_quill`,
    /// `amber_ladder`, `tangelo` …) whose meaning is not documented; including
    /// them in "worst window" would be inventing precision we do not have.
    pub gauge: bool,
}

pub enum FetchError {
    /// Kept separate from `Http` because it is the only error the caller can act
    /// on: it means "come back later", and the server usually says how much later.
    RateLimited { retry_after: Option<i64> },
    Http { code: u16, message: String },
    Other(String),
}

/// Append-only record of every request this process actually spends.
///
/// Added because "it kept 429ing" is not evidence. With this, a run answers
/// exactly how many requests went out, when, and what came back.
pub fn log_request(label: &str, outcome: &str) {
    use std::io::Write;
    let dir = crate::accounts::dirs_home().join(".config/claude-gauge");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("requests.log"))
    {
        let _ = writeln!(f, "{} {:<8} {}", Utc::now().to_rfc3339(), label, outcome);
    }
}

/// `Retry-After` is either delta-seconds or an HTTP-date. Accept both; a value we
/// cannot parse must fall through to our own backoff rather than being treated as 0.
fn parse_retry_after(v: &str) -> Option<i64> {
    if let Ok(secs) = v.trim().parse::<i64>() {
        return Some(secs.clamp(0, 3600));
    }
    let when = DateTime::parse_from_rfc2822(v.trim()).ok()?;
    let d = when.with_timezone(&Utc) - Utc::now();
    Some(d.num_seconds().clamp(0, 3600))
}

/// The API reports some limits twice: once as a top-level object carrying
/// `utilization`, and again inside `limits[]`. Same limit, two names.
///
/// Dedup by IDENTITY, never by value. `nimbus_quill` and `weekly_scoped:Fable`
/// currently share a pct (0.0) and a reset (none) but are unrelated limits — a
/// "merge rows that look the same" rule would collapse them today and split them
/// the moment Fable is used, so rows would appear and vanish between refreshes.
pub fn canonical(name: &str) -> &str {
    match name {
        "session" => "five_hour",
        "weekly_all" => "seven_day",
        n => n,
    }
}

/// The two windows that always show, even at 0%.
///
/// 0% is meaningful for these and only these: it means "this account has full
/// headroom", which is exactly what you opened the menu to find out. For every
/// other window — a per-model cap like `weekly_scoped:Fable`, or one of the
/// undocumented codename keys (`nimbus_quill`, `amber_ladder`, `cinder_cove`,
/// `tangelo`, `iguana_necktie`, `omelette_promotional`, all observed permanently
/// `0.0`) — 0% means "not a limit you are anywhere near", which is noise.
///
/// So the rule is not "hide zeros", it is "hide zeros that say nothing". An
/// allow-list of two beats a deny-list of six: a codename key we have never seen
/// still behaves correctly, and if any of them ever carries a real value it
/// appears automatically rather than staying hidden by name.
fn is_core(name: &str) -> bool {
    matches!(canonical(name), "five_hour" | "seven_day")
}

/// Hidden while zero, but NOT hidden unconditionally: a row that ever carries a
/// real value must appear, because a meter that can hide a live limit is worse
/// than a noisy one.
fn hidden_at_zero(w: &Window) -> bool {
    w.pct == 0.0 && !is_core(&w.name)
}

/// Raw API keys are an implementation detail; the menu shows prose.
/// Unrecognised keys fall through as-is rather than being dropped.
pub fn display_name(name: &str) -> String {
    match name {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        "seven_day_opus" => "Weekly · Opus".into(),
        "seven_day_sonnet" => "Weekly · Sonnet".into(),
        "seven_day_cowork" => "Weekly · Cowork".into(),
        "seven_day_oauth_apps" => "Weekly · Apps".into(),
        n => match n.strip_prefix("weekly_scoped:") {
            Some(model) => format!("Weekly · {model}"),
            None => n.to_string(),
        },
    }
}

/// Windows worth showing: aliases collapsed, opaque-and-zero hidden, worst first.
pub fn visible(windows: &[Window]) -> Vec<&Window> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out: Vec<&Window> = Vec::new();
    // Canonical names first, so `five_hour` wins over its `session` alias even if
    // the response ever orders them the other way round.
    for pass_canonical in [true, false] {
        for w in windows {
            let canon = canonical(&w.name);
            if (w.name == canon) != pass_canonical || seen.contains(&canon) {
                continue;
            }
            if hidden_at_zero(w) {
                continue;
            }
            seen.push(canon);
            out.push(w);
        }
    }
    out.sort_by(|a, b| b.pct.partial_cmp(&a.pct).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Window names that are known, meaningful consumables. Anything else is still
/// displayed in the menu but never drives the headline number.
fn is_gauge(name: &str) -> bool {
    name == "five_hour"
        || name == "seven_day"
        || name.starts_with("seven_day_")
        || name.starts_with("session")
        || name.starts_with("weekly")
}

pub fn fetch(access_token: &str) -> Result<Vec<Window>, FetchError> {
    // http_status_as_error(false): ureq's default turns any 4xx/5xx into
    // Error::StatusCode(u16), which throws away the response — including the
    // `Retry-After` header, the one piece of information that makes a 429
    // actionable rather than something to guess about.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();

    let mut resp = agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .call()
        .map_err(|e| FetchError::Other(format!("{e}")))?;

    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_retry_after);

    if status == 429 {
        return Err(FetchError::RateLimited { retry_after });
    }
    if status >= 400 {
        return Err(FetchError::Http {
            code: status,
            message: match status {
                401 | 403 => "unauthorized — sign in again".into(),
                _ => "request refused".into(),
            },
        });
    }

    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| FetchError::Other(format!("read: {e}")))?;

    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| FetchError::Other(format!("parse: {e}")))?;

    // The endpoint can answer 200 with an error envelope.
    if let Some(err) = v.get("error") {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        if err.get("type").and_then(|t| t.as_str()) == Some("rate_limit_error") {
            return Err(FetchError::RateLimited { retry_after: None });
        }
        return Err(FetchError::Http { code: 200, message });
    }

    let mut out: Vec<Window> = Vec::new();

    // Shape 1: top-level objects carrying `utilization`. Keys are account-dependent
    // and null-heavy, so iterate rather than index by name.
    if let Some(map) = v.as_object() {
        for (k, val) in map {
            if let Some(pct) = val.get("utilization").and_then(|u| u.as_f64()) {
                out.push(Window {
                    name: k.clone(),
                    pct,
                    resets_at: parse_ts(val.get("resets_at")),
                    gauge: is_gauge(k),
                });
            }
        }
    }

    // Shape 2: `limits[]`, which carries per-model weekly caps the top-level keys omit.
    if let Some(limits) = v.get("limits").and_then(|l| l.as_array()) {
        for e in limits {
            let Some(pct) = e.get("percent").and_then(|p| p.as_f64()) else {
                continue;
            };
            let kind = e.get("kind").and_then(|k| k.as_str()).unwrap_or("limit");
            let model = e
                .get("scope")
                .and_then(|s| s.get("model"))
                .and_then(|m| m.get("display_name"))
                .and_then(|d| d.as_str());
            let name = match model {
                Some(m) => format!("{kind}:{m}"),
                None => kind.to_string(),
            };
            let gauge = is_gauge(kind);
            out.push(Window {
                name,
                pct,
                resets_at: parse_ts(e.get("resets_at")),
                gauge,
            });
        }
    }

    if out.is_empty() {
        return Err(FetchError::Other("no usage windows in response".into()));
    }
    out.sort_by(|a, b| b.pct.partial_cmp(&a.pct).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// `resets_at` is an ISO-8601 string on this endpoint, but epoch seconds elsewhere
/// in the same schema family — accept both rather than assume.
fn parse_ts(v: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
    }
    if let Some(n) = v.as_i64() {
        return DateTime::from_timestamp(n, 0);
    }
    None
}

/// "3h 21m" / "2d 4h" — a countdown you can read at a glance.
pub fn until(t: DateTime<Utc>) -> String {
    let secs = (t - Utc::now()).num_seconds();
    if secs <= 0 {
        return "now".into();
    }
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}
