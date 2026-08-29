//! claude-usage — a macOS menubar meter for several Claude Code accounts at once.
//!
//! Each Claude Code config root is a separate account with a separate quota pool,
//! and nothing in the product shows you all of them together: the in-session
//! statusline only ever knows about the account whose session it is drawing. This
//! sits in the menubar and answers "which account has room right now".
//!
//! Read-only. See accounts.rs for why it never refreshes a token.

mod accounts;
mod autostart;
mod cache;
mod instance;
mod notify;
mod odometer;
mod sessions;
mod tokens;
mod terminal;
mod usage;

use accounts::{AccountStatus, State};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use std::collections::HashMap;
use tray_icon::menu::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::TrayIconBuilder;

/// This was 60s, "matching usage-guard's oauth_min_interval_s" — a reasoning
/// error. That 60 is a FLOOR on event-driven polls, not a timer: usage-guard only
/// fetches when a Stop hook fires. Copying the number turned it into a sustained
/// 60 requests/hour/account, and this app was the only thing on the machine
/// polling /api/oauth/usage on a clock. Quota moves slowly; 5 minutes is plenty.
const REFRESH: Duration = Duration::from_secs(300);

/// Sessions come from `ps`, `lsof` and one AppleScript call — no network, so they
/// are not tied to the request budget and refresh five times as often.
///
/// Not faster than this. Measured 2026-08-29: ps 29ms, lsof 19ms, and the
/// AppleScript round-trip to Terminal 367ms — the last is 8x the other two
/// combined, because it wakes Terminal and walks every window and tab. At 60s
/// that is a 0.7% duty cycle; at 10s it would be visible.
const SESSION_REFRESH: Duration = Duration::from_secs(60);

/// Requests to one account are spaced out rather than fired as a burst of three.
const STAGGER: Duration = Duration::from_millis(400);

/// Older than this and the reading is marked as stale in the bar.
const STALE_AFTER_S: i64 = 330;

/// Short label for the menubar, where width is scarce: "claude2" -> "2".
fn short(label: &str) -> String {
    let t = label.trim_start_matches("claude");
    if t.is_empty() { "1".into() } else { t.to_string() }
}

/// "2" -> "⑵". Parenthesized digits, U+2474..U+2487, covering 1-20.
///
/// Falls back to "[n]" outside that range, or for a label that is not a plain
/// number. This fallback is the whole point: the emoji keycaps used up to v1.0.0
/// had none, so a two-digit account or a non-numeric label emitted a lone U+20E3
/// that rendered as a stray box — indistinguishable from the bar's own error
/// glyph. Any decorated-digit scheme has a range; only the fallback makes it safe.
///
/// U+2474 is NOT present in SF Pro, the macOS system font. AppKit font-fallback
/// substitutes another face, so it does render — but at that font's size and
/// weight, which need not match the rest of the bar. Verified by eye, not assumed.
fn marker(tag: &str) -> String {
    match tag.parse::<u32>() {
        Ok(n) if (1..=20).contains(&n) => char::from_u32(0x2474 + n - 1)
            .map(String::from)
            .unwrap_or_else(|| format!("[{tag}]")),
        _ => format!("[{tag}]"),
    }
}

