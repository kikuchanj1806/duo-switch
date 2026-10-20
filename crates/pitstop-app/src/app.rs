//! The app's running state: the last usage read, the snapshot built from it,
//! and the one place that rebuilds both the window and the tray menu.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pitstop_core::engine::AccountUsage;
use pitstop_core::usage::Http;
use pitstop_core::{signin, Engine, ProviderId, RealHost};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

use crate::duty;
use crate::i18n::Lang;
use crate::model::{self, Snapshot};
use crate::settings;

/// How often the app looks whether a notice is due.
const DUTIES_EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct AppState {
    inner: Mutex<Inner>,
    /// One change at a time: a switch from the menu and one from the window
    /// never interleave.
    pub ops: Mutex<()>,
    pub sign_in: Mutex<Option<crate::signin::SignInJob>>,
    /// A round of duties is under way; another is not started over it.
    duties: AtomicBool,
    automatic: Mutex<Automatic>,
}

/// What the run-out notices remember while the app runs.
#[derive(Default)]
struct Automatic {
    /// Limits notified, until they reset.
    sent: HashSet<String>,
}

#[derive(Default)]
struct Inner {
    revision: u64,
    usage: Vec<AccountUsage>,
    reads: duty::Reads,
    updated_at: Option<i64>,
    installed: Vec<(ProviderId, bool)>,
    snapshot: Option<Snapshot>,
}

/// Where Pitstop keeps its state: the home directory, or `PITSTOP_HOME` to
/// try the app on a sandbox without touching real accounts.
pub fn host() -> RealHost {
    std::env::var_os("PITSTOP_HOME")
        .map(|home| RealHost::with_home(home.into()))
        .unwrap_or_default()
}

/// The real home, where installers put `codex`.
pub fn real_home() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default()
}

pub fn engine() -> Engine<RealHost> {
    Engine::new(host())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn snapshot(app: &AppHandle) -> Option<Snapshot> {
    let state = app.state::<AppState>();
    let inner = state.inner.lock().ok()?;
    inner.snapshot.clone()
}

/// Read the accounts again and show them, in the window and the tray menu.
pub fn refresh(app: &AppHandle) {
    let host = host();
    let settings = settings::load(&host);
    let lang = Lang::resolve(settings.language.as_deref());
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let engine = Engine::new(host);

    // Copy what the snapshot needs, then read the accounts (files, and the
    // keychain on macOS) without holding the lock.
    let state = app.state::<AppState>();
    let (revision, usage, reading, updated_at, installed) = {
        let Ok(mut inner) = state.inner.lock() else {
            return;
        };
        inner.revision += 1;
        (
            inner.revision,
            inner.usage.clone(),
            inner.reads.busy(),
            inner.updated_at,
            inner.installed.clone(),
        )
    };
    #[cfg(target_os = "macos")]
    let (desktop, codex_running) = (
        Some(crate::desktop::overview()),
        crate::desktop::codex_app().running().ok(),
    );
    #[cfg(not(target_os = "macos"))]
    let (desktop, codex_running) = (None, None);
    let snapshot = model::build(
        &engine,
        &model::Input {
            revision,
            now: now(),
            usage: &usage,
            reading,
            updated_at,
            installed: &installed,
            settings: &settings,
            language: lang,
            autostart,
            codex_running,
            desktop,
        },
    );
    {
        let Ok(mut inner) = state.inner.lock() else {
            return;
        };
        // A refresh that started later may have finished first.
        if inner
            .snapshot
            .as_ref()
            .is_some_and(|shown| shown.revision > revision)
        {
            return;
        }
        inner.snapshot = Some(snapshot.clone());
    }
    let _ = app.emit("snapshot", &snapshot);
    crate::tray::update(app, &snapshot, lang);
}

/// Usage read under an account's old name now belongs to its new one.
pub fn rename_usage(app: &AppHandle, provider: ProviderId, old: &str, new: &str) {
    let state = app.state::<AppState>();
    if let Ok(mut inner) = state.inner.lock() {
        for usage in &mut inner.usage {
            if usage.provider == provider && usage.name == old {
                new.clone_into(&mut usage.name);
            }
        }
    };
}

/// Stop a sign-in that is running, then quit.
pub fn quit(app: &AppHandle) {
    crate::signin::cancel(app);
    app.exit(0);
}

/// Ask OpenAI for every account's usage, off the calling
/// thread; a read already under way is not started twice.
pub fn read_usage(app: &AppHandle) {
    read(app, false);
}

/// [`read_usage`] even right after another read: an account just added has
/// no reading at all.
pub fn read_usage_now(app: &AppHandle) {
    read(app, true);
}

/// Ask for a read; [`duty::Reads`] says when it happens.
fn read(app: &AppHandle, now_anyway: bool) {
    let plan = {
        let state = app.state::<AppState>();
        let Ok(mut inner) = state.inner.lock() else {
            return;
        };
        inner.reads.ask(now_anyway, now())
    };
    follow(app, plan);
}

fn follow(app: &AppHandle, plan: duty::ReadPlan) {
    match plan {
        duty::ReadPlan::Nothing => {}
        duty::ReadPlan::Start => start_read(app),
        duty::ReadPlan::WaitUntil(due) => {
            let later = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs((due - now()).max(0) as u64));
                let plan = {
                    let state = later.state::<AppState>();
                    let Ok(mut inner) = state.inner.lock() else {
                        return;
                    };
                    inner.reads.turn_came(due, now())
                };
                follow(&later, plan);
            });
            // Refresh spins while the read waits for its turn.
            refresh(app);
        }
    }
}

