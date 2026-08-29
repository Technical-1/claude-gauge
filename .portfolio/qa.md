# Project Q&A

## Overview

A macOS menubar meter that shows quota for several Claude Code accounts at once.
Each account has its own separate quota pool, and nothing in the product shows
them together — the in-session status line only ever knows about the account
whose session it is drawing. This sits in the menubar and answers the question
you actually have: **which account has room right now.** The interesting part is
that it is deliberately read-only against a credential store it does not own, and
that most of what it displays costs no network requests at all.

## Problem Solved

Running several accounts in parallel means quota exhaustion arrives without
warning: you find out you are out of headroom when work stops, and you have no
idea which of your other accounts to move to. The information exists — it is just
scattered across separate Keychain entries and only visible one account at a time
from inside a session. This puts all of it in one glance.

## Target Users

- **Anyone running more than one Claude Code account** — sees at a glance which
  account has headroom, and which is about to run out
- **Anyone running many concurrent sessions** — sees how many sessions are on each
  account, and can jump straight to the one they want

## Key Features

### One field per account in the menubar
`⑴ 22%  ⑵ 100%  ⑶ 6%` — the worst gauged window for each account. Every failure is
a distinct readout with a different fix: `--` means not signed in, `exp` means the
token expired, `429` means rate-limited, `7%~` means the meter is serving the last
good value while it backs off.

### A session list per account, with click-to-focus
Each account opens a submenu of its running sessions, named by project directory
and session title, with a live working/idle indicator. Clicking one brings its
terminal window to the front.

### Burn rate with a projection
When an account is measurably filling, the menu shows `▲ 24%/hr · full in 3h 29m`;
when it is measured and flat, `— steady`; when there is not yet enough history,
nothing at all. A percentage tells you where you are; a rate tells you whether an
account will survive the next two hours — and the third state matters because
"measured and safe" must not look like "no idea yet".

### A lifetime token odometer
A running total of every token ever processed across all accounts — data the usage
endpoint does not expose at all. Deliberately the *only* token figure in the UI: a
per-account token count is a magnitude with no decision attached, so it was
removed in favour of the burn rate, which answers an actual question.

## Technical Highlights

### Reporting an expired token instead of a rate limit
The usage endpoint returns **429 for an expired access token**, not 401. Handled
naively, every account with a stale credential reads as "rate limited", and you
switch away from an account that was actually fine. `poll()` in `src/accounts.rs`
therefore checks the token's `expiresAt` *before* spending a request, so expiry is
reported as its own state with its own fix. Ordering the check before the network
call is the entire fix, and it only works in that order.

### Surviving rate limits instead of amplifying them
The first version polled every 60 seconds per account with no backoff, so a single
429 triggered a retry every 60 seconds and sustained the condition indefinitely.
`src/cache.rs` now holds a per-account freshness floor shared by every caller, an
exponential backoff that honours `Retry-After`, and the last good reading so a
transient refusal never blanks the display. The Refresh menu item deliberately
bypasses the freshness floor but **not** the backoff.

### Joining processes to windows through the tty
Listing sessions by name, and raising the right window on click, requires matching
a running process to a terminal tab. `src/sessions.rs` and `src/terminal.rs` do
this by tty: `ps -o tty=` gives one per process, and Terminal exposes a title per
tab keyed by the same tty. The direct alternatives fail — transcripts are appended
and closed rather than held open, so `lsof` reveals nothing, and "newest transcript
in this folder" is ambiguous precisely when several sessions share a directory.

### An odometer that is incremental and never rewinds
A lifetime token total cannot re-parse a gigabyte of transcripts every five
minutes. `src/odometer.rs` exploits the fact that transcripts are append-only:
it records a byte offset per file and reads only what is new, taking 5.4s on the
first pass and 0.6s afterwards. Two details matter — reading stops at the last
*complete* line, because a file being written can end mid-line and advancing past
it would drop those tokens permanently; and a deleted transcript keeps its
contribution, because an odometer measures what was spent, not what still exists.

### Counting sessions without counting the wrong things
Matching processes by command line over-counts badly: shell processes inherit the
environment variable that identifies an account, so a single session can appear as
several. `src/sessions.rs` matches on `comm` being exactly the CLI's name, which
was verified against a per-process check before shipping. The count is an
`Option`, and a failure to enumerate renders as *nothing* rather than as zero —
a leading indicator that lies is worse than one that is absent.

