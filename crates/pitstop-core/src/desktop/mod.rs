//! Claude Desktop profiles, kept the way `claude-switch` keeps them, so the
//! two can be used on the same Mac in turn:
//!
//! - `~/Library/Application Support/Claude` is a symlink to one profile,
//!   `~/ClaudeProfiles/<name>` (`CLAUDE_PROFILES_DIR`), each a whole app
//!   folder signed in to its own account;
//! - `_meta/<name>.account` names the profile's `account/organization`
//!   folder under `claude-code-sessions/`, and `_meta/<name>.autosync.json`
//!   whether switching to it brings the other profiles' sessions along;
//! - `_backups/` keeps a copy of a profile before each switch, less its
//!   caches at any depth, and before each sync the list of cards it adds;
//!   only the newest few of each per profile are kept (see [`KEEP_BACKUPS`]);
//! - `_meta/.claude-switch.lock` keeps one change at a time, across both.
//!
//! A sync copies Code Local session cards (`local_*.json`) from the source
//! profiles' account into the target's, rewriting only the identity fields
//! [`card::transform`] recognises. The transcripts themselves live in
//! `~/.claude/projects` (`CLAUDE_CONFIG_DIR`), shared by every profile, and
//! are never touched: a card whose transcript is missing is left out.
//!
//! Claude is asked to quit before anything is changed and opened again
//! after; it is never killed.

pub mod card;
pub mod control;
mod fsx;
pub mod limits;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

pub use control::{DesktopApp, MacApp, NoApp};

use crate::error::{Error, Result};

/// Why a profile change was refused; the window words each one itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    NotSetUp,
    AlreadySetUp,
    NoAppData,
    Unsafe,
    App,
    AppWontQuit,
    AutomationDenied,
    AppReopened,
    Busy,
    InvalidName,
    UnknownProfile,
    ProfileExists,
    NotAssigned,
    NoSessions,
    UnknownPair,
    AccountTaken,
    NoSources,
    NoHistory,
    InvalidCard,
    InvalidAutosync,
    InvalidLabel,
    SyncIncomplete,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::NotSetUp => "desktopNotSetUp",
            Code::AlreadySetUp => "desktopAlreadySetUp",
            Code::NoAppData => "desktopNoAppData",
            Code::Unsafe => "desktopUnsafe",
            Code::App => "desktopApp",
            Code::AppWontQuit => "desktopAppWontQuit",
            Code::AutomationDenied => "desktopAutomationDenied",
            Code::AppReopened => "desktopAppReopened",
            Code::Busy => "desktopBusy",
            Code::InvalidName => "desktopInvalidName",
            Code::UnknownProfile => "desktopUnknownProfile",
            Code::ProfileExists => "desktopProfileExists",
            Code::NotAssigned => "desktopNotAssigned",
            Code::NoSessions => "desktopNoSessions",
            Code::UnknownPair => "desktopUnknownPair",
            Code::AccountTaken => "desktopAccountTaken",
            Code::NoSources => "desktopNoSources",
            Code::NoHistory => "desktopNoHistory",
            Code::InvalidCard => "desktopInvalidCard",
            Code::InvalidAutosync => "desktopInvalidAutosync",
            Code::InvalidLabel => "desktopInvalidLabel",
            Code::SyncIncomplete => "desktopSyncIncomplete",
        }
    }
}

pub(crate) fn refuse(code: Code, message: impl Into<String>) -> Error {
    Error::Desktop {
        code: code.as_str(),
        message: message.into(),
    }
}

pub(crate) fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Names `claude-switch` takes as commands, so no profile may be called so.
const COMMANDS: [&str; 17] = [
    "version",
    "help",
    "list",
    "ls",
    "status",
    "doctor",
    "sessions",
    "init",
    "setup",
    "new",
    "add",
    "backup",
    "account",
    "sync",
    "auto-sync",
    "restore",
    "import",
];
/// Chromium's caches, left out of a backup wherever they sit (the preview
/// browser keeps ~1 GB under `Partitions/<name>/Cache`); the app makes them
/// again.
const CACHE_DIRS: [&str; 6] = [
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GraphiteDawnCache",
];
/// Left out at the top of a profile's backup: the large folders the app
/// downloads again by itself.
const EXCLUDES: [&str; 3] = ["vm_bundles", "claude-code", "claude-code-vm"];
/// Backups of each kind kept per profile, the newest; `0` keeps them all.
/// `CLAUDE_SWITCH_KEEP_BACKUPS` sets it, for `claude-switch` too.
pub const KEEP_BACKUPS: usize = 3;
const SESSIONS: &str = "claude-code-sessions";
/// What a clean-up never walks into looking for caches.
const KEEP_OUT: [&str; 5] = [
    "vm_bundles",
    "claude-code",
    "claude-code-vm",
    SESSIONS,
    "local-agent-mode-sessions",
];
const MAX_LABEL: usize = 100;

pub fn valid_name(name: &str) -> Result<&str> {
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !COMMANDS.contains(&name);
    if ok {
        Ok(name)
    } else {
        Err(refuse(
            Code::InvalidName,
            format!("invalid profile name `{name}`: letters, digits, `-` and `_`, starting with a letter or digit"),
        ))
    }
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// Where everything is.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    /// The profiles, `~/ClaudeProfiles`.
    pub root: PathBuf,
    /// Claude Code's folder, whose `projects/` holds the transcripts.
    pub config: PathBuf,
    /// The folder Claude Desktop uses, a symlink to a profile once set up.
    pub app_data: PathBuf,
}

