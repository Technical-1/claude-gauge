# Design — dropdown, naming, and session counts

**Status: BUILT 2026-08-29.** All four steps implemented and verified.
Later additions — session submenu, burn rate, token counts, start-at-login,
and hiding zero-value non-core rows — are documented in `README.md`. Written 2026-08-29 from a live
screenshot of v1.0.0's dropdown. All design decisions are settled except Q6,
which can only be answered by building.

## Decided 2026-08-29

| | Decision |
|---|---|
| Menubar | `① 7%·2  ② 100%·13  ③ 4%` — outline circled digit (U+2460, 1–20), `·` before session count, `[n]` fallback outside range |
| Dropdown | Compact, 3 rows per account, keep `↻` for resets |
| Opaque windows | **Hide unless non-zero** — invisible at 0%, appear automatically if one ever becomes meaningful |
| Second instance | Notify "already running", then exit |
| States | Short words: `7%~` stale · `--` not signed in · `exp` expired · `429` rate limited · `err` other |
| Unknown session count | **Omit the suffix.** `[3] 4%` = unknown, `[3] 4%·0` = genuinely zero |
| `FULL` badge | **None.** `100%` is already unmistakable; a badge adds a threshold that can disagree with intuition at 96% |

"Hide unless non-zero" is the better of the two hiding options for a specific
reason: it cannot leave you blind. A static hide-list would make a genuinely new
limit invisible forever; this shows it the moment it carries a value. The cost is
that a row can appear without warning, which is the correct trade for a meter.

## What is wrong today

From the screenshot, per account:

```
1⃣ claude — session 7%  ↻ 3h 18m
      five_hour            7%   ↻ 3h 18m
      session              7%   ↻ 3h 18m      <- duplicate of five_hour
      seven_day            2%   ↻ 6d 14h
      weekly_all           2%   ↻ 6d 14h      <- duplicate of seven_day
    · nimbus_quill         0%                 <- opaque, always 0
      weekly_scoped:Fable  0%
```

Seven lines per account, 21 lines total, to convey six real numbers.

| # | Problem | Severity |
|---|---|---|
| P1 | `session` duplicates `five_hour`; `weekly_all` duplicates `seven_day` | noise, halves the signal |
| P2 | `nimbus_quill` is opaque and permanently `0%` | noise |
| P3 | Raw API key names (`five_hour`, `weekly_scoped:Fable`) leak into the UI | unpolished |
| P4 | Emoji keycaps `1⃣2⃣3⃣` | disliked; want ASCII |
| P5 | No session count per account | the leading indicator is missing |
| P6 | Nothing stops a second instance | two menubar items |

## Measured evidence for P1

Grouping cached windows by `(pct, resets_at)` across all three accounts:

```
claude    7.0%  reset 2026-08-29T21:29:59   five_hour, session
claude    2.0%  reset 2026-09-05T08:59:59   seven_day, weekly_all
claude2 100.0%  reset 2026-08-31T01:00:00   seven_day, weekly_all
claude3   4.0%  reset 2026-08-29T22:00:00   five_hour, session
```

Consistent on every account. `five_hour` arrives as a top-level object carrying
`utilization`; `session` arrives as a `limits[]` entry with `kind: "session"`.
Same underlying limit, reported twice by the API.

### Dedup by identity, never by value

`nimbus_quill` and `weekly_scoped:Fable` **also** group together right now — both
`0%`, both no reset. They are unrelated limits that happen to read the same today.

A rule like "merge windows with equal pct and reset" would collapse them now and
split them the moment Fable usage becomes non-zero, so rows would appear and
disappear between refreshes. The alias set must therefore be an explicit map:

```
session      -> five_hour       (alias, drop the duplicate)
weekly_all   -> seven_day       (alias, drop the duplicate)
```

Anything not in the map is shown. **Unknown keys fail visible, not hidden** — a
new limit the API starts returning must surface rather than be silently dropped.

## Proposed display names (P3)

| API key | Display | Notes |
|---|---|---|
| `five_hour` (+ `session`) | **5-hour** | the one that gates you day to day |
| `seven_day` (+ `weekly_all`) | **Weekly** | |
| `seven_day_opus` | **Weekly · Opus** | not currently returned, handle anyway |
| `weekly_scoped:<Model>` | **Weekly · \<Model\>** | e.g. `Weekly · Fable` |
| `nimbus_quill`, `amber_ladder`, `cinder_cove`, `tangelo` | *hidden while 0%* | shown automatically if ever non-zero |
| anything unrecognised | shown, key as-is | fail visible |