/// The headline. One field per account so a glance answers "where do I work".
fn title(statuses: &[AccountStatus]) -> String {
    statuses
        .iter()
        .map(|s| {
            let tag = marker(&short(&s.label));
            // Session counts live in the dropdown only. The bar answers "where do
            // I work" in one glance; the count is context you want once you have
            // already decided to look, and it doubled the width of every field.
            match (&s.state, s.worst()) {
                (State::Ok(_), Some(w)) => {
                    let stale = if s.age_s > STALE_AFTER_S { "~" } else { "" };
                    format!("{tag} {:.0}%{stale}", w.pct)
                }
                (State::Ok(_), None) => format!("{tag} ?"),
                (State::NotLoggedIn, _) => format!("{tag} --"),
                (State::Expired { .. }, _) => format!("{tag} exp"),
                (State::Http { code: 429, .. }, _) => format!("{tag} 429"),
                (State::Http { .. }, _) => format!("{tag} err"),
                (State::Error(_), _) => format!("{tag} err"),
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// The native macOS About panel, via muda's predefined item — which calls
/// `orderFrontStandardAboutPanelWithOptions` for us. No extra window and no objc2
/// bridge is needed for this.
///
/// Only `name`, `version`, `short_version`, `copyright`, `icon` and `credits`
/// are mapped on macOS; `website`, `authors` and `comments` are Linux/Windows-only
/// and get silently dropped. So the author and repo link go in `credits` or they
/// would not appear at all.
fn about_item() -> PredefinedMenuItem {
    PredefinedMenuItem::about(
        Some("About Claude Usage"),
        Some(AboutMetadata {
            name: Some("Claude Usage".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            copyright: Some("\u{00a9} 2026 Jacob Kanfer".into()),
            credits: Some(
                "Jacob Kanfer\ngithub.com/Technical-1/claude-usage\n\n\
                 Quota meter for multiple Claude Code accounts.\n\
                 Read-only: never writes to the Keychain."
                    .into(),
            ),
            ..Default::default()
        }),
    )
}

/// The exact text of one account's block: header line, then one line per window.
///
/// Split out so `build_menu` and `--menu` cannot drift. Menu text is the only
/// output that cannot be inspected without launching a GUI, which makes it
/// exactly the thing worth being able to print.
fn account_lines(s: &AccountStatus, sessions: Option<&[sessions::Session]>) -> Vec<String> {
    let tag = marker(&short(&s.label));
    // Absent rather than "0 sessions" when the count is unknown. The two must not
    // look alike; see AccountStatus::sessions.
    let sess = match sessions.map(<[_]>::len) {
        Some(1) => "1 session".to_string(),
        Some(n) => format!("{n} sessions"),
        None => String::new(),
    };
    let mut out = vec![format!("{tag} {:<22}{}", s.label, sess)];

    match &s.state {
        State::Ok(ws) => {
            let vis = usage::visible(ws);
            if vis.is_empty() {
                out.push("      no gauged window".into());
            }
            for w in vis {
                let reset = w
                    .resets_at
                    .map(|t| format!("   ↻ {}", usage::until(t)))
                    .unwrap_or_default();
                out.push(format!(
                    "      {:<15}{:>4.0}%{}",
                    usage::display_name(&w.name),
                    w.pct,
                    reset
                ));
            }
            // Directly under the quota rows, because it is derived from them.
            // Shown only when the five-hour window is measurably filling; nothing
            // to say means no row.
            if let Some((rate, eta)) = s.burn {
                let when = match eta {
                    Some(secs) => format!(
                        " · full in {}",
                        usage::until(chrono::Utc::now() + chrono::Duration::seconds(secs))
                    ),
                    None => String::new(),
                };
                out.push(format!("      ▲ {rate:.0}%/hr on 5-hour{when}"));
            }
            if s.age_s > STALE_AFTER_S {
                out.push(format!("      cached {}m ago — backing off", s.age_s / 60));
            }
        }
        State::NotLoggedIn => {
            out.push("      not signed in — open this account, then /login".into())
        }
        State::Expired { hours_ago } => out.push(format!(
            "      token expired {hours_ago}h ago — open this account once"
        )),
        State::Http { code, message } => out.push(format!("      HTTP {code} — {message}")),
        State::Error(e) => out.push(format!("      {e}")),
    }
    out
}

/// Returns the menu, the Refresh and Quit items, and a map from each session
/// item's id to the tty it should raise.
///
/// The map is rebuilt with the menu: muda hands out fresh ids each time, so a
/// stale map would silently raise the wrong session after any refresh.
/// Everything the menu will display, computed before any UI object is touched.
///
/// Separating "what to show" from "the menu that shows it" is what stops the
/// menu closing under you. macOS closes an open menu when its NSMenu is
/// replaced, and the old code replaced it on every refresh — including the many
/// refreshes where nothing had changed. With the content in hand first, the
/// update path can ask two cheap questions: did anything change at all, and can
/// the change be applied without swapping the menu.
#[derive(PartialEq)]
struct Rendered {
    accounts: Vec<RenderedAccount>,
    odometer: Option<String>,
    title: String,
}

#[derive(PartialEq)]
struct RenderedAccount {
    header: String,
    sessions: Vec<RenderedSession>,
    details: Vec<String>,
}

#[derive(PartialEq)]
struct RenderedSession {
    label: String,
    enabled: bool,
    tty: Option<String>,
}

impl Rendered {
    /// The item layout, ignoring every string. Two renders sharing a shape reuse
    /// the same menu items and can be updated in place; a different shape means
    /// items must be added or removed, which needs a rebuild.
    ///
    /// A header flips between a plain item and a submenu when an account's
    /// session count crosses zero — captured here because the session count is
    /// part of the shape.
    fn shape(&self) -> (Vec<(usize, usize)>, bool) {
        (
            self.accounts
                .iter()
                .map(|a| (a.sessions.len(), a.details.len()))
                .collect(),
            self.odometer.is_some(),
        )
    }
}

fn render(
    statuses: &[AccountStatus],
    all_sessions: Option<&[sessions::Session]>,
    odo: tokens::Tokens,
) -> Rendered {
    let accounts = statuses
        .iter()
        .map(|s| {
            let mine: Option<Vec<sessions::Session>> = all_sessions.map(|all| {
                all.iter()
                    .filter(|x| x.root_label == s.label)
                    .cloned()
                    .collect()
            });
            let lines = account_lines(s, mine.as_deref());
            RenderedAccount {
                header: lines[0].clone(),
                sessions: mine
                    .unwrap_or_default()
                    .into_iter()
                    .map(|x| RenderedSession {
                        label: x.label(),
                        enabled: x.raisable,
                        tty: if x.raisable { x.tty.clone() } else { None },
                    })
                    .collect(),
                details: lines[1..].to_vec(),
            }
        })
        .collect();

    let odometer = (odo.input > 0 || odo.output > 0).then(|| {
        format!(
            "Tokens all time      \u{2191} {}   \u{2193} {}",
            tokens::human(odo.input),
            tokens::human(odo.output)
        )
    });

    Rendered { accounts, odometer, title: title(statuses) }
}

/// A header is a submenu when the account has sessions to list, a plain readout
/// otherwise. Both need their text updated in place, so they are held together.
enum Header {
    Plain(MenuItem),
    Sub(Submenu),
}

impl Header {
    fn set_text(&self, text: &str) {
        match self {
            Header::Plain(m) => m.set_text(text),
            Header::Sub(s) => s.set_text(text),
        }
    }
}

/// Retained handles to every item, so text can change without rebuilding.
struct Handles {
    headers: Vec<Header>,
    sessions: Vec<Vec<MenuItem>>,
    details: Vec<Vec<MenuItem>>,
    odometer: Option<MenuItem>,
    refresh: MenuItem,
    quit: MenuItem,
    login: CheckMenuItem,
    raise_map: HashMap<MenuId, String>,
}

impl Handles {
    /// Apply new text to the existing items. Never replaces the NSMenu, so an
    /// open menu stays open — the whole point.
    ///
    /// Only valid when the shapes match. Position `j` may now hold a
    /// *different* session than last time (one ended and another started, in
    /// equal numbers), so the tty mapping is rewritten rather than assumed stable.
    fn update(&mut self, r: &Rendered) {
        for (i, acc) in r.accounts.iter().enumerate() {
            self.headers[i].set_text(&acc.header);
            for (j, sess) in acc.sessions.iter().enumerate() {
                let item = &self.sessions[i][j];
                item.set_text(&sess.label);
                item.set_enabled(sess.enabled);
                match &sess.tty {
                    Some(tty) => {
                        self.raise_map.insert(item.id().clone(), tty.clone());
                    }
                    None => {
                        self.raise_map.remove(item.id());
                    }
                }
            }
            for (j, line) in acc.details.iter().enumerate() {
                self.details[i][j].set_text(line);
            }
        }
        if let (Some(item), Some(text)) = (&self.odometer, &r.odometer) {
            item.set_text(text);
        }
        self.login.set_checked(autostart::is_enabled());
    }
}

fn build_menu(r: &Rendered) -> (Menu, Handles) {
    let menu = Menu::new();
    let mut h = Handles {
        headers: Vec::new(),
        sessions: Vec::new(),
        details: Vec::new(),
        odometer: None,
        refresh: MenuItem::new("Refresh now", true, None),
        quit: MenuItem::new("Quit", true, None),
        login: CheckMenuItem::new("Start at login", true, autostart::is_enabled(), None),
        raise_map: HashMap::new(),
    };

    for acc in &r.accounts {
        if acc.sessions.is_empty() {
            let item = MenuItem::new(&acc.header, false, None);
            menu.append(&item).ok();
            h.headers.push(Header::Plain(item));
            h.sessions.push(Vec::new());
        } else {
            let sub = Submenu::new(&acc.header, true);
            let mut items = Vec::new();
            for sess in &acc.sessions {
                // A session with no controlling terminal still burns quota, so it
                // is listed — just not clickable, because there is no tab to
                // raise. Hiding it would make the submenu disagree with the count.
                let item = MenuItem::new(&sess.label, sess.enabled, None);
                if let Some(tty) = &sess.tty {
                    h.raise_map.insert(item.id().clone(), tty.clone());
                }
                sub.append(&item).ok();
                items.push(item);
            }
            menu.append(&sub).ok();
            h.headers.push(Header::Sub(sub));
            h.sessions.push(items);
        }

        let mut rows = Vec::new();
        for line in &acc.details {
            let item = MenuItem::new(line, false, None);
            menu.append(&item).ok();
            rows.push(item);
        }
        h.details.push(rows);
        menu.append(&PredefinedMenuItem::separator()).ok();
    }

    if let Some(text) = &r.odometer {
        let item = MenuItem::new(text, false, None);
        menu.append(&item).ok();
        menu.append(&PredefinedMenuItem::separator()).ok();
        h.odometer = Some(item);
    }

    menu.append(&h.refresh).ok();
    menu.append(&h.login).ok();
    menu.append(&about_item()).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&h.quit).ok();
    (menu, h)
}

/// Accounts are polled with a gap between them. Three requests fired back to back
/// look like a burst to a rate limiter even when the hourly average is modest.
/// Cache hits cost nothing, so the gap only applies when a request is actually spent.
fn poll_all(roots: &[accounts::Root]) -> Vec<AccountStatus> {
    let mut out: Vec<AccountStatus> = Vec::with_capacity(roots.len());
    for (i, r) in roots.iter().enumerate() {
        // Only pause after a poll that actually went to the network. A run served
        // entirely from cache must cost nothing, including wall-clock — `--title`
        // is meant to be cheap enough to pipe into another statusline.
        if i > 0 && out.last().is_some_and(spent_a_request) {
            std::thread::sleep(STAGGER);
        }
        let st = accounts::poll(r);
        out.push(st);
    }
    out
}

/// True if this status came from a request we just spent, rather than the cache.
fn spent_a_request(s: &AccountStatus) -> bool {
    s.age_s == 0 && matches!(s.state, State::Ok(_))
}

/// The Refresh menu item. Bypasses the freshness floor, never the backoff.
fn poll_all_forced(roots: &[accounts::Root]) -> Vec<AccountStatus> {
    let mut out: Vec<AccountStatus> = Vec::with_capacity(roots.len());
    for (i, r) in roots.iter().enumerate() {
        // Only pause after a poll that actually went to the network. A run served
        // entirely from cache must cost nothing, including wall-clock — `--title`
        // is meant to be cheap enough to pipe into another statusline.
        if i > 0 && out.last().is_some_and(spent_a_request) {
            std::thread::sleep(STAGGER);
        }
        let st = accounts::poll_forced(r);
        out.push(st);
    }
    out
}

fn main() {
    let roots = accounts::load_roots();
    if roots.is_empty() {
        eprintln!("no roots configured in ~/.config/claude-usage/roots.json");
        std::process::exit(1);
    }

    // --title: print exactly what the menubar would show, and exit. Useful for
    // checking glyph rendering without a GUI, and for piping into another statusline.
    if std::env::args().any(|a| a == "--title") {
        println!("{}", title(&poll_all(&roots)));
        return;
    }

    // --menu: print the dropdown's exact text and exit. The menu is otherwise
    // only inspectable by launching the GUI and looking at it.
    if std::env::args().any(|a| a == "--menu") {
        let all = sessions::list(&roots);
        for s in poll_all(&roots) {
            let mine: Option<Vec<sessions::Session>> = all.as_ref().map(|v| {
                v.iter().filter(|x| x.root_label == s.label).cloned().collect()
            });
            let lines = account_lines(&s, mine.as_deref());
            let listed = mine.as_deref().unwrap_or(&[]);
            println!("{}{}", lines[0], if listed.is_empty() { "" } else { "  ▸" });
            for sess in listed {
                println!("        {}{}", sess.label(),
                         if sess.raisable { "" } else { "   [cannot raise]" });
            }
            for line in &lines[1..] {
                println!("{line}");
            }
            println!("{}", "-".repeat(46));
        }
        let odo = odometer::update(&roots);
        println!("Tokens all time      ↑ {}   ↓ {}",
                 tokens::human(odo.input), tokens::human(odo.output));
        println!("{}", "-".repeat(46));
        println!("Refresh now\nStart at login\nAbout Claude Usage\n\nQuit");
        return;
    }

    // --list: a headless mode, so the same binary is useful from a script and so
    // the data path can be checked without a GUI in the way.
    if std::env::args().any(|a| a == "--list") {
        for s in poll_all(&roots) {
            match (&s.state, s.worst()) {
                (State::Ok(ws), _) => {
                    println!("{}:", s.label);
                    for w in ws {
                        let r = w.resets_at.map(|t| format!("  resets in {}", usage::until(t)))
                            .unwrap_or_default();
                        println!("   {}{:<24} {:>6.1}%{}",
                                 if w.gauge { "" } else { "· " }, w.name, w.pct, r);
                    }
                }
                (State::NotLoggedIn, _) => println!("{}: not signed in", s.label),
                (State::Expired { hours_ago }, _) =>
                    println!("{}: token expired {}h ago — open it once to refresh", s.label, hours_ago),
                (State::Http { code, message }, _) => println!("{}: HTTP {code} — {message}", s.label),
                (State::Error(e), _) => println!("{}: {e}", s.label),
            }
        }
        return;
    }

    // Only the GUI is exclusive. The headless modes above are read-only and
    // must keep working while the tray app runs.
    let _guard = match instance::acquire() {
        Ok(g) => g,
        Err(()) => {
            notify::post("Claude Usage", "Claude Usage is already running.");
            eprintln!("claude-usage is already running");
            return;
        }
    };

    let statuses = Arc::new(Mutex::new(poll_all(&roots)));
    let dirty = Arc::new(Mutex::new(true));
    let odo = Arc::new(Mutex::new(tokens::Tokens::default()));
    let sess = Arc::new(Mutex::new(sessions::list(&roots)));

    // Sessions refresh on their own, faster timer. They cost no requests, so
    // there is no reason for them to wait on the quota budget.
    {
        let sess = Arc::clone(&sess);
        let dirty = Arc::clone(&dirty);
        let roots = roots.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(SESSION_REFRESH);
                let fresh = sessions::list(&roots);
                *sess.lock().unwrap() = fresh;
                *dirty.lock().unwrap() = true;
            }
        });
    }

    // The first odometer pass reads every transcript ever written (~1.2GB), so it
    // runs on its own thread — the menubar must appear immediately, not after a
    // full-corpus scan. Later passes only read newly appended bytes.
    {
        let odo = Arc::clone(&odo);
        let dirty = Arc::clone(&dirty);
        let roots = roots.clone();
        std::thread::spawn(move || {
            let t = odometer::update(&roots);
            *odo.lock().unwrap() = t;
            *dirty.lock().unwrap() = true;
        });
    }

    // Network off the UI thread. A hung request must never freeze the menubar.
    {
        let statuses = Arc::clone(&statuses);
        let dirty = Arc::clone(&dirty);
        let roots = roots.clone();
        let odo = Arc::clone(&odo);
        std::thread::spawn(move || loop {
            std::thread::sleep(REFRESH);
            let fresh = poll_all(&roots);
            let t = odometer::update(&roots);
            *statuses.lock().unwrap() = fresh;
            *odo.lock().unwrap() = t;
            *dirty.lock().unwrap() = true;
        });
    }

    let mut event_loop = EventLoopBuilder::new().build();
    // Accessory: menubar only, no Dock icon, no app-switcher entry.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let mut last = render(
        &statuses.lock().unwrap(),
        sess.lock().unwrap().as_deref(),
        *odo.lock().unwrap(),
    );
    let (menu, mut handles) = build_menu(&last);
    let first_title = last.title.clone();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_title(first_title)
        .with_tooltip("Claude usage")
        .build()
        .expect("failed to create tray icon");

    let menu_rx = MenuEvent::receiver();

    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));

        while let Ok(ev) = menu_rx.try_recv() {
            if ev.id == *handles.quit.id() {
                std::process::exit(0);
            }
            if ev.id == *handles.refresh.id() {
                *statuses.lock().unwrap() = poll_all_forced(&roots);
                *dirty.lock().unwrap() = true;
            }
            if ev.id == *handles.login.id() {
                // muda has already flipped the check state, so act on the NEW value.
                let want = !autostart::is_enabled();
                let r = if want { autostart::enable() } else { autostart::disable() };
                match r {
                    Ok(()) if want => notify::post("Claude Usage", "Will start at login."),
                    Ok(()) => notify::post("Claude Usage", "Will no longer start at login."),
                    Err(e) => notify::post("Claude Usage", &format!("Could not change it: {e}")),
                }
                *dirty.lock().unwrap() = true;
            }
            // Clicking a session brings its Terminal tab to the front. A failure
            // is surfaced, never swallowed — a denied Automation permission would
            // otherwise look like a dead menu item.
            if let Some(tty) = handles.raise_map.get(&ev.id)
                && let Err(e) = terminal::raise(tty) {
                    notify::post("Claude Usage", &e);
                }
        }

        let mut d = dirty.lock().unwrap();
        if *d {
            *d = false;
            let fresh = render(
                &statuses.lock().unwrap(),
                sess.lock().unwrap().as_deref(),
                *odo.lock().unwrap(),
            );

            // 1. Nothing changed — touch nothing. Most refreshes land here: the
            //    session list is usually identical and quota only moves on the
            //    300s tick. Replacing the menu to install identical content was
            //    what closed an open menu every 60 seconds.
            if fresh != last {
                if fresh.shape() == last.shape() {
                    // 2. Same items, new text: update in place. Does NOT replace
                    //    the NSMenu, so a menu you have open stays open.
                    handles.update(&fresh);
                } else {
                    // 3. Items were added or removed — a session started or
                    //    ended. Only this needs a rebuild, and only this can
                    //    close the menu.
                    let (menu, fresh_handles) = build_menu(&fresh);
                    handles = fresh_handles;
                    tray.set_menu(Some(Box::new(menu)));
                }
                tray.set_title(Some(fresh.title.clone()));
                last = fresh;
            }
        }
    });
}
