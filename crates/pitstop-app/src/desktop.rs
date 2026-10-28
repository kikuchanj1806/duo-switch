//! Claude Desktop's profiles from the window and the tray: switching, the
//! account each profile is signed in to, and bringing Code Local sessions
//! along. All profile work happens in `pitstop_core::desktop`; anything that
//! touches files or the app runs off the UI thread.

use pitstop_core::desktop::{
    AutoSync, CleanReport, Desktop, DesktopApp, MacApp, NoApp, Overview, Paths, Storage, SyncReport,
};
use pitstop_core::Host;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::app::{self, AppState};
use crate::commands::blocking;
use crate::i18n::Lang;
use crate::model::UiError;
use crate::settings;

type Answer<T> = Result<T, UiError>;
type Profiles = Desktop<Box<dyn DesktopApp>>;

fn sandboxed() -> bool {
    std::env::var_os("PITSTOP_HOME").is_some()
}

/// `app`, or no app at all in a sandbox (`PITSTOP_HOME`), which never quits
/// or opens the real ones.
fn control(app: MacApp) -> Box<dyn DesktopApp> {
    if sandboxed() {
        Box::new(NoApp)
    } else {
        Box::new(app)
    }
}

/// Codex Desktop, quit while its login is swapped and opened again after.
pub fn codex_app() -> Box<dyn DesktopApp> {
    control(MacApp::CODEX)
}

/// The profiles under the app's home.
fn profiles() -> pitstop_core::error::Result<Profiles> {
    Desktop::new(
        Paths::new(&app::host().home(), sandboxed()),
        control(MacApp::CLAUDE),
    )
}

pub fn overview() -> Overview {
    match profiles() {
        Ok(d) => d.overview(),
        Err(e) => Overview::broken(e.to_string()),
    }
}

/// Run a change one at a time with the app's other changes, then show what
/// it left.
fn change<T>(
    app: &AppHandle,
    work: impl FnOnce(&Profiles) -> pitstop_core::error::Result<T>,
) -> Answer<T> {
    let state = app.state::<AppState>();
    let done = {
        let _op = state.ops.lock();
        profiles().and_then(|d| work(&d))
    };
    app::refresh(app);
    done.map_err(|e| UiError::from(&e))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Switched {
    pub from: String,
    pub to: String,
    pub switched: bool,
    pub backup: Option<String>,
    /// The automatic sync into `to`, when it is on and worked.
    pub sync: Option<SyncReport>,
    /// The automatic sync failed; the switch itself stands.
    pub sync_error: Option<UiError>,
}

pub fn switch(app: &AppHandle, name: &str) -> Answer<Switched> {
    let report = change(app, |d| d.switch(name))?;
    let (sync, sync_error) = match report.sync {
        None => (None, None),
        Some(Ok(done)) => (Some(done), None),
        Some(Err(e)) => (None, Some(UiError::from(&e))),
    };
    Ok(Switched {
        from: report.from,
        to: report.to,
        switched: report.switched,
        backup: report.backup.map(|p| p.display().to_string()),
        sync,
        sync_error,
    })
}

/// A switch chosen in the tray menu: told in a notification, and in the
/// window when it is open; a failure opens the window to say why.
pub fn switch_from_tray(app: &AppHandle, name: &str) {
    let app = app.clone();
    let name = name.to_owned();
    std::thread::spawn(move || {
        let lang = Lang::resolve(settings::load(&app::host()).language.as_deref());
        match switch(&app, &name) {
            Ok(done) => {
                if done.switched {
                    let body = match (&done.sync, &done.sync_error) {
                        (Some(sync), _) => lang.desktop_synced(sync.added),
                        (None, Some(error)) => error.message.clone(),
                        (None, None) => String::new(),
                    };
                    app::notify(&app, &lang.desktop_switched(&done.to), &body);
                }
                let _ = app.emit("desktop-switched", &done);
            }
            Err(error) => {
                app::show_window(&app);
                let _ = app.emit(
                    "desktop-failed",
                    serde_json::json!({ "name": name, "error": error }),
                );
            }
        }
    });
}

#[tauri::command]
pub async fn desktop_switch(app: AppHandle, name: String) -> Answer<Switched> {
    blocking(move || switch(&app, &name)).await
}

#[tauri::command]
pub async fn desktop_assign(app: AppHandle, name: String, pair: String) -> Answer<()> {
    blocking(move || change(&app, |d| d.assign(&name, &pair))).await
}

#[tauri::command]
pub async fn desktop_unassign(app: AppHandle, name: String) -> Answer<()> {
    blocking(move || change(&app, |d| d.unassign(&name))).await
}

/// Set or clear (`None`) the name shown for a profile's account.
#[tauri::command]
pub async fn desktop_set_label(app: AppHandle, name: String, label: Option<String>) -> Answer<()> {
    blocking(move || change(&app, |d| d.set_label(&name, label.as_deref()))).await
}

#[tauri::command]
pub async fn desktop_set_autosync(app: AppHandle, name: String, on: bool) -> Answer<AutoSync> {
    blocking(move || change(&app, |d| d.set_autosync(&name, on))).await
}

/// What a sync into the profile in use would add. Reads only.
#[tauri::command]
pub async fn desktop_preview() -> Answer<SyncReport> {
    blocking(move || {
        profiles()
            .and_then(|d| d.preview(None, None))
            .map_err(|e| UiError::from(&e))
    })
    .await
}

#[tauri::command]
pub async fn desktop_sync(app: AppHandle) -> Answer<SyncReport> {
    blocking(move || change(&app, |d| d.sync(None, None))).await
}

#[tauri::command]
pub async fn desktop_add(app: AppHandle, name: String) -> Answer<()> {
    blocking(move || change(&app, |d| d.add(&name))).await
}

#[tauri::command]
pub async fn desktop_setup(app: AppHandle, name: String) -> Answer<()> {
    blocking(move || change(&app, |d| d.setup(&name))).await
}

/// Where the disk space under the profiles goes. Reads only.
#[tauri::command]
pub async fn desktop_storage() -> Answer<Storage> {
    blocking(move || {
        profiles()
            .and_then(|d| d.storage())
            .map_err(|e| UiError::from(&e))
    })
    .await
}

/// Free the caches, older Claude Code versions and old backups of the
/// profiles not in use.
#[tauri::command]
pub async fn desktop_clean(app: AppHandle) -> Answer<CleanReport> {
    blocking(move || change(&app, |d| d.clean())).await
}
