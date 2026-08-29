# claude-usage

A macOS menubar meter for **several Claude Code accounts at once**.

Each Claude Code config root is a separate account with a separate quota pool, and
nothing shows them together — the in-session status line only ever knows about the
account whose session it is drawing. This answers the question you actually have:
**which account has room right now.**

```
  ⑴ 22%   ⑵ 100%   ⑶ 6%
```

One field per account. Click for per-window detail, running sessions, and reset
countdowns.

## Features

- **One glance, every account** — worst gauged window per account in the menubar
- **Session list per account** — running sessions named by project and title, with
  a live working/idle mark; click one to bring its terminal window to the front
- **Burn rate** — `▲ 24%/hr on 5-hour · full in 2h 55m` when an account is
  measurably filling, with the projection suppressed if the window resets first
- **Lifetime odometer** — a running total of every token ever processed, across
  all accounts, at the bottom of the menu
- **Distinct failure states** — not-signed-in, expired, and rate-limited each read
  differently, because each has a different fix
- **Rate-limit aware** — a shared request budget, exponential backoff, and
  last-good values so a transient refusal never blanks the display
- **Read-only** — never writes to the Keychain, never refreshes a token

## Reading the meter

| Menubar | Meaning | Fix |
|---|---|---|
| `⑵ 99%` | worst gauged window for that account | switch accounts |
| `⑵ 99%~` | last good value; backing off after a 429 | wait, it self-heals |
| `⑶ --` | no keychain entry — not signed in | run that account once, then `/login` |
| `⑴ exp` | access token expired | open that account once |
| `⑵ 429` | rate limited with no cached value to fall back on | wait; it backs off automatically |
| `⑴ err` | other error | see the dropdown for the message |

Account numbers are circled digits (U+2460, covering 1–20). Anything outside
that range, or a non-numeric label, falls back to `[n]` — the emoji keycaps this
replaced had no fallback and rendered a stray box indistinguishable from an error.

## Tech Stack

- **Language**: Rust 2024
- **Menubar**: `tray-icon` 0.24 with `muda` menus, on a `tao` event loop
- **HTTP**: `ureq` 3.4, blocking
- **System**: macOS Keychain, AppleScript, `ps`/`lsof`, launchd

See [`.portfolio/stack.md`](.portfolio/stack.md) for the reasoning behind each.

## Getting Started

### Prerequisites

- macOS 11 or later
- Rust 1.90+ (2024 edition)
- At least one signed-in Claude Code config root

### Installation

```sh
cargo build --release
./build-app.sh --no-sign --install     # unsigned, local use
```

For a signed build, set your own Developer ID and a stored notarytool profile:

```sh
export SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export NOTARY_PROFILE="claude-usage"
./build-app.sh --install
```

### Usage

```sh
./target/release/claude-usage            # menubar app
./target/release/claude-usage --list     # headless table, then exit
./target/release/claude-usage --title    # the menubar string, then exit
./target/release/claude-usage --menu     # the dropdown's exact text, then exit
```

The headless modes share the same cache as the app, so they cost no requests
while it is running, and they keep working alongside it — only the GUI is
single-instance.

## Configuration

`~/.config/claude-usage/roots.json`, seeded on first run by discovering `~/.claude`
and any `~/.claude-*` directory carrying a `projects/` folder or a `settings.json`:

```json
[
  { "label": "claude",  "path": "~/.claude" },
  { "label": "claude2", "path": "~/.claude-work" },
  { "label": "claude3", "path": "~/.claude-3" }
]
```

After the first run the file is yours — it is never rewritten. A config file
rather than continuous discovery on purpose: a retired root often still exists on
disk, and a meter that lists dead accounts trains you to ignore it.

Labels are assigned positionally, not taken from the directory name. The
menubar tag comes from stripping the `claude` prefix, so a root named
`.claude-work` would render as `[-work]` rather than a number.

## How it finds credentials

Claude Code stores OAuth credentials in the macOS Keychain, keyed by **the config
root's path**:

| Root | Keychain service |
|---|---|
| `~/.claude` (default) | `Claude Code-credentials` |
| any other root | `Claude Code-credentials-<first 8 hex of sha256(absolute path)>` |

**The path is the identity** — which is why a second config root *is* a second
account, and why moving or renaming a config directory orphans its credentials.

Usage then comes from `GET https://api.anthropic.com/api/oauth/usage` with
`anthropic-beta: oauth-2025-04-20`.

## Read-only, deliberately

The app **never writes to the Keychain and never refreshes a token**, even though
the stored blob contains a `refreshToken` and expired access tokens are common.

