//! Discovering Claude Code config roots and reading their credentials.
//!
//! READ-ONLY, DELIBERATELY. This module never writes to the keychain and never
//! performs an OAuth refresh, even though the stored blob contains a refreshToken
//! and we can see the accessToken has expired.
//!
//! Why: refresh tokens rotate. Spending one here without persisting the new pair
//! back into the keychain would invalidate the credential Claude Code itself holds
//! — this little meter would silently log you out of the account it is reporting
//! on. Writing it back instead means racing Claude Code for its own credential
//! store. Neither is worth it for a status display, so an expired token is
//! reported as a state, not worked around.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One configured account.
#[derive(Debug, Clone)]
pub struct Root {
    pub label: String,
    pub path: PathBuf,
}

/// The macOS Keychain service name Claude Code stores this root's credentials under.
///
/// Verified against the live keychain on 2026-08-29: the default root uses the bare
/// name, and every other root appends the first 8 hex of sha256 of its ABSOLUTE path.
///   ~/.claude       -> "Claude Code-credentials"
///   ~/.claude-work  -> "Claude Code-credentials-<first 8 hex of sha256 of the
///                       ABSOLUTE path>"
///
/// The path is the identity, which is why a second config root *is* a second
/// account. Verified against a live keychain on 2026-08-29.
pub fn service_name(path: &Path) -> String {
    let home = dirs_home();
    if path == home.join(".claude") {
        return "Claude Code-credentials".to_string();
    }
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("Claude Code-credentials-{}", &hex[..8])
}

