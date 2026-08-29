//! macOS notifications, for the one event worth interrupting you: a walled
//! account becoming usable again.
//!
//! `osascript` rather than a notification crate — zero dependencies, and the
//! app already shells out to `security`, so this adds no new capability.

/// Post a notification. Best-effort: a failed notification must never affect
/// the meter, so every error is swallowed.
///
/// Arguments are passed through `on run argv` rather than interpolated into
/// the AppleScript source. Account labels come from a user-edited config file,
/// and building a script by string concatenation would let a label containing a
/// quote change the meaning of the script rather than just its text.
pub fn post(title: &str, body: &str) {
    let script = r#"on run argv
    display notification (item 1 of argv) with title (item 2 of argv)
end run"#;
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .arg(body)
        .arg(title)
        .output();
}
