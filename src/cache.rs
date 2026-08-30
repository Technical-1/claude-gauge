//! Per-account on-disk cache of the last good usage reading.
//!
//! WHY THIS EXISTS
//!
//! Before this, every path that wanted a number spent a request: the tray's
//! timer, `--list`, `--title`, and each click of "Refresh now". Three accounts
//! on a 60s timer is 180 requests/hour against `/api/oauth/usage` — sustained,
//! whether or not anyone is looking — and the app was the only thing on the
//! machine polling that endpoint on a clock. That is what produced 429s.
//!
//! The cache makes all of those paths share one budget, and gives a 429 somewhere
//! to fall back to so a transient refusal never blanks a known-good reading.

use crate::accounts::Root;
use crate::usage::Window;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub windows: Vec<Window>,
    /// Epoch seconds of the last SUCCESSFUL fetch.
    #[serde(default)]
    pub fetched_at: i64,
    /// Do not spend a request before this epoch second. Set on 429.
    #[serde(default)]
    pub backoff_until: i64,
    /// Consecutive 429s, for exponential backoff when there is no Retry-After.
    #[serde(default)]
    pub strikes: u32,
    /// (epoch seconds, five_hour pct) samples, for the burn rate.
    /// Bounded — see HISTORY_MAX.
    #[serde(default)]
    pub history: Vec<(i64, f64)>,
    /// True once this account has been seen at or above WALLED_PCT. Persisted so
    /// the transition survives a restart — otherwise quitting the app while an
    /// account is walled would lose the very event we want to be told about.
    #[serde(default)]
    pub was_walled: bool,
}

/// ~2 hours at the 300s refresh. Enough span to measure a slow climb without
/// letting the file grow without bound.
const HISTORY_MAX: usize = 24;
/// Refuse to compute a rate across less than this. Quota readings dither by a
/// point or two (documented in usage-guard's FINDINGS), so a short span turns
/// noise into a confident-looking number — the same mistake that once armed a
/// gate at 38%.
const MIN_SPAN_S: i64 = 900;

impl Entry {
    pub fn push_history(&mut self, now: i64, pct: f64) {
        self.history.push((now, pct));
        let n = self.history.len();
        if n > HISTORY_MAX {
            self.history.drain(..n - HISTORY_MAX);
        }
    }

    /// Percentage points per hour, and seconds until 100% at that rate.
    ///
    /// `None` when there is nothing worth saying: too few samples, too short a
    /// span, or not measurably rising. A row that reports "nothing is happening"
    /// is clutter, and a rate derived from two adjacent noisy samples is worse
    /// than no rate at all — which is why the span floor exists.
    pub fn burn(&self) -> Option<(f64, Option<i64>)> {
        if self.history.len() < 3 {
            return None;
        }
        let (t0, p0) = *self.history.first()?;
        let (t1, p1) = *self.history.last()?;
        let span = t1 - t0;
        if span < MIN_SPAN_S {
            return None;
        }
        let rate = (p1 - p0) / (span as f64 / 3600.0);
        // Quota does not un-consume, so a negative rate is a window reset or
        // noise, never a real decline. Flat is not news; say nothing.
        if rate < 0.5 {
            return None;
        }
        let remaining = 100.0 - p1;
        let eta = if remaining > 0.0 {
            Some((remaining / rate * 3600.0) as i64)
        } else {
            None
        };
        Some((rate, eta))
    }

    pub fn age(&self, now: i64) -> i64 {
        now.saturating_sub(self.fetched_at)
    }
    pub fn has_data(&self) -> bool {
        !self.windows.is_empty() && self.fetched_at > 0
    }
}

fn dir() -> PathBuf {
    crate::accounts::dirs_home().join(".config/claude-gauge/cache")
}

/// Keyed by the ROOT PATH, never by the label.
///
/// Labels come from a user-edited config file and can be renamed or reordered at
/// any time. Keying on one means that editing `roots.json` silently re-points a
/// cache file at a different account — and it carries more than a percentage:
/// `was_walled` would fire a reset notification for the wrong account, and
/// `history` would splice two accounts' samples into one burn rate.
///
/// Observed 2026-08-29 while reordering roots during testing: two accounts'
/// readings swapped and were served as fresh.
///
/// The path is the identity — the same fact that makes a second config root a
/// second account — so it is what the cache keys on. This is the same
/// `sha256(abs_path)[..8]` the Keychain service name uses.
fn file(root: &Root) -> PathBuf {
    let digest = Sha256::digest(root.path.to_string_lossy().as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    dir().join(format!("{hex}.json"))
}

pub fn load(root: &Root) -> Option<Entry> {
    let txt = std::fs::read_to_string(file(root)).ok()?;
    serde_json::from_str(&txt).ok()
}

/// Best-effort. A cache that cannot be written must never break the meter.
pub fn store(root: &Root, e: &Entry) {
    let _ = std::fs::create_dir_all(dir());
    if let Ok(txt) = serde_json::to_string(e) {
        let _ = std::fs::write(file(root), txt);
    }
}
