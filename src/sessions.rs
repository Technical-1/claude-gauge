//! Counting running Claude Code sessions per account.
//!
//! THE FAILURE MODE THIS MODULE EXISTS TO AVOID
//!
//! A wrong count is worse than no count. Quota is a lagging indicator — it tells
//! you what you already spent. Session count is a leading one, and a leading
//! indicator that lies sends you to the wrong account.
//!
//! An early attempt matched processes by command line (`pgrep -f`), which counted
//! shell wrappers that merely inherit `CLAUDE_CONFIG_DIR` from their parent.
//! Measured 2026-08-29: it reported 13 sessions on one account where there were
//! 5. Matching `comm` exactly gives 10 total, split 4/5/1, which agrees with a
//! per-process check.
//!
//! CLASS: sensor. Returns None rather than guessing; callers must not render
//! None as zero.

use crate::accounts::Root;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// One running Claude Code session.
#[derive(Debug, Clone)]
pub struct Session {
    pub root_label: String,
    /// `None` for a session with no controlling terminal — a headless one, e.g.
    /// started by usage-guard's resume watcher. It still burns quota, so it is
    /// listed; it just cannot be raised.
    pub tty: Option<String>,
    pub cwd: Option<String>,
    /// Terminal tab title, which Claude Code sets to the session title.
    pub title: Option<String>,
    /// Whether clicking this session can actually bring something to the front.
    ///
    /// False for a headless session (no tty at all) AND for one whose tty exists
    /// but has no Terminal tab — tmux, ssh, another terminal app. Stays TRUE when
    /// Terminal did not answer, so a denied Automation permission produces an
    /// explanatory click rather than a menu of silently dead items.
    pub raisable: bool,
}

impl Session {
    /// "◐ claude-gauge — Handoff doc and menu bar app"
    ///
    /// Claude's title already begins with a status glyph (✳ idle, ◐/◑ working),
    /// so it is lifted to the front rather than left stranded mid-string after
    /// the directory.
    pub fn label(&self) -> String {
        let dir = self
            .cwd
            .as_deref()
            .and_then(|c| c.rsplit('/').next())
            .unwrap_or("?");
        match &self.title {
            Some(t) => {
                let (glyph, rest) = split_glyph(t);
                if rest.is_empty() {
                    format!("{glyph}{dir}")
                } else {
                    format!("{glyph}{dir} — {rest}")
                }
            }
            None => format!("{dir}  (no terminal)"),
        }
    }
}

/// Split a leading status glyph off a tab title: "✳ Foo" -> ("✳ ", "Foo").
fn split_glyph(title: &str) -> (String, String) {
    let mut chars = title.chars();
    match chars.next() {
        Some(c) if !c.is_alphanumeric() && c != '/' => {
            (format!("{c} "), chars.as_str().trim().to_string())
        }
        _ => (String::new(), title.trim().to_string()),
    }
}

/// Working directory per pid, in ONE `lsof` call rather than one per process.
fn cwds(pids: &[u32]) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    if pids.is_empty() {
        return out;
    }
    let csv = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let Ok(o) = Command::new("lsof")
        .args(["-a", "-p", &csv, "-d", "cwd", "-Fpn"])
        .output()
    else {
        return out;
    };
    // -F output is one field per line: `p<pid>`, then `f<fd>`, then `n<path>`.
    let mut cur: Option<u32> = None;
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        match line.as_bytes().first() {
            Some(b'p') => cur = line[1..].parse().ok(),
            Some(b'n') => {
                if let Some(pid) = cur {
                    out.entry(pid).or_insert_with(|| line[1..].to_string());
                }
            }
            _ => {}
        }
    }
    out
}

/// Every running session, attributed to a configured account.
/// `None` means enumeration failed — never "there are no sessions".
pub fn list(roots: &[Root]) -> Option<Vec<Session>> {
    let out = Command::new("ps")
        .args(["-Ewwo", "pid=,comm=,tty=,command="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || text.trim().is_empty() {
        return None;
    }

    let default_root = crate::accounts::dirs_home().join(".claude");
    let mut raw: Vec<(u32, Option<String>, String)> = Vec::new();

    for line in text.lines() {
        let mut f = line.split_whitespace();
        let Some(pid) = f.next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if f.next() != Some("claude") {
            continue;
        }
        // "??" is ps's marker for no controlling terminal.
        let tty = match f.next() {
            Some("??") | None => None,
            Some(t) => Some(t.to_string()),
        };
        let path = line
            .split_whitespace()
            .find_map(|t| t.strip_prefix("CLAUDE_CONFIG_DIR="))
            .map(PathBuf::from)
            .unwrap_or_else(|| default_root.clone());
        let Some(r) = roots.iter().find(|r| r.path == path) else {
            continue;
        };
        raw.push((pid, tty, r.label.clone()));
    }

    let pids: Vec<u32> = raw.iter().map(|(p, _, _)| *p).collect();
    let cwd_map = cwds(&pids);
    // One AppleScript call for every tab, rather than one per session.
    let title_map = crate::terminal::titles();
    let terminal_answered = title_map.is_some();

    Some(
        raw.into_iter()
            .map(|(pid, tty, root_label)| {
                let title = tty
                    .as_ref()
                    .and_then(|t| title_map.as_ref().and_then(|m| m.get(t).cloned()));
                let raisable = match (&tty, terminal_answered) {
                    (None, _) => false,             // headless — nothing to raise
                    (Some(_), true) => title.is_some(), // Terminal answered: tab exists or it doesn't
                    (Some(_), false) => true,       // Terminal silent — let the click explain
                };
                Session {
                    title,
                    cwd: cwd_map.get(&pid).cloned(),
                    tty,
                    root_label,
                    raisable,
                }
            })
            .collect(),
    )
}