pub fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// What we could learn about one account. Every failure is a named state rather
/// than an error string, because each one has a different fix and the menu says so.
#[derive(Debug, Clone)]
pub enum State {
    Ok(Vec<crate::usage::Window>),
    NotLoggedIn,
    /// accessToken's expiresAt is in the past. Fix: open that account once.
    Expired { hours_ago: i64 },
    /// Reached the API but it refused. 429 here usually means the account is
    /// actually rate limited, which is itself the answer you wanted.
    Http { code: u16, message: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AccountStatus {
    pub label: String,
    pub state: State,
    /// Seconds since the reading was actually fetched. 0 means "just now".
    /// Non-zero means the number came from cache — either because it was still
    /// fresh, or because we are backing off and chose to show the last good
    /// value rather than blanking the meter.
    pub age_s: i64,
    /// Running Claude Code sessions on this account.
    ///
    /// `None` means "could not determine", NOT zero. Rendering a confident `0`
    /// when enumeration failed would be a silent lie, which is the one failure
    /// mode a meter must not have.
    pub sessions: Option<usize>,
    /// (points/hour, seconds until 100%) when a real rising trend is measured.
    pub burn: Option<(f64, Option<i64>)>,
    /// The sessions themselves, for this account's submenu. Empty when
    /// enumeration failed — `sessions` is what distinguishes that from zero.
    pub session_list: Vec<crate::sessions::Session>,
}

impl AccountStatus {
    /// Highest utilisation across this account's windows — the number that decides
    /// whether the account is usable right now.
    pub fn worst(&self) -> Option<&crate::usage::Window> {
        match &self.state {
            State::Ok(ws) => ws
                .iter()
                .filter(|w| w.gauge)
                .max_by(|a, b| a.pct.partial_cmp(&b.pct).unwrap_or(std::cmp::Ordering::Equal)),
            _ => None,
        }
    }
}

struct Token {
    access: String,
    expires_at_ms: Option<i64>,
}

fn read_token(service: &str) -> Result<Option<Token>, String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .map_err(|e| format!("security: {e}"))?;
    if !out.status.success() {
        return Ok(None); // no such entry -> not logged in
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("credential parse: {e}"))?;
    let oauth = v.get("claudeAiOauth").ok_or("no claudeAiOauth field")?;
    let access = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .ok_or("no accessToken")?
        .to_string();
    Ok(Some(Token {
        access,
        expires_at_ms: oauth.get("expiresAt").and_then(|t| t.as_i64()),
    }))
}

/// No account is re-polled more often than this, no matter how many callers ask.
/// The tray timer, `--list`, `--title` and the Refresh button all share it.
pub const MIN_INTERVAL_S: i64 = 120;

/// An account at or above this is treated as walled — not worth starting work in.
const WALLED_PCT: f64 = 95.0;
/// It must fall back below THIS to count as freed. The gap is deliberate
/// hysteresis: a single threshold would re-fire every time a reading dithered
/// across 95%, and the quota meter is known to dither by a point or two.
const FREED_PCT: f64 = 80.0;

/// First backoff after a 429 when the server does not send `Retry-After`.
const BACKOFF_BASE_S: i64 = 300;
const BACKOFF_MAX_S: i64 = 1800;

fn status(label: &str, state: State, age_s: i64) -> AccountStatus {
    AccountStatus { label: label.to_string(), state, age_s, sessions: None, session_list: Vec::new(), burn: None }
}

fn from_cache(root: &Root, e: &crate::cache::Entry, now: i64) -> AccountStatus {
    let mut st = status(&root.label, State::Ok(e.windows.clone()), e.age(now));
    st.burn = e.burn();
    st
}

pub fn poll(root: &Root) -> AccountStatus {
    poll_inner(root, false)
}

/// "Refresh now" bypasses the freshness floor but NOT the backoff. Letting a
/// button push through a backoff is exactly how one 429 becomes a sustained one.
pub fn poll_forced(root: &Root) -> AccountStatus {
    poll_inner(root, true)
}

fn poll_inner(root: &Root, force: bool) -> AccountStatus {
    let now = chrono::Utc::now().timestamp();
    let mut entry = crate::cache::load(root).unwrap_or_default();

    // 1. Still fresh — serve it and spend nothing.
    if entry.has_data() && !force && entry.age(now) < MIN_INTERVAL_S {
        return from_cache(root, &entry, now);
    }

    // 2. Backing off after a 429. NOT spending the request is the entire fix;
    //    the old code retried every 60s per account and sustained the condition.
    if now < entry.backoff_until {
        return if entry.has_data() {
            from_cache(root, &entry, now)
        } else {
            status(&root.label, State::Http {
                code: 429,
                message: format!("rate limited — retrying in {}s", entry.backoff_until - now),
            }, 0)
        };
    }

    let service = service_name(&root.path);
    match read_token(&service) {
        Err(e) => status(&root.label, State::Error(e), 0),
        Ok(None) => status(&root.label, State::NotLoggedIn, 0),
        Ok(Some(tok)) => {
            let now_ms = now * 1000;
            // Check BEFORE spending a request. An expired token returns a
            // confusing 429 from this endpoint rather than a 401, so asking
            // first is the only way to report the real cause.
            if let Some(exp) = tok.expires_at_ms
                && exp < now_ms {
                    return status(&root.label, State::Expired {
                        hours_ago: (now_ms - exp) / 3_600_000,
                    }, 0);
                }
            match crate::usage::fetch(&tok.access) {
                Ok(ws) => {
                    crate::usage::log_request(&root.label, "200 ok");

                    // Only gauged windows decide this, for the same reason they
                    // decide the headline: the opaque always-0.0 codename keys
                    // would make every account look permanently free.
                    let worst = ws
                        .iter()
                        .filter(|w| w.gauge)
                        .map(|w| w.pct)
                        .fold(f64::NEG_INFINITY, f64::max);

                    if worst.is_finite() {
                        entry.push_history(now, worst);
                        if entry.was_walled && worst < FREED_PCT {
                            entry.was_walled = false;
                            crate::notify::post(
                                "Claude Usage",
                                &format!("{} is available again — now at {:.0}%", root.label, worst),
                            );
                        } else if !entry.was_walled && worst >= WALLED_PCT {
                            // Arm silently. Hitting the wall is something you
                            // already found out the hard way; being told it is
                            // over is the part you cannot observe for yourself.
                            entry.was_walled = true;
                        }
                    }

                    entry.windows = ws.clone();
                    entry.fetched_at = now;
                    entry.backoff_until = 0;
                    entry.strikes = 0;
                    crate::cache::store(root, &entry);
                    let mut st = status(&root.label, State::Ok(ws), 0);
                    st.burn = entry.burn();
                    st
                }
                Err(crate::usage::FetchError::RateLimited { retry_after }) => {
                    entry.strikes = entry.strikes.saturating_add(1);
                    // Prefer the server's own advice; otherwise back off
                    // exponentially so repeated refusals get quieter, not louder.
                    let wait = retry_after.unwrap_or_else(|| {
                        (BACKOFF_BASE_S << (entry.strikes - 1).min(3)).min(BACKOFF_MAX_S)
                    });
                    entry.backoff_until = now + wait;
                    crate::cache::store(root, &entry);
                    crate::usage::log_request(
                        &root.label,
                        &format!("429 strike={} backoff={}s retry_after={:?}",
                                 entry.strikes, wait, retry_after),
                    );
                    if entry.has_data() {
                        from_cache(root, &entry, now)
                    } else {
                        status(&root.label, State::Http {
                            code: 429,
                            message: format!("rate limited — retrying in {wait}s"),
                        }, 0)
                    }
                }
                Err(crate::usage::FetchError::Http { code, message }) => {
                    crate::usage::log_request(&root.label, &format!("{code} {message}"));
                    status(&root.label, State::Http { code, message }, 0)
                }
                Err(crate::usage::FetchError::Other(e)) => {
                    crate::usage::log_request(&root.label, &format!("err {e}"));
                    status(&root.label, State::Error(e), 0)
                }
            }
        }
    }
}

/// Config roots that look like accounts: `~/.claude`, plus any `~/.claude-*`
/// directory carrying the marks of a real root.
///
/// Labels are assigned positionally (`claude`, `claude2`, `claude3`, …) rather
/// than taken from the directory name: `short()` strips the `claude` prefix to
/// get the menubar tag, so a name like `.claude-work` would yield `[-work]`
/// instead of a number. Rename them in the config file if you want.
fn discover(home: &Path) -> Vec<Root> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let default = home.join(".claude");
    if default.is_dir() {
        dirs.push(default);
    }
    if let Ok(entries) = std::fs::read_dir(home) {
        let mut others: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".claude-"))
                    // A config root has these; a stray dotfile directory does not.
                    && (p.join("projects").is_dir() || p.join("settings.json").is_file())
            })
            .collect();
        others.sort();
        dirs.extend(others);
    }
    dirs.into_iter()
        .enumerate()
        .map(|(i, path)| Root {
            label: if i == 0 { "claude".into() } else { format!("claude{}", i + 1) },
            path,
        })
        .collect()
}