## Engineering Decisions

### Read-only against a credential store I do not own
- **Constraint**: The stored credential contains a refresh token, and expired
  access tokens are common.
- **Options**: Refresh transparently; refresh and write the new pair back; or
  refuse to refresh at all.
- **Choice**: Never write to the Keychain, never refresh.
- **Why**: Refresh tokens rotate. Spending one without persisting the new pair
  invalidates the credential its owner holds — a read-only status display would
  silently sign you out of the account it reports on. Writing it back instead
  means racing another process for its own credential store. A meter is not worth
  either risk, so expiry became a displayed state with a one-step fix.

### Polling on a timer rather than consuming a push feed
- **Constraint**: The tool being measured hands live quota data to its status line
  on every render, for free. Consuming that would eliminate all network requests.
- **Options**: Register as a status-line command and cache what gets pushed; keep
  polling on a timer; or do both.
- **Choice**: Keep polling.
- **Why**: The push only fires while a session is actively rendering. An idle
  account would stop reporting and its cached value would silently rot — and an
  idle account is exactly when you most need to know it has room. A menubar meter
  is ambient: it must be correct when nobody is looking, which is the one thing a
  render-triggered feed cannot promise. The measured cost of polling is 36
  requests an hour.

### A config file seeded by discovery, rather than continuous discovery
- **Constraint**: Accounts are directories on disk, and retired ones often remain
  after they stop being used.
- **Options**: Discover every candidate on each run; hardcode a list; or discover
  once and hand the result to the user.
- **Choice**: Discovery seeds a JSON config on first run, and never touches it
  again.
- **Why**: A meter that lists dead accounts trains you to ignore it. Continuous
  discovery cannot tell a retired root from an active one, but a human can — so
  the program makes the guess once and the user owns the answer.

### An icon authored in HTML rather than SVG
- **Constraint**: The app needed an icon, and the artwork is a gauge with a
  gradient arc, tick marks and a needle — all positioned trigonometrically.
- **Options**: Hand-write SVG path data; use a vector editor; or generate it.
- **Choice**: An HTML file that computes the geometry in JavaScript, rasterised at
  2048px by headless Chrome and downsampled into an `.iconset`.
- **Why**: The arc endpoints and twenty tick positions are calculated, not drawn.
  Writing them as code means changing the needle position is editing one constant,
  and the source stays reviewable — an SVG with the same content is a wall of
  precomputed coordinates that cannot be adjusted without recomputing them.

## Frequently Asked Questions

### Does this ever change my sign-in state?
No. It reads Keychain entries and never writes them, and it never performs a token
refresh. The worst it can do to an account is spend a rate-limited read.

### Why does an account show `429` when its quota is nearly empty?
Because the usage endpoint rate-limits on how often you *ask*, independently of
how much quota you have consumed. Seeing `429` at 1% utilisation means too many
requests, not too little headroom — which is why the app enforces a shared
freshness floor and backs off rather than retrying.

### How does it know which account a session belongs to?
Each session inherits an environment variable naming its config directory, which
the app reads from the process table. An unset variable means the default account
— that is how the tool itself addresses it.

### Why do the token counts not match the quota percentage?
They measure different things and one cannot be derived from the other. Quota is
weighted by model and caching in ways the transcripts do not expose. The token
figures also deliberately exclude cache reads, which outnumber everything else by
roughly 150× and would otherwise produce a number that mostly tracks cache hits.

### Can I click a session that is not in a terminal window?
No, and it is shown greyed out rather than hidden. Sessions started without a
controlling terminal still consume quota, so hiding them would make the submenu
disagree with the account's session count.

### What happens if I run two copies?
The second one notices the first holds a lock file and exits with a notification
instead of adding a duplicate menubar item. The lock is an advisory `flock` rather
than a PID file, so it cannot go stale — the kernel releases it even if the app is
force-quit.

### Why is there no window, not even for About?
The app runs with an accessory activation policy, so it has no Dock icon and no
window. The About panel is macOS's own standard panel, which the menu library can
open directly — that removed the last reason to create a window at all.

### How do I add or remove an account?
Edit `~/.config/claude-usage/roots.json`. It is seeded on first run from whatever
config directories exist and is never rewritten afterwards, so entries you delete
stay deleted.
