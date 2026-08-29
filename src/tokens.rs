//! Token totals per account, read from Claude Code's own transcripts.
//!
//! Every assistant message in `<root>/projects/<slug>/<session>.jsonl` carries a
//! `usage` block. Summing it gives real token counts the usage endpoint does not
//! expose.
//!
//! WINDOWED, NOT TOTAL. The full corpus is ~2.7GB across ~8,400 files; scanning
//! it on every refresh is out of the question. Only files modified inside the
//! window are opened — measured 2026-08-29, that is 16 files / 21MB / 0.11s for a
//! 5-hour window, which is nothing.
//!
//! THESE NUMBERS DO NOT TRACK THE QUOTA PERCENTAGE. Quota is weighted by model
//! and caching in ways the transcripts do not expose. They are complementary
//! figures, not two views of the same thing. Do not try to derive one from the other.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct Tokens {
    /// Tokens newly processed: `input_tokens` + `cache_creation_input_tokens`.
    pub input: u64,
    pub output: u64,
}

/// `cache_read_input_tokens` is deliberately EXCLUDED.
///
/// Measured on one account's 5-hour window: output 4.9M, cache_write 24M,
/// cache_read 751M. Cache reads outnumber everything else by ~150x, so including
/// them would produce a number that mostly measures cache hits and moves for
/// reasons unrelated to how much work was done.
fn add_usage(t: &mut Tokens, u: &serde_json::Value) {
    let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    t.input += g("input_tokens") + g("cache_creation_input_tokens");
    t.output += g("output_tokens");
}

/// Every `.jsonl` under `<root>/projects`, at any depth.
///
/// Must recurse. Transcripts are NOT all two levels deep: subagent runs live at
/// `projects/<slug>/<session-id>/subagents/agent-*.jsonl` and carry their own
/// usage blocks. A `projects/*/*.jsonl` walk missed 2,270 files in one root and
/// 527 in another — a silent undercount that grows with subagent use.
pub fn transcripts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("projects")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file()
                    && path.extension().and_then(|x| x.to_str()) == Some("jsonl") =>
                {
                    out.push(path)
                }
                _ => {}
            }
        }
    }
    out
}

/// Sum the usage blocks in `text` into `total`.
pub fn sum_into(total: &mut Tokens, text: &str) {
    for line in text.lines() {
        // Cheap reject before paying for a JSON parse: most lines in a transcript
        // are not assistant messages and carry no usage block.
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
            add_usage(total, u);
        }
    }
}

/// Sum usage across transcripts touched since `cutoff` (epoch seconds).
pub fn since(root: &Path, cutoff: i64) -> Tokens {
    let mut total = Tokens::default();
    for path in transcripts(root) {
        let recent = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_secs() as i64 >= cutoff);
        if !recent {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            sum_into(&mut total, &text);
        }
    }
    total
}

/// 776_000_000 -> "776M". Menu width is scarce; exact digits are not the point.
pub fn human(n: u64) -> String {
    match n {
        0..=999 => format!("{n}"),
        1_000..=999_999 => format!("{:.0}k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}
