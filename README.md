# claude-usage

A macOS menubar meter for **several Claude Code accounts at once**.

Each Claude Code config root is a separate account with a separate quota pool, and
nothing in the product shows them together — the in-session statusline only ever
knows about the account whose session it is drawing. This answers the question you
actually have: **which account has room right now.**

```
  ① 11%·4   ② 100%·5   ③ 4%·1
     │   │
     │   └── running Claude Code sessions on that account
     └────── worst gauged window
```

One field per account. Click for per-window detail and reset countdowns.

## Build & run

```sh
cargo build --release
./target/release/claude-usage           # menubar app
./target/release/claude-usage --list    # headless, prints a table and exits
./target/release/claude-usage --title   # prints the menubar string and exits
```

`--list` is the same data path with no GUI, so it is also what you script against.

## Which accounts it shows

`~/.config/claude-usage/roots.json`, created on first run:

```json
[
  { "label": "claude",  "path": "~/.claude" },
  { "label": "claude2", "path": "~/.claude-work" },
  { "label": "claude3", "path": "~/.claude-3" }
]
```

Seeded on first run by discovering `~/.claude` and any `~/.claude-*` directory
that carries a `projects/` folder or a `settings.json`. After that the file is
yours — it is never rewritten.

A config file rather than continuous discovery on purpose: a retired root often
still exists on disk, and a meter that lists dead accounts trains you to ignore
it. Edit the file to add, drop, or rename one.

Labels are assigned positionally (`claude`, `claude2`, …), not taken from the
directory name. The menubar tag comes from stripping the `claude` prefix, so a
root named `.claude-work` would render as `[-work]` rather than a number. Rename
labels here if you want something else.

## How it finds each account's credentials

Claude Code stores OAuth credentials in the macOS Keychain, keyed by **the config
root's path**:

| Root | Keychain service |
|---|---|
| `~/.claude` (default) | `Claude Code-credentials` |
| any other root | `Claude Code-credentials-<first 8 hex of sha256(absolute path)>` |

Verified against a live keychain. This is also why a second config root *is* a
second account — **the path is the identity**, so moving or renaming a config
directory orphans its credentials.

Usage then comes from `GET https://api.anthropic.com/api/oauth/usage` with
`anthropic-beta: oauth-2025-04-20`.

## Read-only, deliberately

The app **never writes to the Keychain and never refreshes a token**, even though
the stored blob contains a `refreshToken` and expired access tokens are common.

Refresh tokens rotate. Spending one here without persisting the new pair back would
invalidate the credential Claude Code itself holds — this meter would silently log
you out of the account it is reporting on. Writing it back instead means racing
Claude Code for its own credential store. Neither is worth it for a status display,
so an expired token is reported as a *state* (`↻`, "open it once to refresh")
rather than worked around.

## Reading the states

| Menubar | Meaning | Fix |
|---|---|---|
| `② 99%` | worst gauged window for that account | switch accounts |
| `② 99%~` | last good value; the meter is backing off after a 429 | wait, it self-heals |
| `② 99%·5` | …with 5 Claude Code sessions running on it | |
| `② 99%` (no `·n`) | session count could not be determined | not the same as zero |
| `③ --` | no keychain entry — not signed in | run `claude3`, then `/login` |
| `① exp` | access token expired | open that account once |
| `② 429` | rate limited with no cached value to fall back on | wait; it backs off automatically |
| `① err` | other error | see the dropdown for the message |

Account numbers are outline circled digits (U+2460, covering 1–20). Anything
outside that range, or a non-numeric label, falls back to `[n]` — the emoji
keycaps this replaced had no fallback and emitted a stray box.

A missing `·n` suffix means **unknown**, not zero. `③ 4%·0` is a confident
zero; `③ 4%` means session enumeration did not produce an answer.

## The dropdown

```
① claude                4 sessions
      5-hour           10%   ↻ 2h 54m
      Weekly            3%   ↻ 6d 14h
      Weekly · Fable    0%
```

Three rows per account, not seven. The API reports some limits twice — `session`
is `five_hour`, `weekly_all` is `seven_day` — so aliases are collapsed.

Collapsing is by **identity**, never by value. `nimbus_quill` and
`weekly_scoped:Fable` currently share a percentage (0.0) and have no reset, but
are unrelated limits; a "merge rows that look alike" rule would fold them together
today and split them the moment Fable is used, so rows would appear and vanish
between refreshes.

Opaque codename windows (`nimbus_quill`, `amber_ladder`, `cinder_cove`,
`tangelo`) are hidden **while zero** rather than hidden outright, so one that ever
carries a real value cannot stay invisible.

## The session submenu

Each account header opens a submenu listing its running sessions:

```
⑵ claude2               5 sessions  ▸
      ◑ FISH-THEME — Mobile menu button and pre-order cleanup
      ✳ ai-lab — AI project management and memory retention at scale
```

Clicking one brings its Terminal tab to the front. `✳` is idle, `◐`/`◑` working —
both come free from the tab title.

**tty is the join key.** Claude Code writes the session title into the terminal
tab, and Terminal exposes it as `custom title` keyed by tty; `ps -o tty=` gives
the same tty per pid. The obvious route does not work — `lsof` shows no open
transcript (Claude Code appends and closes), and "newest `.jsonl` in the project
folder" breaks exactly where it matters, since several sessions can share one cwd.

