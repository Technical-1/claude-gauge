//! "Start at login", via a LaunchAgent.
//!
//! The plist points at `current_exe()`, not a hardcoded path — so enabling it
//! from the dist/ build autostarts dist/, and enabling it from /Applications
//! autostarts /Applications. Whichever copy you turned it on from is the one
//! that comes back, which is the only behaviour that is not surprising.
//!
//! CLASS: actuator. Every failure is returned, never swallowed.

use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.technical1.claude-usage";

fn plist_path() -> PathBuf {
    crate::accounts::dirs_home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

pub fn is_enabled() -> bool {
    plist_path().exists()
}

pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = plist_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // KeepAlive is the DICT form, not <true/>. A bare KeepAlive relaunches the
    // app the instant the Quit menu item calls exit(0), which makes Quit look
    // broken. SuccessfulExit=false restarts only after a crash.
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
</dict></plist>
"#,
        exe.display()
    );
    std::fs::write(&path, plist).map_err(|e| e.to_string())?;

    let uid = unsafe { libc::getuid() };
    // bootout first so re-enabling after a path change actually takes effect;
    // bootstrap on an already-loaded label is an error, not a no-op.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .output();
    let out = Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        // Leave no half-state: if launchd refused, the plist should not linger
        // claiming the feature is on.
        let _ = std::fs::remove_file(&path);
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn disable() -> Result<(), String> {
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .output();
    let path = plist_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
