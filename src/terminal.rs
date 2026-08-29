//! Talking to Terminal.app via AppleScript: reading tab titles, and raising a tab.
//!
//! WHY tty IS THE JOIN KEY
//!
//! Claude Code writes the session title into the terminal tab, and Terminal
//! exposes it as `custom title` keyed by tty. `ps -o tty=` gives us the same tty
//! per pid, so pid → tty → title is exact.
//!
//! The obvious alternative does not work: `lsof` shows no open transcript
//! (Claude Code appends and closes), and falling back to "newest .jsonl in the
//! project folder" breaks exactly where it matters — several sessions can share
//! one cwd, with no way to tell which pid owns which file.
//!
//! Requires macOS Automation permission ("Claude Usage wants to control
//! Terminal"). Denial is reported, never silent.
//!
//! CLASS: sensor for `titles`, actuator for `raise`. Neither may panic.

use std::collections::HashMap;
use std::process::Command;

fn osa(script: &str) -> Result<String, String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// tty (bare, e.g. "ttys003") -> tab title.
///
/// `None` means Terminal did not answer at all — not running, or Automation
/// permission denied. That is NOT the same as an empty map, which means Terminal
/// answered and has no tabs. Callers use the difference to decide whether an
/// unmatched session is genuinely unraisable or merely unknown.
///
/// The delimiter is " ::: ", not a tab. Inside `tell application "Terminal"`,
/// the word `tab` resolves to Terminal's *tab class*, not the tab character — a
/// name collision that silently emits the wrong separator.
pub fn titles() -> Option<HashMap<String, String>> {
    // Two BULK property fetches, then a loop over plain lists.
    //
    // The obvious version nests `repeat with t in tabs of w` and reads `tty of t`
    // and `custom title of t` inside it — which costs one Apple Event round-trip
    // PER PROPERTY ACCESS. At 13 tabs that is 26 round-trips and measured 362ms.
    // Asking for `tty of tabs of windows` returns every value in a single event;
    // the loop below then walks ordinary in-process lists. Measured 75ms for
    // byte-identical output — 4.8x faster, same data.
    let script = r#"tell application "Terminal"
  set ttys to tty of tabs of windows
  set titles to custom title of tabs of windows
  set out to ""
  repeat with wi from 1 to count of ttys
    set wt to item wi of ttys
    set wn to item wi of titles
    repeat with ti from 1 to count of wt
      set out to out & (item ti of wt) & " ::: " & (item ti of wn) & linefeed
    end repeat
  end repeat
  return out
end tell"#;
    let mut map = HashMap::new();
    let text = osa(script).ok()?;
    for line in text.lines() {
        if let Some((tty, title)) = line.split_once(" ::: ") {
            let bare = tty.trim().trim_start_matches("/dev/").to_string();
            map.insert(bare, title.trim().to_string());
        }
    }
    Some(map)
}

/// Bring the tab on `tty` to the front. Err carries a reason fit to show a human.
pub fn raise(tty: &str) -> Result<(), String> {
    // Acting on the tab reference directly. Taking `index of t` fails with a
    // -1700 coercion error, which is what a first attempt hit.
    let script = format!(
        r#"tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      if tty of t is "/dev/{tty}" then
        set selected of t to true
        set index of w to 1
        activate
        return "ok"
      end if
    end repeat
  end repeat
  return "notab"
end tell"#
    );
    match osa(&script) {
        Ok(r) if r.trim() == "ok" => Ok(()),
        Ok(_) => Err("no Terminal tab for that session".into()),
        Err(e) if e.contains("Not authorized") || e.contains("-1743") => Err(
            "Automation permission denied — enable it in System Settings ▸ Privacy & Security ▸ Automation".into(),
        ),
        Err(e) => Err(e),
    }
}
