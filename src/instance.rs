//! Single-instance guard.
//!
//! WHY flock RATHER THAN A PID FILE
//!
//! An advisory lock on an open fd is released by the kernel when the process
//! ends — including on crash, SIGKILL, or a force-quit from Activity Monitor.
//! A PID file survives all three, so it needs stale detection, and that
//! detection is itself unreliable once PIDs are recycled. The lock cannot go
//! stale, so there is nothing to detect.
//!
//! CLASS: gate, but a deliberately soft one. If the lock cannot be created at
//! all (unwritable home, exotic filesystem) we let the app start rather than
//! refuse to run — a duplicate menubar item is a far smaller harm than a meter
//! that will not launch.

use std::fs::File;
use std::os::unix::io::AsRawFd;

/// Held for the lifetime of the process. Never dropped early — binding it to
/// `_guard` in main keeps it alive; `let _ = acquire()` would release it at once.
pub struct Guard(#[allow(dead_code)] File);

/// `Ok(guard)` — we are the only instance.
/// `Err(())`  — another instance already holds the lock.
pub fn acquire() -> Result<Guard, ()> {
    let dir = crate::accounts::dirs_home().join(".config/claude-gauge");
    let _ = std::fs::create_dir_all(&dir);
    let Ok(f) = File::create(dir.join("instance.lock")) else {
        // Cannot lock at all — fail open, see CLASS note above.
        return Ok(Guard(File::open("/dev/null").expect("/dev/null")));
    };
    // LOCK_NB: report the conflict immediately instead of blocking forever
    // behind the running instance.
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 { Ok(Guard(f)) } else { Err(()) }
}
