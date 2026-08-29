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

use std::path::Path;

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

/// Sum usage across transcripts touched since `cutoff` (epoch seconds).
pub fn since(root: &Path, cutoff: i64) -> Tokens {
    let mut total = Tokens::default();
    let projects = root.join("projects");
    let Ok(dirs) = std::fs::read_dir(&projects) else {
        return total;
    };
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let recent = f
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64 >= cutoff)
                .unwrap_or(false);
            if !recent {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                // Cheap reject before paying for a JSON parse: most lines in a
                // transcript are not assistant messages and carry no usage block.
                if !line.contains("\"usage\"") {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                    add_usage(&mut total, u);
                }
            }
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