/// Read every account's usage, off the calling thread.
fn start_read(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        refresh(&app);
        // However the read ends, the next one may start.
        let read_now = std::panic::catch_unwind(|| engine().usage(&Http::new()));
        let plan = {
            let state = app.state::<AppState>();
            let Ok(mut inner) = state.inner.lock() else {
                return;
            };
            if let Ok(Ok(usage)) = read_now {
                let (usage, updated_at) =
                    duty::merge_read(&inner.usage, inner.updated_at, usage, now());
                inner.usage = usage;
                inner.updated_at = updated_at;
            }
            inner.reads.ended(now())
        };
        refresh(&app);
        follow(&app, plan);
        run_duties(&app);
    });
}

// ---------- duties: run-out notices ----------

pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// One round: decide from the last snapshot, act, and show what came of it.
/// Never two rounds at once.
pub fn run_duties(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.duties.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let finished =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| duties(&app))).is_ok();
        let state = app.state::<AppState>();
        if !finished {
            // A panic while the state was held must not stop the app updating.
            state.inner.clear_poison();
        }
        state.duties.store(false, Ordering::SeqCst);
    });
}

fn duties(app: &AppHandle) {
    let Some(snapshot) = snapshot(app) else {
        return;
    };
    if snapshot.reading || snapshot.updated_at.is_none() {
        return;
    }
    let settings = settings::load(&host());
    if !settings.notify_run_out {
        return;
    }
    let lang = Lang::resolve(settings.language.as_deref());
    let state = app.state::<AppState>();
    let fresh = {
        let Ok(mut auto) = state.automatic.lock() else {
            return;
        };
        let fresh = duty::run_out_notices(&snapshot.suggestions, &auto.sent, now());
        auto.sent.extend(fresh.iter().map(|n| n.key.clone()));
        fresh
    };
    for notice in fresh {
        notify(
            app,
            &lang.ran_out(&notice.from, &lang.window(&notice.window)),
            &lang.still_has_room(&notice.to),
        );
    }
}

/// Seconds until the next usage read, judged from the resets the last
/// reading knows of (they stay where they are whatever the new one says).
fn next_read_wait(app: &AppHandle) -> u64 {
    use pitstop_core::usage::UsageReport;
    let state = app.state::<AppState>();
    let (resets, offline): (Vec<i64>, bool) =
        state
            .inner
            .lock()
            .map(|inner| {
                let resets = inner
                    .usage
                    .iter()
                    .filter_map(|u| match &u.report {
                        UsageReport::Ok(usage) => {
                            Some(usage.windows.iter().filter_map(|w| w.resets_at))
                        }
                        _ => None,
                    })
                    .flatten()
                    .collect();
                // No reading came back and some failed for want of a network, as
                // when the app starts at login before Wi-Fi is up.
                let offline = !inner.usage.iter().any(|u| matches!(u.report, UsageReport::Ok(_)))
                && inner.usage.iter().any(
                    |u| matches!(&u.report, UsageReport::Failed(why) if why.starts_with("network")),
                );
                (resets, offline)
            })
            .unwrap_or_default();
    if offline {
        return duty::READ_OFFLINE as u64;
    }
    duty::next_read_after(resets.into_iter(), now()).max(1) as u64
}

/// Look for each tool not found yet (all of them the first time), so one
/// installed while the app runs can be added without a restart; `now` skips
/// the pause between looks. True when one turned up.
pub fn look_for_tools(app: &AppHandle, now: bool) -> bool {
    let state = app.state::<AppState>();
    let missing: Vec<ProviderId> = match state.inner.lock() {
        Ok(inner) => ProviderId::ALL
            .iter()
            .copied()
            .filter(|p| !inner.installed.iter().any(|(q, found)| q == p && *found))
            .collect(),
        Err(_) => return false,
    };
    if missing.is_empty() {
        return false;
    }
    // On macOS a look starts a login shell: not more than twice a minute.
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if let Ok(mut last) = LAST.lock() {
        if !now && last.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return false;
        }
        *last = Some(Instant::now());
    }
    let home = real_home();
    let found: Vec<(ProviderId, bool)> = missing
        .into_iter()
        .map(|p| (p, signin::find_program(p, &home).is_some()))
        .collect();
    let Ok(mut inner) = state.inner.lock() else {
        return false;
    };
    let mut turned_up = false;
    for (p, there) in found {
        match inner.installed.iter_mut().find(|(q, _)| *q == p) {
            Some(entry) => {
                turned_up |= there && !entry.1;
                entry.1 = there;
            }
            None => inner.installed.push((p, there)),
        }
    }
    turned_up
}

/// Look for the tools once, read usage now, again shortly after each limit
/// resets, and at least every five minutes.
pub fn start_background(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        refresh(&handle);
        look_for_tools(&handle, true);
        refresh(&handle);
    });

    let handle = app.clone();
    std::thread::spawn(move || loop {
        read_usage(&handle);
        std::thread::sleep(Duration::from_secs(next_read_wait(&handle)));
    });

    let handle = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(DUTIES_EVERY);
        run_duties(&handle);
    });
}

/// The title bar is drawn by the system, not the page, so it follows the
/// chosen theme here; none hands it back to the system's.
pub fn apply_window_theme(app: &AppHandle, choice: Option<&str>) {
    let theme = match choice {
        Some("light") => Some(tauri::Theme::Light),
        Some("dark") => Some(tauri::Theme::Dark),
        _ => None,
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(theme);
    }
}

pub fn show_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The window goes, the app stays in the tray; on macOS the Dock icon goes
/// with the window.
pub fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

/// Bring the window up with one of its dialogs open, for the tray menu.
pub fn open_dialog(app: &AppHandle, kind: &str, provider: Option<&str>, name: Option<&str>) {
    show_window(app);
    let _ = app.emit(
        "open-dialog",
        serde_json::json!({ "kind": kind, "provider": provider, "name": name }),
    );
}