Refresh tokens rotate. Spending one here without persisting the new pair back
would invalidate the credential Claude Code itself holds — this meter would
silently sign you out of the account it is reporting on. Writing it back instead
means racing Claude Code for its own credential store. Neither is worth it for a
status display, so an expired token is reported as a *state* (`exp`, "open it once
to refresh") rather than worked around.

**An expired token returns 429 from this endpoint, not 401.** So expiry is
checked *before* the request is spent — otherwise every stale account looks
rate-limited and you switch away from an account that was fine.

## Request budget

`/api/oauth/usage` returns **429 if you ask too often**, independently of how much
quota you have left. It was observed doing so while accounts sat near 0%.

An earlier version refreshed every 60s per account, described as "matching" a
related tool's minimum interval. That was a reasoning error worth recording: that
60 is a **floor on event-driven polls, not a timer** — the other tool only fetches
when an event fires. Copying the number turned a floor into a sustained 60
requests per hour per account, with no backoff, so one 429 sustained itself.

| Guard | Behaviour |
|---|---|
| Refresh interval | **300s**. Quota moves slowly. |
| Freshness floor | No account is re-polled within 120s. The timer, `--list`, `--title`, `--menu` and Refresh share one budget. |
| Backoff | A 429 honours `Retry-After`; otherwise 5m → 10m → 20m, capped at 30m. |
| Refresh button | Bypasses the freshness floor, **never** the backoff. |
| Stale-serving | A 429 shows the last good value with `~` rather than blanking. |
| Stagger | 400ms between accounts, skipped entirely on cache hits. |

Caches live in `~/.config/claude-usage/cache/`, keyed by `sha256(path)[..8]` —
**never by label**, since labels are user-editable and remapping one would
re-point another account's history and notification state. Every request actually
spent is appended to `~/.config/claude-usage/requests.log`.

## The dropdown

```
⑵ claude2               5 sessions  ▸
      ◑ FISH-THEME — Mobile menu button and pre-order cleanup
      ✳ ai-lab — AI project management at scale
      Weekly          100%   ↻ 1d 4h
      5-hour            6%   ↻ 1h 44m
      ▲ 24%/hr on 5-hour · full in 2h 55m
```

### Deduplicated windows

The API reports some limits twice — `session` is `five_hour`, `weekly_all` is
`seven_day` — so aliases are collapsed.

Collapsing is by **identity**, never by value. Two unrelated limits currently
share a percentage (both zero) and would be merged today, then split apart the
moment one becomes non-zero — rows appearing and vanishing between refreshes.

Two core windows always show, even at 0%, because `0%` there means "full
headroom". Everything else is hidden while zero and appears automatically if it
ever carries a value. The rule is not "hide zeros", it is "hide zeros that say
nothing".

### Sessions

Clicking a session raises its Terminal tab. `✳` is idle, `◐`/`◑` working — both
come free from the tab title.

**tty is the join key.** Claude Code writes the session title into the terminal
tab, and Terminal exposes it as `custom title` keyed by tty; `ps -o tty=` gives the
same tty per pid. The obvious routes fail — `lsof` shows no open transcript
(transcripts are appended and closed), and "newest transcript in the project
folder" breaks exactly where it matters, since several sessions can share one
working directory.

A session is clickable only when Terminal reports a tab for its tty. Sessions with
no controlling terminal, or running under tmux/ssh, are shown greyed out — they
still consume quota, so hiding them would make the submenu disagree with the
count. If Terminal does not answer at all, items stay clickable so the click can
explain that Automation permission is needed.

Requires macOS Automation permission, prompted on first click.

### Refresh cadence

Sessions refresh every **60s**, independently of quota's 300s — they cost no
requests, so there is no reason for them to wait on the request budget.

| Step | Cost |
|---|---|
| `ps` (pids, ttys, environment) | ~25 ms |
| `lsof` (working directory per pid) | ~53 ms |
| AppleScript (tty → tab title) | ~69 ms |
| **total** | **~147 ms, a 0.25% duty cycle at 60s** |

The AppleScript fetches properties in **bulk**. The obvious form nests
`repeat with t in tabs of w` and reads `tty of t` inside it, which costs one Apple
Event round-trip *per property access* — 26 of them across 13 tabs, measured at
362ms. `tty of tabs of windows` returns everything in one event; iterating the
resulting plain lists is free. Same output byte-for-byte, 4.8× faster.

### Burn rate

```
▲ 24%/hr on 5-hour · full in 2h 55m
```

Shown only when the five-hour window is measurably filling: at least 3 samples
spanning 15 minutes, rising faster than 0.5%/hr. Otherwise there is no row —
"nothing is happening" is not worth a line.

**The rate tracks `five_hour` specifically, not the worst window.** Tracking
the worst meant that on an account whose weekly was pinned at 100%, the rate
described the weekly — flat only because it was at the ceiling — while its
five-hour could be climbing unseen. The five-hour is also the only window that
moves fast enough for a 15-minute sample to say anything.

**The projection is dropped when the window resets first.** An account at 25%
rising 10%/hr whose window resets in 44 minutes was previously told it would be
"full in 7h 52m" — a wall it can never hit, because the window empties long
before. Quoting an exhaustion time past the reset projects a future that gets
cancelled.

The span floor is not optional. Quota readings dither by a point or two, and
differencing two adjacent samples turns that noise into a confident-looking
number. Quota also never un-consumes, so a negative rate is a window reset, never
information.

### Token counts

Tokens are read from `<root>/projects/**/*.jsonl`, which carry a `usage` block per
message. They feed the lifetime odometer below.

The walk must **recurse**. Transcripts are not all two levels deep: subagent
runs live at `projects/<slug>/<session-id>/subagents/agent-*.jsonl` and carry
their own usage blocks. A `projects/*/*.jsonl` walk silently missed 2,270 files in
one root and 527 in another.

`cache_read_input_tokens` is **excluded**. Measured on one 5-hour window: output
4.9M, cache_write 24M, cache_read 751M. Cache reads outnumber everything else by
~150×, so including them yields a number that mostly measures cache hits.

Token totals **do not track the quota percentage** and cannot be derived from
it — quota is weighted by model and caching in ways the transcripts do not expose.
That is why there is no per-account token row: a raw magnitude with no decision
attached is clutter. The burn rate above is the actionable number; the odometer
below is the cumulative one.

### The odometer

The bottom of the menu carries a lifetime total across every account:

```
Tokens all time      ↑ 2.1B   ↓ 291.7M
```

Transcripts are append-only, so it is incremental: `~/.config/claude-usage/odometer.json`
records how far into each transcript has already been counted, and later passes
read only the newly appended bytes. Measured: 5.4s for the first full pass over
8,400 files, 0.6s thereafter. The first pass runs on its own thread so the
menubar appears immediately rather than after a full-corpus scan.

**An odometer never goes down.** A deleted transcript keeps its contribution —
those tokens really were spent. Only the per-file offsets are pruned.

Reading stops at the last *complete* line. A transcript being written right now
can end mid-line; advancing past it would drop that line's tokens permanently.

## Run at login

A "Start at login" checkbox in the menu writes a LaunchAgent pointing at
`current_exe()` — whichever copy you enable it from is the one that comes back.

It uses the **dict** form of `KeepAlive`, not `<true/>`. A bare `KeepAlive`
relaunches the app the instant Quit calls `exit(0)`, which makes Quit look broken.
`SuccessfulExit: false` restarts only after a crash.

## Development

```sh
cargo build --release          # build
cargo clippy --release         # lint (clean at default level)
./target/release/claude-usage --menu    # verify the dropdown without a GUI
./build-app.sh --icon          # re-render assets/icon.icns from assets/icon.html
```

The icon is authored as HTML and rasterised at 2048px by headless Chrome, then
downsampled into an `.iconset`. The gauge geometry is computed rather than drawn,
so moving the needle is editing one constant.

## Project Structure

```
claude-usage/
├── src/
│   ├── main.rs         # tray, menu, event loop, headless modes
│   ├── accounts.rs     # config roots, Keychain, poll orchestration
│   ├── usage.rs        # the HTTP call, response parsing, window dedup
│   ├── cache.rs        # per-account cache, backoff, burn-rate history
│   ├── sessions.rs     # running-session enumeration and attribution
│   ├── terminal.rs     # AppleScript: read tab titles, raise a tab
│   ├── tokens.rs       # recursive transcript walk, usage parsing
│   ├── odometer.rs     # incremental lifetime token total
│   ├── notify.rs       # macOS notifications
│   ├── instance.rs     # single-instance guard (flock)
│   └── autostart.rs    # LaunchAgent write/remove
├── assets/
│   ├── icon.html       # icon source — computed geometry
│   └── icon.icns       # generated bundle icon
├── build-app.sh        # bundle, sign, notarize, staple, verify, install
└── .portfolio/         # architecture, stack, and Q&A write-ups
```

## License

MIT — see [`LICENSE`](LICENSE).

## Author

Jacob Kanfer — [GitHub](https://github.com/Technical-1)