/// Accounts to show, from `~/.config/claude-usage/roots.json`, created on first run.
///
/// A config file rather than pure discovery: retired roots often still exist on
/// disk, and a meter that lists dead accounts trains you to ignore it. Discovery
/// seeds the file on first run; after that you edit it and it is left alone.
pub fn load_roots() -> Vec<Root> {
    let home = dirs_home();
    let cfg_dir = home.join(".config/claude-usage");
    let cfg = cfg_dir.join("roots.json");

    if let Ok(txt) = std::fs::read_to_string(&cfg)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt)
            && let Some(arr) = v.as_array() {
                let roots: Vec<Root> = arr
                    .iter()
                    .filter_map(|e| {
                        Some(Root {
                            label: e.get("label")?.as_str()?.to_string(),
                            path: PathBuf::from(
                                e.get("path")?
                                    .as_str()?
                                    // Only a LEADING ~ is the home directory.
                                    .strip_prefix('~')
                                    .map(|rest| format!("{}{rest}", home.to_string_lossy()))
                                    .unwrap_or_else(|| e["path"].as_str().unwrap_or("").into()),
                            ),
                        })
                    })
                    .collect();
                if !roots.is_empty() {
                    return roots;
                }
            }

    let found = discover(&home);
    let _ = std::fs::create_dir_all(&cfg_dir);
    let seed: Vec<serde_json::Value> = found
        .iter()
        .map(|r| {
            let p = r.path.to_string_lossy().to_string();
            let shown = p
                .strip_prefix(&home.to_string_lossy().to_string())
                .map(|rest| format!("~{rest}"))
                .unwrap_or(p);
            serde_json::json!({ "label": r.label, "path": shown })
        })
        .collect();
    let _ = std::fs::write(
        &cfg,
        serde_json::to_string_pretty(&seed).unwrap_or_default() + "\n",
    );
    found
}