## Agreed layout

```
 ① claude                              2 sessions
       5-hour       7%   ↻ 3h 18m
       Weekly       2%   ↻ 6d 14h
 ────────────────────────────────────────────────
 ② claude2                            13 sessions
       5-hour       3%   ↻ 3h 48m
       Weekly     100%   ↻ 1d 6h
 ────────────────────────────────────────────────
 [3] claude3
       5-hour       4%   ↻ 3h 48m
       Weekly       1%   ↻ 23h 48m
 ────────────────────────────────────────────────
 Refresh now
 About Claude Gauge
 Quit
```

7 rows per account → 3. Note `[3]` carries **no** session text: that is the
"detection could not determine a count" case, deliberately distinct from
`0 sessions`.

Menubar: `① 7%·2  ② 100%·13  ③ 4%`

**Q6 remains open and is answerable only on a real build.** macOS menu items
render in the proportional system font, so the column alignment above may not
hold. Inline bars were rejected partly for this reason. **Verify alignment first
in the build loop, before any other polish** — if it does not hold, the column
positions are the thing to change, not the content.

## Session counts (P5)

Source: enumerate `claude` processes, read `CLAUDE_CONFIG_DIR` from each via
`ps -Eww`. Unset means the default root. Map the path back to a configured root
using the same `sha256[:8]` identity rule as `accounts.rs:32`.

**This is the riskiest item and the only one that can be silently wrong.** A
first attempt during investigation found the 13 sessions carrying the env var but
**zero** default-root sessions, because those have it unset and the detection did
not account for that. A miscount actively misleads; a missing meter merely
annoys. Needs a verification pass against a known-good count before shipping.

No network cost. Refreshed on the same 300s tick as everything else.

## Single-instance guard (P6)

`flock` on `~/.config/claude-gauge/instance.lock`. If the lock is held, exit
immediately rather than adding a second menubar item.

Note this is *defence in depth*, not the primary rate-limit fix: because the
cache lives on disk, a second instance already costs ~0 extra requests (observed
2026-08-29 — a second copy started 26s after the first and spent zero). The guard
is about the confusing UI, not the request budget.

Notifies "Claude Gauge is already running", then exits.

## Gap 1 — the non-OK states have no ASCII design

The layout above only shows healthy accounts. Every other state also has to
render, and the current forms are Unicode/emoji that the ASCII decision touches:

| State | Today | Meaning |
|---|---|---|
| `Ok` | `1⃣ 7%` | normal |
| `NotLoggedIn` | `1⃣ –` | no keychain entry |
| `Expired` | `1⃣ ↻` | access token expired — collides with the reset `↻` |
| `Http{429}` | `1⃣ ` | rate limited with no cached value |
| stale (backing off) | `1⃣ 7%~` | serving last good value |
| `Error` | `1⃣ !` | anything else |

**Decided** — short words:

```
  [1] 7%       ok
  [1] 7%~      stale, backing off after a 429
  [1] --       not signed in
  [1] exp      token expired  (avoids reusing ↻, which now means "resets")
  [1] 429      rate limited, no cached value to show
  [1] err      other error
```

`Expired` currently renders as `↻`, and the dropdown uses `↻` for reset times.
Once both are on screen the same glyph means two different things. Worth fixing
regardless of the ASCII decision.

## Gap 2 — "0 sessions" vs "cannot tell"

`[3] 4%·0` and "session detection failed" must not look the same. A confident `·0`
when the count is actually unknown is exactly the silent-lie failure mode flagged
under P5.

**Decided** — omit the suffix entirely when the count is unknown
(`[3] 4%`), and show `·0` only when enumeration succeeded and genuinely found
none. Same in the dropdown: `0 sessions` vs no session text at all.

## Open questions

- **Q6** — proportional-font alignment. Still open: the text is correct (verified
  via `--menu`), but whether the columns *visually* line up in the macOS menu can
  only be judged by looking at the running app. Check this first after relaunch.

Q1–Q5, Q7, Q8 are decided; see the table at the top.

`WALLED_PCT` (95) still governs the **reset notification**, which is unrelated
to the dropped `FULL` badge. Dropping the badge does not mean dropping that
threshold — they were separate uses of the same number and only one was retired.

## Build order, once decided

1. Naming + dedup (P1–P3) — pure display logic, no new data sources
2. ASCII forms (P4) — trivial once the above lands
3. Single-instance guard (P6) — self-contained
4. Session counts (P5) — last, because it is the only one that can lie

Each step is independently verifiable, and the risky one goes last so there is a
known-good meter to check it against.