impl Paths {
    /// The paths under `home`. A sandbox takes everything from `home` and
    /// nothing from the environment.
    pub fn new(home: &Path, sandboxed: bool) -> Paths {
        let env = |key: &str| -> Option<PathBuf> {
            if sandboxed {
                return None;
            }
            let value = PathBuf::from(std::env::var_os(key).filter(|v| !v.is_empty())?);
            Some(match value.strip_prefix("~") {
                Ok(rest) => home.join(rest),
                Err(_) => value,
            })
        };
        Paths {
            home: home.to_path_buf(),
            root: fsx::resolve_lenient(
                &env("CLAUDE_PROFILES_DIR").unwrap_or_else(|| home.join("ClaudeProfiles")),
            ),
            config: fsx::resolve_lenient(
                &env("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            ),
            app_data: home.join("Library/Application Support/Claude"),
        }
    }
}

/// Whether switching to a profile brings the other profiles' sessions along.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSync {
    pub enabled: bool,
    pub sources: Vec<String>,
}

/// One `account/organization` folder of a profile, and how many cards it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairView {
    pub pair: String,
    pub cards: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub name: String,
    pub current: bool,
    /// The `account/organization` assigned to it.
    pub account: Option<String>,
    pub account_error: Option<String>,
    pub pairs: Vec<PairView>,
    pub autosync: AutoSync,
    pub autosync_error: Option<String>,
    /// A name to show for the profile's account, set by hand.
    pub label: Option<String>,
    /// The account Claude Desktop says it last signed in to in this profile
    /// (`lastKnownAccountUuid`), to assign without guessing.
    pub signed_in: Option<String>,
    /// Its limits as Claude Desktop last recorded them (see [`limits`]).
    pub limits: Option<limits::Limits>,
}

/// What the window shows of Claude Desktop: names, folder ids and display
/// names. Never a token or a cookie.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    /// `missing` (no Claude Desktop), `notSetUp` (a plain app folder),
    /// `ready`, or `broken` (see `message`).
    pub state: &'static str,
    pub message: Option<String>,
    pub current: Option<String>,
    pub running: bool,
    pub root: String,
    pub profiles: Vec<ProfileView>,
}

impl Overview {
    pub fn broken(message: String) -> Overview {
        Overview {
            state: "broken",
            message: Some(message),
            current: None,
            running: false,
            root: String::new(),
            profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub name: String,
    pub pair: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    /// `alreadyPresent`, `missingTranscript`, `ambiguousTranscript`,
    /// `filenameConflict`, `noSessionId`, `invalidCard`, `notAFile`,
    /// `overLimit`.
    pub reason: &'static str,
    pub count: usize,
}

/// What a sync added, or would add.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub dry_run: bool,
    pub target: String,
    pub pair: String,
    pub sources: Vec<SourceView>,
    /// Profiles left out for having no account assigned.
    pub unassigned: Vec<String>,
    /// Cards added, or that would be.
    pub added: usize,
    pub skipped: Vec<Skipped>,
    /// Up to eight cards that could not be used, and why.
    pub details: Vec<String>,
    pub backup: Option<String>,
}

/// The disk space one profile takes, and how much of it a clean-up frees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStorage {
    pub name: String,
    pub current: bool,
    pub bytes: u64,
    /// Chromium caches, at any depth.
    pub caches: u64,
    /// Claude Code versions older than the newest one in the profile.
    pub old_versions: u64,
}

/// Where the disk space under the profiles goes. Reads only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    pub profiles: Vec<ProfileStorage>,
    pub backups: u64,
    /// Backups past the newest [`KEEP_BACKUPS`] of each kind.
    pub old_backups: u64,
    pub shared: u64,
    /// `~/.claude/projects`: the transcripts, never touched here.
    pub transcripts: u64,
    pub keep: usize,
}

/// What a clean-up removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    /// The profiles cleaned: every one but the one in use.
    pub profiles: Vec<String>,
    pub freed: u64,
    pub backups_removed: usize,
}

#[derive(Debug)]
pub struct SwitchReport {
    pub from: String,
    pub to: String,
    pub switched: bool,
    pub backup: Option<PathBuf>,
    /// The automatic sync into `to`, when it is on. The switch stands even
    /// when this failed.
    pub sync: Option<Result<SyncReport>>,
}

struct Card {
    filename: OsString,
    payload: Vec<u8>,
}

struct Plan {
    dest: PathBuf,
    pair: String,
    cards: Vec<Card>,
    counts: BTreeMap<&'static str, usize>,
    details: Vec<String>,
}

/// One change at a time, with `claude-switch` too: an exclusive `flock` on
/// the same lock file, released when dropped.
pub struct Lock(#[allow(dead_code)] fs::File);

pub struct Desktop<A: DesktopApp> {
    paths: Paths,
    app: A,
    keep: usize,
}

/// [`KEEP_BACKUPS`], or `CLAUDE_SWITCH_KEEP_BACKUPS`; a value that is not a
/// whole number keeps every backup rather than stop a switch half-way.
fn keep_from_env() -> usize {
    match std::env::var("CLAUDE_SWITCH_KEEP_BACKUPS") {
        Err(_) => KEEP_BACKUPS,
        Ok(value) => value.trim().parse().unwrap_or_else(|_| {
            eprintln!("warning: CLAUDE_SWITCH_KEEP_BACKUPS={value} is not a whole number; keeping every backup");
            0
        }),
    }
}

/// `20261020-091500-1a2b3c4d`, as [`fsx::stamp`] makes it.
fn is_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 24
        && b[8] == b'-'
        && b[15] == b'-'
        && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit)
        && b[16..].iter().all(u8::is_ascii_hexdigit)
}

/// `2.1.309` as numbers, to find the newest; anything else is not a version.
fn version(name: &str) -> Option<Vec<u64>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

impl<A: DesktopApp> Desktop<A> {
    pub fn new(paths: Paths, app: A) -> Result<Self> {
        let home = fsx::resolve_lenient(&paths.home);
        if paths.root == home || paths.root == Path::new("/") {
            return Err(refuse(
                Code::Unsafe,
                "the profiles folder must be a folder of its own",
            ));
        }
        for other in [&paths.app_data, &paths.config] {
            if paths.root.starts_with(other) || other.starts_with(&paths.root) {
                return Err(refuse(
                    Code::Unsafe,
                    "the profiles, app and Claude Code folders must not sit inside one another",
                ));
            }
        }
        Ok(Desktop {
            paths,
            app,
            keep: keep_from_env(),
        })
    }

