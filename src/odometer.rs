//! A lifetime token odometer: total tokens ever processed, across every account.
//!
//! INCREMENTAL BY BYTE OFFSET
//!
//! Transcripts are append-only, so after the first pass only the newly appended
//! bytes need reading. The state file records how far into each transcript we
//! have already counted; a later run seeks there and reads the remainder.
//!
//! The alternative — rescanning everything each refresh — is affordable today
//! (~1.2GB, a few seconds) but grows without bound, and it would be several
//! seconds of disk and CPU every five minutes forever.
//!
//! AN ODOMETER NEVER GOES DOWN. A deleted transcript keeps its contribution:
//! those tokens really were spent, and a car's odometer does not un-wind when you
//! sell the tyres. Only the per-file offsets are pruned, never the totals.
//!
//! CLASS: sensor. Every failure is skipped, never raised.

use crate::accounts::Root;
use crate::tokens::Tokens;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    /// Absolute path -> bytes already counted.
    #[serde(default)]
    seen: HashMap<String, u64>,
}

fn state_file() -> PathBuf {
    crate::accounts::dirs_home().join(".config/claude-usage/odometer.json")
}

fn load() -> State {
    std::fs::read_to_string(state_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(s: &State) {
    let path = state_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(s) {
        let _ = std::fs::write(path, text);
    }
}

/// Read from `offset` to EOF, but only count through the last COMPLETE line.
///
/// Returning the full length here would be a silent data loss: a transcript
/// being written right now can end mid-line, that line fails to parse, and
/// advancing the offset past it means its tokens are never counted. Stopping at
/// the last newline leaves the partial line to be read whole next time.
fn read_tail(path: &PathBuf, offset: u64) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    let end = buf.rfind('\n').map(|i| i + 1)?;
    buf.truncate(end);
    Some((buf, offset + end as u64))
}

/// Fold any newly appended transcript data into the running totals.
pub fn update(roots: &[Root]) -> Tokens {
    let mut st = load();
    let mut present: HashMap<String, u64> = HashMap::new();
    let mut changed = false;

    for root in roots {
        for path in crate::tokens::transcripts(&root.path) {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let size = meta.len();
            let key = path.to_string_lossy().to_string();
            // Truncated or replaced: the recorded offset is meaningless, so start
            // over for this file rather than seek past the end.
            let offset = match st.seen.get(&key) {
                Some(&n) if n <= size => n,
                _ => 0,
            };
            present.insert(key.clone(), offset);
            if offset >= size {
                continue;
            }
            if let Some((text, new_offset)) = read_tail(&path, offset) {
                let mut delta = Tokens::default();
                crate::tokens::sum_into(&mut delta, &text);
                st.input += delta.input;
                st.output += delta.output;
                present.insert(key, new_offset);
                changed = true;
            }
        }
    }

    // Prune offsets for transcripts that no longer exist — bounding the state
    // file — while leaving their contribution in the totals.
    if present.len() != st.seen.len() {
        changed = true;
    }
    st.seen = present;

    // Only write when something actually moved. The state file carries an entry
    // per transcript (~1.2MB at 8,400 files) and this runs every 300s; saving
    // unconditionally would be ~345MB of identical rewrites per day for nothing.
    if changed {
        save(&st);
    }
    Tokens { input: st.input, output: st.output }
}