A session is clickable only when Terminal reports a tab for its tty. Headless
sessions (no tty — e.g. started by usage-guard's resume watcher) and sessions in
tmux/ssh are shown greyed out; they still burn quota, so hiding them would make
the submenu disagree with the count. If Terminal does not answer at all, items
stay clickable so the click can explain that Automation permission is needed.

Requires macOS Automation permission, prompted on first click.

## Burn rate

```
      ▲ 24%/hr · full in 3h 29m
```

Shown only when a real rising trend is measured: at least 3 samples spanning
15 minutes, rising faster than 0.5%/hr.

The span floor is not optional. Quota readings dither by a point or two, and
differencing two adjacent samples turns that noise into a confident-looking
number — the failure that once armed usage-guard's Stop gate at 38%. Quota also
never un-consumes, so a negative rate is a window reset, never information.

## Token counts

```
      tokens 5h      ↑ 24.2M  ↓ 5.0M
```

Summed from `<root>/projects/**/*.jsonl`, which carry a `usage` block per message.

**Windowed, not total.** The full corpus is ~2.7GB across ~8,400 files. Only
files modified inside the window are opened — 16 files / 21MB / 0.11s for 5 hours.

`cache_read_input_tokens` is **excluded**. Measured on one 5-hour window:
output 4.9M, cache_write 24M, cache_read 751M. Cache reads outnumber everything
else by ~150×, so including them yields a number that mostly measures cache hits.

**These do not track the quota percentage** and cannot be derived from it.
Quota is weighted by model and caching in ways the transcripts do not expose.

## Start at login

A "Start at login" checkbox writes the LaunchAgent, pointing at
`current_exe()` — so whichever copy you enable it from is the one that comes back.

## Session counts

Counted from processes whose `comm` is exactly `claude`, attributed by each
process's `CLAUDE_CONFIG_DIR` (unset = the default root).

Matching the *command line* instead over-counts badly: shell wrappers inherit
the variable from their parent. Measured 2026-08-29, `pgrep -f` reported 13
sessions on an account that had 5. `comm` gives 10 total split 4/5/1, which
agrees with a per-process check.

**An expired token returns 429 from this endpoint, not 401.** So expiry is
checked *before* the request is spent — otherwise every stale account looks
rate-limited and you switch away from an account that was actually fine.

## What the response contains

Two shapes, both parsed:

1. Top-level objects carrying `utilization` — `five_hour`, `seven_day`,
   `seven_day_opus`, …
2. `limits[]`, carrying per-model weekly caps the top-level keys omit
   (`weekly_scoped:Fable`).

The payload also carries opaque, always-`0.0` keys with codenames
(`nimbus_quill`, `amber_ladder`, `cinder_cove`, `tangelo`). They are **shown but
never gauged** — marked `·` and excluded from the headline number, because
treating an undocumented always-zero field as headroom would be inventing precision
that is not there.

## Run at login

Not installed by default. To do it:

```sh
cat > ~/Library/LaunchAgents/com.technical1.claude-usage.plist <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.technical1.claude-usage</string>
  <key>ProgramArguments</key>
  <array><string>REPLACE_WITH_ABSOLUTE_PATH/target/release/claude-usage</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict></plist>
EOF
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.technical1.claude-usage.plist
```

Use the **dict** form of `KeepAlive` shown above, not `<true/>`. A bare
`KeepAlive` relaunches the app the instant the Quit menu item calls `exit(0)`,
which makes Quit look broken. `SuccessfulExit: false` restarts only after a crash.

## Request budget — why this app was the thing getting 429'd

`/api/oauth/usage` will return **429** if you ask it too often, and that is
unrelated to how much quota you have left. It was observed 429ing while both
accounts sat near 0% utilisation.

The original refresh interval was 60s, described here as "matching usage-guard's
`oauth_min_interval_s`". That was a reasoning error worth recording: **that 60 is a
floor on event-driven polls, not a timer.** usage-guard only fetches when a Stop
hook fires. Copying the number turned a floor into a sustained 60 requests per hour
per account — and nothing else on the machine polls that endpoint on a clock. The
statusline does not poll at all; Claude Code hands it `rate_limits` in the payload.

What the app does now:

| Guard | Behaviour |
|---|---|
| Refresh interval | **300s**, not 60s. Quota moves slowly. |
| Freshness floor | An account is never re-polled within `MIN_INTERVAL_S` (120s). The tray timer, `--list`, `--title` and Refresh all share one budget. |
| Backoff | A 429 honours `Retry-After`; otherwise 5m → 10m → 20m, capped at 30m. |
| Refresh button | Bypasses the freshness floor, **never** the backoff. Letting a button push through a backoff is how one 429 becomes a sustained one. |
| Stale-serving | A 429 shows the last good value with `~`, rather than blanking the meter. |
| Stagger | 400ms between accounts, so three requests are not a burst. Skipped entirely on cache hits. |

Cache lives at `~/.config/claude-usage/cache/<label>.json`. Every request the app
actually spends is appended to `~/.config/claude-usage/requests.log` — so "it kept
429ing" can be checked rather than guessed at.

## Packaging

`./build-app.sh` builds, signs, notarizes, staples and verifies the bundle.

```sh
./build-app.sh --no-sign     # unsigned bundle, for local iteration
./build-app.sh               # sign + notarize + staple + verify
./build-app.sh --install     # …and copy into /Applications
./build-app.sh --icon        # also re-render assets/icon.icns from icon.html
```

Signing requires your own Developer ID and a stored notarytool profile:

```sh
export SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export NOTARY_PROFILE="claude-usage"
xcrun notarytool store-credentials "claude-usage" \
  --key ~/path/AuthKey_XXXXXXXX.p8 --key-id XXXXXXXX --issuer <ISSUER-UUID>
```

Verify with `spctl`, not by launching it. A bundle always launches on the
machine that built it, notarized or not, so a successful launch proves nothing.

## License

MIT — see `LICENSE`.