    /// Keep `keep` backups of each kind per profile (`0`: all of them).
    pub fn keeping(mut self, keep: usize) -> Self {
        self.keep = keep;
        self
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    fn meta(&self) -> PathBuf {
        self.paths.root.join("_meta")
    }
    fn backups(&self) -> PathBuf {
        self.paths.root.join("_backups")
    }
    fn shared(&self) -> PathBuf {
        self.paths.root.join("_shared")
    }

    fn ensure_meta(&self) -> Result<()> {
        fsx::mkdirs_private(&self.paths.root)?;
        for path in [self.meta(), self.backups()] {
            if fsx::is_symlink(&path) {
                return Err(refuse(
                    Code::Unsafe,
                    format!("refusing to write through a symlink: {}", path.display()),
                ));
            }
            if !path.is_dir() {
                fsx::mkdir_private(&path)?;
            }
        }
        Ok(())
    }

    pub fn lock(&self) -> Result<Lock> {
        self.ensure_meta()?;
        let path = self.meta().join(".claude-switch.lock");
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|e| io(&path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock(file)),
            Err(fs::TryLockError::WouldBlock) => Err(refuse(
                Code::Busy,
                "another profile change (claude-switch or Duo Switch) is running; wait for it to finish",
            )),
            Err(fs::TryLockError::Error(e)) => Err(io(&path, e)),
        }
    }

    /// Quit Claude, run `work`, and open Claude again when it was running
    /// (or always, with `always_open`).
    fn stopped<T>(&self, always_open: bool, work: impl FnOnce() -> Result<T>) -> Result<T> {
        let was_running = self.app.running()?;
        self.app.quit()?;
        let out = work();
        if (was_running || always_open) && self.paths.app_data.is_dir() {
            if let Err(e) = self.app.open() {
                eprintln!("warning: open Claude yourself: {e}");
            }
        }
        out
    }

    pub fn profile(&self, name: &str) -> Result<PathBuf> {
        valid_name(name)?;
        let path = self.paths.root.join(name);
        if !fsx::is_real_dir(&path) {
            return Err(refuse(
                Code::UnknownProfile,
                format!("no profile named `{name}`"),
            ));
        }
        Ok(path)
    }

    /// The profile the app folder points to.
    pub fn current(&self) -> Result<String> {
        if !fsx::is_symlink(&self.paths.app_data) {
            return Err(refuse(
                Code::NotSetUp,
                "Claude Desktop is not using profiles yet",
            ));
        }
        let target =
            fs::canonicalize(&self.paths.app_data).map_err(|e| io(&self.paths.app_data, e))?;
        if target.parent() != Some(self.paths.root.as_path()) {
            return Err(refuse(
                Code::Unsafe,
                format!(
                    "the Claude folder points to {}, not straight into {}",
                    target.display(),
                    self.paths.root.display()
                ),
            ));
        }
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| refuse(Code::Unsafe, "the profile's name is not text"))?
            .to_owned();
        self.profile(&name)?;
        Ok(name)
    }

    pub fn profiles(&self) -> Result<Vec<String>> {
        if !self.paths.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.paths.root).map_err(|e| io(&self.paths.root, e))? {
            let entry = entry.map_err(|e| io(&self.paths.root, e))?;
            if let Some(name) = entry.file_name().to_str() {
                if fsx::is_real_dir(&entry.path()) && valid_name(name).is_ok() {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// A profile's `account/organization` folders.
    pub fn pairs(&self, name: &str) -> Result<Vec<String>> {
        let base = self.profile(name)?.join(SESSIONS);
        if fsx::is_symlink(&base) {
            return Err(refuse(
                Code::Unsafe,
                format!("the session cards folder is a symlink: {}", base.display()),
            ));
        }
        if !base.is_dir() {
            return Ok(Vec::new());
        }
        let ids = |dir: &Path| -> Result<Vec<String>> {
            let mut out = Vec::new();
            for entry in fs::read_dir(dir).map_err(|e| io(dir, e))? {
                let entry = entry.map_err(|e| io(dir, e))?;
                if let Some(id) = entry.file_name().to_str() {
                    if is_id(id) && fsx::is_real_dir(&entry.path()) {
                        out.push(id.to_owned());
                    }
                }
            }
            out.sort();
            Ok(out)
        };
        let mut pairs = Vec::new();
        for account in ids(&base)? {
            for org in ids(&base.join(&account))? {
                pairs.push(format!("{account}/{org}"));
            }
        }
        Ok(pairs)
    }

    fn pair_path(&self, name: &str, pair: &str) -> Result<PathBuf> {
        if !self.pairs(name)?.iter().any(|p| p == pair) {
            return Err(refuse(
                Code::UnknownPair,
                format!("no account/organization {pair} in profile `{name}`"),
            ));
        }
        Ok(self.profile(name)?.join(SESSIONS).join(pair))
    }

    /// The `account/organization` assigned to a profile.
    pub fn mapping(&self, name: &str) -> Result<Option<String>> {
        let path = self.meta().join(format!("{name}.account"));
        if fsx::is_symlink(&path) {
            return Err(refuse(
                Code::Unsafe,
                format!("the account file is a symlink: {}", path.display()),
            ));
        }
        if !fsx::is_real_file(&path) {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).map_err(|e| io(&path, e))?;
        let pair = text.trim().to_owned();
        self.pair_path(name, &pair)?;
        Ok(Some(pair))
    }

    fn required_mapping(&self, name: &str) -> Result<String> {
        self.mapping(name)?.ok_or_else(|| {
            refuse(
                Code::NotAssigned,
                format!("profile `{name}` has no account assigned yet"),
            )
        })
    }

    /// Write a `_meta` file; what it said before goes to `_backups` first.
    fn set_meta(&self, path: &Path, payload: &[u8]) -> Result<()> {
        self.ensure_meta()?;
        if fsx::exists(path) {
            if !fsx::is_real_file(path) {
                return Err(refuse(
                    Code::Unsafe,
                    format!("not a settings file: {}", path.display()),
                ));
            }
            let name = path.file_name().expect("a file").to_string_lossy();
            fsx::copy_entry(
                path,
                &self.backups().join(format!("{name}-{}", fsx::stamp())),
            )?;
            fsx::replace_write(path, payload)
        } else {
            fsx::exclusive_write(path, payload)
        }
    }

    fn autosync_path(&self, name: &str) -> PathBuf {
        self.meta().join(format!("{name}.autosync.json"))
    }

    pub fn autosync(&self, name: &str) -> Result<AutoSync> {
        let path = self.autosync_path(name);
        if !fsx::exists(&path) {
            return Ok(AutoSync::default());
        }
        let config = card::read(&path)?;
        let enabled = config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sources = match config.get("sources") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|s| s.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    refuse(
                        Code::InvalidAutosync,
                        format!("auto-sync setting of `{name}` is not valid"),
                    )
                })?,
            Some(_) => {
                return Err(refuse(
                    Code::InvalidAutosync,
                    format!("auto-sync setting of `{name}` is not valid"),
                ))
            }
        };
        if enabled && sources.is_empty() {
            return Err(refuse(
                Code::InvalidAutosync,
                format!("auto-sync of `{name}` is on but names no source"),
            ));
        }
        Ok(AutoSync { enabled, sources })
    }

    /// Assign `pair` to profile `name`. Auto-sync goes off: a new identity
    /// is turned on again by hand, never inherited.
    pub fn assign(&self, name: &str, pair: &str) -> Result<()> {
        let _lock = self.lock()?;
        let pairs = self.pairs(name)?;
        if pairs.is_empty() {
            return Err(refuse(
                Code::NoSessions,
                "no Code data in this profile yet: open the right account, start one Code Local session, end it, then try again",
            ));
        }
        if !pairs.iter().any(|p| p == pair) {
            return Err(refuse(
                Code::UnknownPair,
                format!("no account/organization {pair} in profile `{name}`"),
            ));
        }
        let account = pair.split('/').next().unwrap_or_default();
        for other in self.profiles()? {
            if other == name {
                continue;
            }
            if let Some(theirs) = self.mapping(&other)? {
                if theirs.split('/').next() == Some(account) {
                    return Err(refuse(
                        Code::AccountTaken,
                        format!("this account is already assigned to profile `{other}`; check which account the second profile is signed in to"),
                    ));
                }
            }
        }
        self.set_meta(
            &self.meta().join(format!("{name}.account")),
            format!("{pair}\n").as_bytes(),
        )?;
        self.set_meta(
            &self.autosync_path(name),
            &card::to_bytes(&json!({"enabled": false})),
        )
    }

    /// Take the account off a profile, keeping the old file in `_backups`;
    /// auto-sync goes off.
    pub fn unassign(&self, name: &str) -> Result<()> {
        let _lock = self.lock()?;
        self.profile(name)?;
        let path = self.meta().join(format!("{name}.account"));
        if fsx::exists(&path) {
            if !fsx::is_real_file(&path) {
                return Err(refuse(Code::Unsafe, "the account file is not a file"));
            }
            let to = self
                .backups()
                .join(format!("{name}.account-reset-{}", fsx::stamp()));
            fs::rename(&path, &to).map_err(|e| io(&path, e))?;
        }
        self.set_meta(
            &self.autosync_path(name),
            &card::to_bytes(&json!({"enabled": false})),
        )
    }

    /// The account Claude Desktop last signed in to in profile `name`, from
    /// its `config.json`. Only that one field is kept; the rest of the file,
    /// a sign-in cache among it, is read past and dropped.
    pub fn signed_in(&self, name: &str) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct Config {
            #[serde(rename = "lastKnownAccountUuid")]
            account: Option<String>,
        }
        let path = self.paths.root.join(name).join("config.json");
        let meta = fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
            return None;
        }
        let config: Config = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
        config.account.filter(|a| is_id(a))
    }

    fn label_path(&self, name: &str) -> PathBuf {
        self.meta().join(format!("{name}.label"))
    }

    /// The name shown for a profile's account, set by hand: one line in
    /// `_meta/<name>.label`, a file `claude-switch` leaves alone.
    pub fn label(&self, name: &str) -> Option<String> {
        let path = self.label_path(name);
        if !fsx::is_real_file(&path) {
            return None;
        }
        let text = fs::read_to_string(&path).ok()?;
        let line = text.lines().next()?.trim();
        (!line.is_empty()).then(|| line.chars().take(MAX_LABEL).collect())
    }

    /// Set or clear (`None` or blank) the name shown for a profile's account.
    /// A cleared one goes to `_backups`.
    pub fn set_label(&self, name: &str, label: Option<&str>) -> Result<()> {
        let _lock = self.lock()?;
        self.profile(name)?;
        let path = self.label_path(name);
        match label.map(str::trim).filter(|l| !l.is_empty()) {
            Some(label) => {
                if label.chars().count() > MAX_LABEL || label.contains(['\n', '\r']) {
                    return Err(refuse(
                        Code::InvalidLabel,
                        format!("a display name is one line of at most {MAX_LABEL} characters"),
                    ));
                }
                self.set_meta(&path, format!("{label}\n").as_bytes())
            }
            None if fsx::exists(&path) => {
                if !fsx::is_real_file(&path) {
                    return Err(refuse(Code::Unsafe, "the display name file is not a file"));
                }
                let to = self
                    .backups()
                    .join(format!("{name}.label-reset-{}", fsx::stamp()));
                fs::rename(&path, &to).map_err(|e| io(&path, e))
            }
            None => Ok(()),
        }
    }

    /// Turn auto-sync on (from every other profile with an account) or off.
    pub fn set_autosync(&self, name: &str, on: bool) -> Result<AutoSync> {
        let _lock = self.lock()?;
        self.profile(name)?;
        let setting = if on {
            self.required_mapping(name)?;
            let (sources, _) = self.source_profiles(name, None)?;
            AutoSync {
                enabled: true,
                sources,
            }
        } else {
            AutoSync::default()
        };
        self.set_meta(
            &self.autosync_path(name),
            &card::to_bytes(&json!({"enabled": setting.enabled, "sources": setting.sources})),
        )?;
        Ok(setting)
    }

    /// How many non-empty transcripts each session id has.
    fn transcript_index(&self) -> Result<HashMap<String, usize>> {
        let history = self.paths.config.join("projects");
        if !history.is_dir() {
            return Err(refuse(
                Code::NoHistory,
                format!("no Code Local history folder: {}", history.display()),
            ));
        }
        fn walk(dir: &Path, index: &mut HashMap<String, usize>) -> Result<()> {
            for entry in fs::read_dir(dir).map_err(|e| io(dir, e))? {
                let entry = entry.map_err(|e| io(dir, e))?;
                let path = entry.path();
                let meta = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
                if meta.is_dir() {
                    walk(&path, index)?;
                } else if meta.is_file()
                    && meta.len() > 0
                    && path.extension().is_some_and(|e| e == "jsonl")
                {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        *index.entry(stem.to_owned()).or_default() += 1;
                    }
                }
            }
            Ok(())
        }
        let mut index = HashMap::new();
        walk(&history, &mut index)?;
        Ok(index)
    }

    /// Copy a profile, less its caches, with its `_meta` files; then drop the
    /// profile's backups past the newest few.
    pub fn backup(&self, name: &str) -> Result<PathBuf> {
        let source = self.profile(name)?;
        self.ensure_meta()?;
        let dest = self.backups().join(format!("{name}-{}", fsx::stamp()));
        fsx::mkdir_private(&dest)?;
        let target = dest.join("profile");
        fsx::mkdir_private(&target)?;
        for entry in fs::read_dir(&source).map_err(|e| io(&source, e))? {
            let entry = entry.map_err(|e| io(&source, e))?;
            let file_name = entry.file_name();
            let n = file_name.to_string_lossy();
            if EXCLUDES.contains(&n.as_ref())
                || CACHE_DIRS.contains(&n.as_ref())
                || n.starts_with("claude-code-sessions.bak-")
            {
                continue;
            }
            fsx::copy_entry_skipping(&entry.path(), &target.join(&file_name), &CACHE_DIRS)?;
        }
        for suffix in [".account", ".autosync.json", ".label"] {
            let meta = self.meta().join(format!("{name}{suffix}"));
            if fsx::is_real_file(&meta) {
                fsx::copy_entry(&meta, &dest.join(meta.file_name().expect("a file")))?;
            }
        }
        fsx::exclusive_write(
            &dest.join("manifest.json"),
            &card::to_bytes(&json!({
                "complete": true,
                "kind": "profile",
                "profile": name,
                "source": source,
                "history_included": false,
                "history_source": self.paths.config.join("projects"),
                "excludes": EXCLUDES,
                "cache_dirs_excluded_at_any_depth": CACHE_DIRS,
                "note": "Not included: the macOS Keychain, or project and worktree folders.",
            })),
        )?;
        self.prune(name, "profile", &format!("{name}-"));
        Ok(dest)
    }

    /// Before a sync, the names of the cards it is about to add. A sync only
    /// adds cards and never writes over one, so this list is all it takes to
    /// undo it: delete those files from `source`.
    fn session_backup(
        &self,
        name: &str,
        pair: &str,
        path: &Path,
        cards: &[Card],
    ) -> Result<PathBuf> {
        self.ensure_meta()?;
        let dest = self
            .backups()
            .join(format!("sessions-{name}-{}", fsx::stamp()));
        fsx::mkdir_private(&dest)?;
        let added: Vec<String> = cards
            .iter()
            .map(|c| c.filename.to_string_lossy().into_owned())
            .collect();
        fsx::exclusive_write(
            &dest.join("manifest.json"),
            &card::to_bytes(&json!({
                "complete": true,
                "kind": "session-cards",
                "profile": name,
                "account_org": pair,
                "source": path,
                "added": added,
                "note": "The sync only adds these cards to `source`; deleting them undoes it.",
            })),
        )?;
        self.prune(name, "session-cards", &format!("sessions-{name}-"));
        Ok(dest)
    }

    /// The backups of one kind of profile `name`, oldest first: folders named
    /// `<prefix><stamp>` whose complete manifest says so. A backup that holds
    /// the transcripts, or anything else, is never one of them.
    fn backups_of(&self, name: &str, kind: &str, prefix: &str) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.backups()) else {
            return Vec::new();
        };
        let mut found: Vec<(String, std::time::SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let file_name = e.file_name().to_string_lossy().into_owned();
                let stamp = file_name.strip_prefix(prefix)?;
                if !is_stamp(stamp) || !fsx::is_real_dir(&e.path()) {
                    return None;
                }
                let manifest_path = e.path().join("manifest.json");
                if !fsx::is_real_file(&manifest_path) {
                    return None;
                }
                let manifest: Value =
                    serde_json::from_slice(&fs::read(&manifest_path).ok()?).ok()?;
                let ours = manifest["complete"] == true
                    && manifest["kind"] == kind
                    && manifest["profile"] == name
                    && manifest["history_included"] != true;
                let written = fs::metadata(&manifest_path)
                    .and_then(|m| m.modified())
                    .ok()?;
                ours.then(|| (stamp[..15].to_owned(), written, e.path()))
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, _, path)| path).collect()
    }

    /// The backups of one kind of `name` past the newest [`KEEP_BACKUPS`].
    fn old_backups(&self, name: &str, kind: &str, prefix: &str) -> Vec<PathBuf> {
        if self.keep == 0 {
            return Vec::new();
        }
        let mut found = self.backups_of(name, kind, prefix);
        found.truncate(found.len().saturating_sub(self.keep));
        found
    }

    /// Delete the old backups of one kind of `name`. A backup that will not
    /// go is said, not raised: the switch or sync it follows stands.
    fn prune(&self, name: &str, kind: &str, prefix: &str) -> usize {
        let mut removed = 0;
        for path in self.old_backups(name, kind, prefix) {
            match fs::remove_dir_all(&path) {
                Ok(()) => removed += 1,
                Err(e) => eprintln!("warning: could not remove {}: {e}", path.display()),
            }
        }
        removed
    }

    /// The caches in `profile` (a profile folder), at any depth, left as they
    /// are where the app keeps code, the VM or sessions.
    fn caches_in(profile: &Path) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !fsx::is_real_dir(&path) {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if CACHE_DIRS.contains(&name.as_ref()) {
                    out.push(path);
                } else if !KEEP_OUT.contains(&name.as_ref())
                    && !name.starts_with("claude-code-sessions")
                {
                    walk(&path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(profile, &mut out);
        out
    }

    /// The Claude Code versions in `profile` older than its newest one.
    fn old_versions_in(profile: &Path) -> Vec<PathBuf> {
        let dir = profile.join("claude-code");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut versions: Vec<(Vec<u64>, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| fsx::is_real_dir(&e.path()))
            .filter_map(|e| Some((version(&e.file_name().to_string_lossy())?, e.path())))
            .collect();
        versions.sort();
        versions.pop();
        versions.into_iter().map(|(_, path)| path).collect()
    }

    /// Where the disk space goes: each profile, what a clean-up would free,
    /// the backups, the shared VM and the transcripts. Reads only.
    pub fn storage(&self) -> Result<Storage> {
        let current = self.current().ok();
        let mut old_backups = 0;
        let mut profiles = Vec::new();
        for name in self.profiles()? {
            let path = self.paths.root.join(&name);
            let size = |paths: Vec<PathBuf>| paths.iter().map(|p| fsx::disk_usage(p)).sum();
            old_backups += size(self.old_backups(&name, "profile", &format!("{name}-")))
                + size(self.old_backups(&name, "session-cards", &format!("sessions-{name}-")));
            profiles.push(ProfileStorage {
                current: current.as_deref() == Some(name.as_str()),
                bytes: fsx::disk_usage(&path),
                caches: size(Self::caches_in(&path)),
                old_versions: size(Self::old_versions_in(&path)),
                name,
            });
        }
        Ok(Storage {
            profiles,
            backups: fsx::disk_usage(&self.backups()),
            old_backups,
            shared: fsx::disk_usage(&self.shared()),
            transcripts: fsx::disk_usage(&self.paths.config.join("projects")),
            keep: self.keep,
        })
    }

    /// Free the space the app makes again by itself: the caches and older
    /// Claude Code versions of every profile but the one in use, and the
    /// backups past the newest few. Sessions, logins and transcripts stay.
    pub fn clean(&self) -> Result<CleanReport> {
        let _lock = self.lock()?;
        let current = self.current()?;
        let mut report = CleanReport {
            profiles: Vec::new(),
            freed: 0,
            backups_removed: 0,
        };
        for name in self.profiles()? {
            for (kind, prefix) in [
                ("profile", format!("{name}-")),
                ("session-cards", format!("sessions-{name}-")),
            ] {
                for path in self.old_backups(&name, kind, &prefix) {
                    let bytes = fsx::disk_usage(&path);
                    fs::remove_dir_all(&path).map_err(|e| io(&path, e))?;
                    report.freed += bytes;
                    report.backups_removed += 1;
                }
            }
            if name == current {
                continue;
            }
            let path = self.paths.root.join(&name);
            for dir in Self::caches_in(&path)
                .into_iter()
                .chain(Self::old_versions_in(&path))
            {
                let bytes = fsx::disk_usage(&dir);
                fs::remove_dir_all(&dir).map_err(|e| io(&dir, e))?;
                report.freed += bytes;
            }
            report.profiles.push(name);
        }
        Ok(report)
    }

    /// The profiles to take cards from, and those left out for having no
    /// account: `names`, or every other profile with an account.
    fn source_profiles(
        &self,
        target: &str,
        names: Option<&[String]>,
    ) -> Result<(Vec<String>, Vec<String>)> {
        if let Some(names) = names.filter(|n| !n.is_empty()) {
            let mut sources = names.to_vec();
            sources.sort();
            sources.dedup();
            for name in &sources {
                self.profile(name)?;
                if name == target {
                    return Err(refuse(
                        Code::NoSources,
                        "the source and the target are the same profile",
                    ));
                }
                self.required_mapping(name)?;
            }
            return Ok((sources, Vec::new()));
        }
        let mut sources = Vec::new();
        let mut unassigned = Vec::new();
        for name in self.profiles()? {
            if name == target {
                continue;
            }
            if self.mapping(&name)?.is_some() {
                sources.push(name);
            } else {
                unassigned.push(name);
            }
        }
        if sources.is_empty() {
            return Err(refuse(
                Code::NoSources,
                "no other profile has an account assigned yet",
            ));
        }
        Ok((sources, unassigned))
    }

    fn plan(&self, target: &str, sources: &[String], limit: Option<usize>) -> Result<Plan> {
        let pair = self.required_mapping(target)?;
        let dest = self.pair_path(target, &pair)?;
        let index = self.transcript_index()?;
        let mut have = HashSet::new();
        let mut names: HashSet<OsString> = HashSet::new();
        for entry in fs::read_dir(&dest).map_err(|e| io(&dest, e))? {
            names.insert(entry.map_err(|e| io(&dest, e))?.file_name());
        }
        // A target card that cannot be read could hide a duplicate: stop.
        for file in fsx::card_names(&dest)? {
            let data = card::read(&file)?;
            if !data.is_object() {
                return Err(refuse(
                    Code::InvalidCard,
                    format!("a target card is not a JSON object: {}", file.display()),
                ));
            }
            if let Some(id) = card::session_id(&data)? {
                have.insert(id);
            }
        }

        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut candidates = Vec::new();
        for source in sources {
            let source_pair = self.required_mapping(source)?;
            for file in fsx::card_names(&self.pair_path(source, &source_pair)?)? {
                if !fsx::is_real_file(&file) {
                    *counts.entry("notAFile").or_default() += 1;
                    continue;
                }
                let modified = fs::metadata(&file)
                    .and_then(|m| m.modified())
                    .map_err(|e| io(&file, e))?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                candidates.push((modified, file, source_pair.clone()));
            }
        }
        // Newest first: with a limit, the latest sessions go over first.
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

        enum Seen {
            Skip(&'static str),
            Card(String, Vec<u8>),
        }
        let mut cards = Vec::new();
        let mut details = Vec::new();
        for (_, file, source_pair) in candidates {
            let filename = file.file_name().expect("a card has a name").to_owned();
            let seen = (|| -> Result<Seen> {
                let data = card::read(&file)?;
                if !data.is_object() {
                    return Err(refuse(Code::InvalidCard, "the card is not a JSON object"));
                }
                let Some(id) = card::session_id(&data)? else {
                    return Ok(Seen::Skip("noSessionId"));
                };
                if have.contains(&id) {
                    return Ok(Seen::Skip("alreadyPresent"));
                }
                match index.get(&id).copied().unwrap_or(0) {
                    0 => return Ok(Seen::Skip("missingTranscript")),
                    1 => {}
                    _ => return Ok(Seen::Skip("ambiguousTranscript")),
                }
                if names.contains(&filename) {
                    return Ok(Seen::Skip("filenameConflict"));
                }
                let payload = card::to_bytes(&card::transform(&data, &source_pair, &pair)?);
                Ok(Seen::Card(id, payload))
            })();
            match seen {
                Ok(Seen::Skip(reason)) => *counts.entry(reason).or_default() += 1,
                Err(e) => {
                    *counts.entry("invalidCard").or_default() += 1;
                    if details.len() < 8 {
                        details.push(format!("{}: {e}", filename.to_string_lossy()));
                    }
                }
                Ok(Seen::Card(id, payload)) => {
                    if limit.is_some_and(|l| cards.len() >= l) {
                        *counts.entry("overLimit").or_default() += 1;
                        continue;
                    }
                    have.insert(id);
                    names.insert(filename.clone());
                    cards.push(Card { filename, payload });
                }
            }
        }
        Ok(Plan {
            dest,
            pair,
            cards,
            counts,
            details,
        })
    }

    fn report(
        &self,
        target: &str,
        sources: &[String],
        unassigned: Vec<String>,
        plan: &Plan,
        dry_run: bool,
    ) -> Result<SyncReport> {
        Ok(SyncReport {
            dry_run,
            target: target.to_owned(),
            pair: plan.pair.clone(),
            sources: sources
                .iter()
                .map(|name| {
                    Ok(SourceView {
                        name: name.clone(),
                        pair: self.required_mapping(name)?,
                    })
                })
                .collect::<Result<_>>()?,
            unassigned,
            added: plan.cards.len(),
            skipped: plan
                .counts
                .iter()
                .map(|(&reason, &count)| Skipped { reason, count })
                .collect(),
            details: plan.details.clone(),
            backup: None,
        })
    }

    /// What a sync into the profile in use would add; reads only.
    pub fn preview(&self, sources: Option<&[String]>, limit: Option<usize>) -> Result<SyncReport> {
        let target = self.current()?;
        let (sources, unassigned) = self.source_profiles(&target, sources)?;
        let plan = self.plan(&target, &sources, limit)?;
        self.report(&target, &sources, unassigned, &plan, true)
    }

    /// Add the other profiles' cards to the profile in use, with Claude
    /// quit meanwhile.
    pub fn sync(&self, sources: Option<&[String]>, limit: Option<usize>) -> Result<SyncReport> {
        let _lock = self.lock()?;
        let target = self.current()?;
        let (sources, unassigned) = self.source_profiles(&target, sources)?;
        self.stopped(false, || {
            self.sync_stopped(&target, &sources, unassigned, limit)
        })
    }

    fn sync_stopped(
        &self,
        target: &str,
        sources: &[String],
        unassigned: Vec<String>,
        limit: Option<usize>,
    ) -> Result<SyncReport> {
        let plan = self.plan(target, sources, limit)?;
        let mut report = self.report(target, sources, unassigned, &plan, false)?;
        if plan.cards.is_empty() {
            return Ok(report);
        }
        if self.app.running()? {
            return Err(refuse(
                Code::AppReopened,
                "Claude opened again before the sync; quit it and try again",
            ));
        }
        let backup = self.session_backup(target, &plan.pair, &plan.dest, &plan.cards)?;
        report.backup = Some(backup.display().to_string());
        for (written, card) in plan.cards.iter().enumerate() {
            let wrote = if self.app.running()? {
                Err(refuse(
                    Code::AppReopened,
                    "Claude opened again during the sync",
                ))
            } else {
                fsx::exclusive_write(&plan.dest.join(&card.filename), &card.payload)
            };
            if let Err(e) = wrote {
                return Err(refuse(
                    Code::SyncIncomplete,
                    format!(
                        "the sync stopped after {written} cards: {e}. The sources are as they were; the cards before it are in {}",
                        backup.display()
                    ),
                ));
            }
        }
        Ok(report)
    }

    /// Point the app folder at a profile, in one rename.
    fn point_to(&self, name: &str) -> Result<()> {
        let target = self.profile(name)?;
        let app_data = &self.paths.app_data;
        if fsx::exists(app_data) && !fsx::is_symlink(app_data) {
            return Err(refuse(
                Code::Unsafe,
                "refusing to replace the real app folder",
            ));
        }
        let parent = app_data.parent().expect("the app folder has a parent");
        let temp = parent.join(format!(".claude-switch-link-{}", fsx::hex8()));
        std::os::unix::fs::symlink(&target, &temp).map_err(|e| io(&temp, e))?;
        fs::rename(&temp, app_data).map_err(|e| {
            let _ = fs::remove_file(&temp);
            io(app_data, e)
        })
    }

    /// A new profile shares the VM bundles instead of downloading its own.
    fn share_for_new(&self, profile: &Path) -> Result<()> {
        let source = self.shared().join("vm_bundles");
        let link = profile.join("vm_bundles");
        if fsx::is_real_dir(&source) && !fsx::exists(&link) {
            std::os::unix::fs::symlink(&source, &link).map_err(|e| io(&link, e))?;
        }
        Ok(())
    }

    /// Make the app's own folder the first profile, `name`, after copying
    /// it whole to `_backups`.
    pub fn setup(&self, name: &str) -> Result<()> {
        valid_name(name)?;
        let _lock = self.lock()?;
        let app_data = &self.paths.app_data;
        if fsx::is_symlink(app_data) {
            return Err(refuse(
                Code::AlreadySetUp,
                "Claude Desktop already uses profiles",
            ));
        }
        if !fsx::is_real_dir(app_data) {
            return Err(refuse(
                Code::NoAppData,
                "no Claude Desktop data: open Claude and sign in first",
            ));
        }
        let target = self.paths.root.join(name);
        if fsx::exists(&target) {
            return Err(refuse(
                Code::ProfileExists,
                format!("profile `{name}` already exists"),
            ));
        }
        self.stopped(false, || {
            let snapshot = self
                .backups()
                .join(format!("pre-setup-{name}-{}", fsx::stamp()));
            fsx::mkdir_private(&snapshot)?;
            fsx::copy_entry(app_data, &snapshot.join("profile"))?;
            fsx::exclusive_write(
                &snapshot.join("manifest.json"),
                &card::to_bytes(
                    &json!({"complete": true, "kind": "pre-setup", "source": app_data}),
                ),
            )?;
            // A rename never copies half a folder across file systems.
            fs::rename(app_data, &target).map_err(|e| io(app_data, e))?;
            if let Err(e) = self.point_to(name) {
                if !fsx::exists(app_data) {
                    let _ = fs::rename(&target, app_data);
                }
                return Err(e);
            }
            Ok(())
        })
    }

    /// A new, empty profile, switched to so its account can sign in.
    pub fn add(&self, name: &str) -> Result<()> {
        valid_name(name)?;
        let _lock = self.lock()?;
        let current = self.current()?;
        let target = self.paths.root.join(name);
        if fsx::exists(&target) {
            return Err(refuse(
                Code::ProfileExists,
                format!("profile `{name}` already exists"),
            ));
        }
        self.stopped(true, || {
            self.backup(&current)?;
            fsx::mkdir_private(&target)?;
            self.share_for_new(&target)?;
            self.point_to(name)
        })
    }

    /// Switch Claude Desktop to profile `name`: quit it, back up the profile
    /// in use, point the app folder at `name`, run its auto-sync, open it.
    pub fn switch(&self, name: &str) -> Result<SwitchReport> {
        let _lock = self.lock()?;
        self.profile(name)?;
        let current = self.current()?;
        if current == name {
            if !self.app.running()? {
                self.app.open()?;
            }
            return Ok(SwitchReport {
                from: current,
                to: name.to_owned(),
                switched: false,
                backup: None,
                sync: None,
            });
        }
        self.stopped(true, || {
            let backup = if std::env::var("CLAUDE_SWITCH_NO_BACKUP").as_deref() == Ok("1") {
                None
            } else {
                Some(self.backup(&current)?)
            };
            self.point_to(name)?;
            let sync = if std::env::var("CLAUDE_SWITCH_NO_SYNC").as_deref() == Ok("1") {
                None
            } else {
                match self.autosync(name) {
                    Ok(setting) if setting.enabled => {
                        Some(self.source_profiles(name, Some(&setting.sources)).and_then(
                            |(sources, unassigned)| {
                                self.sync_stopped(name, &sources, unassigned, None)
                            },
                        ))
                    }
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                }
            };
            Ok(SwitchReport {
                from: current.clone(),
                to: name.to_owned(),
                switched: true,
                backup,
                sync,
            })
        })
    }

    /// Everything the window shows; what cannot be read is said, not raised.
    pub fn overview(&self) -> Overview {
        let app_data = &self.paths.app_data;
        let running = self.app.running().unwrap_or(false);
        let mut overview = Overview {
            state: "ready",
            message: None,
            current: None,
            running,
            root: self.paths.root.display().to_string(),
            profiles: Vec::new(),
        };
        let names = match self.profiles() {
            Ok(names) => names,
            Err(e) => return Overview::broken(e.to_string()),
        };
        if !fsx::exists(app_data) && names.is_empty() {
            overview.state = "missing";
            return overview;
        }
        if fsx::is_real_dir(app_data) {
            overview.state = "notSetUp";
            return overview;
        }
        match self.current() {
            Ok(name) => overview.current = Some(name),
            Err(e) => {
                overview.state = "broken";
                overview.message = Some(e.to_string());
            }
        }
        overview.profiles = names
            .into_iter()
            .map(|name| {
                let (account, account_error) = match self.mapping(&name) {
                    Ok(pair) => (pair, None),
                    Err(e) => (None, Some(e.to_string())),
                };
                let label = self.label(&name);
                let (autosync, autosync_error) = match self.autosync(&name) {
                    Ok(setting) => (setting, None),
                    Err(e) => (AutoSync::default(), Some(e.to_string())),
                };
                let pairs: Vec<PairView> = self
                    .pairs(&name)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|pair| {
                        let dir = self.paths.root.join(&name).join(SESSIONS).join(&pair);
                        let cards = fsx::card_names(&dir).map(|c| c.len()).unwrap_or(0);
                        PairView { pair, cards }
                    })
                    .collect();
                let signed_in = self.signed_in(&name);
                // The assigned account's limits; before one is assigned, those
                // of the account signed in, when its organization is plain.
                let org = account.clone().or_else(|| {
                    let account = signed_in.as_deref()?;
                    let mut theirs = pairs
                        .iter()
                        .filter(|p| p.pair.split('/').next() == Some(account));
                    let only = theirs.next()?;
                    theirs.next().is_none().then(|| only.pair.clone())
                });
                let limits = org.and_then(|pair| {
                    let org = pair.split('/').nth(1)?.to_owned();
                    limits::read(&self.paths.root.join(&name), &org)
                });
                ProfileView {
                    current: overview.current.as_deref() == Some(name.as_str()),
                    name,
                    account,
                    account_error,
                    pairs,
                    autosync,
                    autosync_error,
                    label,
                    signed_in,
                    limits,
                }
            })
            .collect();
        overview
    }
}

#[cfg(test)]
mod tests;
