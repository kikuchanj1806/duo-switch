use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::control::fake::FakeApp;
use super::*;

struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    d: Desktop<FakeApp>,
}

/// A home with Claude Desktop's own folder (signed in, with caches) and
/// Claude Code's history folder.
fn world_with(app: FakeApp) -> World {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let app_data = home.join("Library/Application Support/Claude");
    fs::create_dir_all(app_data.join("Cache")).unwrap();
    fs::write(app_data.join("Cache/blob"), "cache").unwrap();
    fs::write(app_data.join("Cookies"), "signed in as A").unwrap();
    fs::create_dir_all(home.join(".claude/projects/-Users-me-app")).unwrap();
    let d = Desktop::new(Paths::new(&home, true), app).unwrap();
    World { _dir: dir, home, d }
}

fn world() -> World {
    world_with(FakeApp::default())
}

impl World {
    fn root(&self) -> PathBuf {
        self.d.paths().root.clone()
    }
    fn calls(&self) -> Vec<&'static str> {
        self.d.app.calls.borrow().clone()
    }
    fn sessions(&self, profile: &str, pair: &str) -> PathBuf {
        let dir = self.root().join(profile).join(SESSIONS).join(pair);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn card(&self, profile: &str, pair: &str, file: &str, value: serde_json::Value) {
        fs::write(
            self.sessions(profile, pair).join(file),
            card::to_bytes(&value),
        )
        .unwrap();
    }
    fn transcript(&self, id: &str) {
        fs::write(
            self.home
                .join(format!(".claude/projects/-Users-me-app/{id}.jsonl")),
            "{\"type\":\"user\"}\n",
        )
        .unwrap();
    }
    /// tk1 (account aaa) and tk2 (account bbb), both assigned, tk2 in use.
    fn two_profiles(&self) {
        self.d.setup("tk1").unwrap();
        self.sessions("tk1", "aaa/a0a");
        self.d.assign("tk1", "aaa/a0a").unwrap();
        self.d.add("tk2").unwrap();
        self.sessions("tk2", "bbb/b0b");
        self.d.assign("tk2", "bbb/b0b").unwrap();
    }
    fn backups_named(&self, prefix: &str) -> Vec<PathBuf> {
        fs::read_dir(self.root().join("_backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
            .collect()
    }
}

fn code(result: Result<impl std::fmt::Debug>) -> &'static str {
    match result {
        Err(Error::Desktop { code, .. }) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn setup_makes_the_app_folder_the_first_profile_after_a_full_copy() {
    let w = world();
    assert_eq!(w.d.overview().state, "notSetUp");
    w.d.setup("tk1").unwrap();
    let app_data = &w.d.paths().app_data;
    assert!(fs::symlink_metadata(app_data)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(w.d.current().unwrap(), "tk1");
    assert_eq!(
        fs::read(w.root().join("tk1/Cookies")).unwrap(),
        b"signed in as A"
    );
    let snapshot = &w.backups_named("pre-setup-tk1-")[0];
    assert!(
        snapshot.join("profile/Cache/blob").exists(),
        "setup copies everything"
    );
    assert_eq!(read_json(&snapshot.join("manifest.json"))["complete"], true);
    // Claude was not running, so it is not opened.
    assert_eq!(w.calls(), ["quit"]);
    assert_eq!(code(w.d.setup("tk1")), "desktopAlreadySetUp");
}

#[test]
fn add_makes_an_empty_profile_backs_up_the_one_in_use_and_opens_claude() {
    let w = world();
    w.d.setup("tk1").unwrap();
    fs::create_dir_all(w.root().join("_shared/vm_bundles")).unwrap();
    w.d.add("tk2").unwrap();
    assert_eq!(w.d.current().unwrap(), "tk2");
    assert!(
        !w.root().join("tk2/Cookies").exists(),
        "a new profile carries no login"
    );
    assert!(fs::symlink_metadata(w.root().join("tk2/vm_bundles"))
        .unwrap()
        .file_type()
        .is_symlink());
    let backup = &w.backups_named("tk1-")[0];
    assert!(backup.join("profile/Cookies").exists());
    assert!(
        !backup.join("profile/Cache").exists(),
        "caches stay out of a backup"
    );
    assert_eq!(w.calls().last(), Some(&"open"));
    assert_eq!(code(w.d.add("tk2")), "desktopProfileExists");
    assert_eq!(code(w.d.add("sync")), "desktopInvalidName");
}

#[test]
fn switch_quits_backs_up_relinks_and_opens_again() {
    let w = world();
    w.d.setup("tk1").unwrap();
    w.d.add("tk2").unwrap();
    *w.d.app.running.borrow_mut() = true;
    w.d.app.calls.borrow_mut().clear();
    let report = w.d.switch("tk1").unwrap();
    assert!(report.switched);
    assert_eq!((report.from.as_str(), report.to.as_str()), ("tk2", "tk1"));
    assert!(report.backup.unwrap().join("manifest.json").exists());
    assert!(report.sync.is_none(), "auto-sync is off by default");
    assert_eq!(w.d.current().unwrap(), "tk1");
    assert_eq!(w.calls(), ["quit", "open"]);
}

#[test]
fn switching_to_the_profile_in_use_only_opens_claude() {
    let w = world();
    w.d.setup("tk1").unwrap();
    let report = w.d.switch("tk1").unwrap();
    assert!(!report.switched);
    assert_eq!(w.calls(), ["quit", "open"]);
    assert_eq!(code(w.d.switch("nobody")), "desktopUnknownProfile");
}

#[test]
fn an_app_that_will_not_quit_stops_the_switch_before_anything_changes() {
    let w = world_with(FakeApp {
        refuses_to_quit: true,
        ..FakeApp::default()
    });
    w.d.setup("tk1").unwrap();
    w.d.add("tk2").unwrap(); // opens Claude, which now will not quit
    assert_eq!(code(w.d.switch("tk1")), "desktopAppWontQuit");
    assert_eq!(w.d.current().unwrap(), "tk2");
}

#[test]
fn the_lock_keeps_one_change_at_a_time() {
    let w = world();
    w.d.setup("tk1").unwrap();
    let held = w.d.lock().unwrap();
    assert_eq!(code(w.d.switch("tk1")), "desktopBusy");
    drop(held);
    w.d.switch("tk1").unwrap();
}

#[test]
fn an_account_is_assigned_to_one_profile_only() {
    let w = world();
    w.d.setup("tk1").unwrap();
    assert_eq!(code(w.d.assign("tk1", "aaa/a0a")), "desktopNoSessions");
    w.sessions("tk1", "aaa/a0a");
    assert_eq!(code(w.d.assign("tk1", "aaa/c0c")), "desktopUnknownPair");
    w.d.assign("tk1", "aaa/a0a").unwrap();
    assert_eq!(w.d.mapping("tk1").unwrap().as_deref(), Some("aaa/a0a"));
    w.d.add("tk2").unwrap();
    // Signed in to the same account again by mistake.
    w.sessions("tk2", "aaa/a0a");
    assert_eq!(code(w.d.assign("tk2", "aaa/a0a")), "desktopAccountTaken");
}

#[test]
fn reassigning_keeps_the_old_file_and_turns_auto_sync_off() {
    let w = world();
    w.two_profiles();
    w.d.set_autosync("tk2", true).unwrap();
    w.sessions("tk2", "bbb/d0d");
    w.d.assign("tk2", "bbb/d0d").unwrap();
    assert!(!w.d.autosync("tk2").unwrap().enabled);
    assert!(!w.backups_named("tk2.account-").is_empty());
    w.d.unassign("tk2").unwrap();
    assert_eq!(w.d.mapping("tk2").unwrap(), None);
}

#[test]
fn sync_adds_cards_with_the_target_identity_and_only_once() {
    let w = world();
    w.two_profiles();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_1.json",
        json!({"cliSessionId": "s1", "accountUuid": "aaa", "organizationUuid": "a0a", "title": "Fix it"}),
    );
    w.transcript("s1");

    let preview = w.d.preview(None, None).unwrap();
    assert!(preview.dry_run);
    assert_eq!(preview.added, 1);
    assert!(!w
        .root()
        .join("tk2")
        .join(SESSIONS)
        .join("bbb/b0b/local_1.json")
        .exists());

    let report = w.d.sync(None, None).unwrap();
    assert_eq!((report.target.as_str(), report.added), ("tk2", 1));
    assert_eq!(report.sources[0].name, "tk1");
    let written = read_json(
        &w.root()
            .join("tk2")
            .join(SESSIONS)
            .join("bbb/b0b/local_1.json"),
    );
    assert_eq!(written["accountUuid"], "bbb");
    assert_eq!(written["organizationUuid"], "b0b");
    assert_eq!(written["title"], "Fix it");
    assert!(Path::new(report.backup.as_deref().unwrap())
        .join("manifest.json")
        .exists());
    // The source card is as it was.
    let source = read_json(
        &w.root()
            .join("tk1")
            .join(SESSIONS)
            .join("aaa/a0a/local_1.json"),
    );
    assert_eq!(source["accountUuid"], "aaa");

    let again = w.d.sync(None, None).unwrap();
    assert_eq!(again.added, 0);
    assert_eq!(
        again.skipped,
        [Skipped {
            reason: "alreadyPresent",
            count: 1
        }]
    );
}

#[test]
fn sync_leaves_out_what_it_cannot_carry_over_safely() {
    let w = world();
    w.two_profiles();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_a.json",
        json!({"cliSessionId": "no-transcript", "accountUuid": "aaa"}),
    );
    w.card(
        "tk1",
        "aaa/a0a",
        "local_b.json",
        json!({"title": "no session id"}),
    );
    w.card(
        "tk1",
        "aaa/a0a",
        "local_c.json",
        json!({"cliSessionId": "s3", "note": "see aaa"}),
    );
    fs::write(
        w.sessions("tk1", "aaa/a0a").join("local_d.json"),
        b"{\"a\":1,\"a\":2}",
    )
    .unwrap();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_e.json",
        json!({"cliSessionId": "s5"}),
    );
    w.card(
        "tk2",
        "bbb/b0b",
        "local_e.json",
        json!({"cliSessionId": "other"}),
    );
    for id in ["s3", "s5"] {
        w.transcript(id);
    }
    let report = w.d.sync(None, None).unwrap();
    assert_eq!(report.added, 0);
    let skipped: BTreeMap<_, _> = report.skipped.iter().map(|s| (s.reason, s.count)).collect();
    assert_eq!(
        skipped,
        BTreeMap::from([
            ("filenameConflict", 1),
            ("invalidCard", 2),
            ("missingTranscript", 1),
            ("noSessionId", 1),
        ])
    );
    assert_eq!(report.details.len(), 2);
}

#[test]
fn auto_sync_runs_when_switching_in() {
    let w = world();
    w.two_profiles();
    let on = w.d.set_autosync("tk2", true).unwrap();
    assert_eq!(on.sources, ["tk1"]);
    w.d.switch("tk1").unwrap();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_9.json",
        json!({"cliSessionId": "s9", "accountUuid": "aaa"}),
    );
    w.transcript("s9");
    let report = w.d.switch("tk2").unwrap();
    let synced = report.sync.expect("auto-sync is on").unwrap();
    assert_eq!(synced.added, 1);
    assert!(w
        .root()
        .join("tk2")
        .join(SESSIONS)
        .join("bbb/b0b/local_9.json")
        .exists());
    w.d.set_autosync("tk2", false).unwrap();
    assert_eq!(w.d.autosync("tk2").unwrap(), AutoSync::default());
}

#[test]
fn auto_sync_needs_an_account_on_both_sides() {
    let w = world();
    w.d.setup("tk1").unwrap();
    w.d.add("tk2").unwrap();
    assert_eq!(code(w.d.set_autosync("tk2", true)), "desktopNotAssigned");
    w.sessions("tk2", "bbb/b0b");
    w.d.assign("tk2", "bbb/b0b").unwrap();
    assert_eq!(code(w.d.set_autosync("tk2", true)), "desktopNoSources");
}

#[test]
fn the_overview_says_what_each_profile_has() {
    let w = world();
    assert_eq!(w.d.overview().state, "notSetUp");
    w.two_profiles();
    w.d.set_autosync("tk2", true).unwrap();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_1.json",
        json!({"cliSessionId": "s1"}),
    );
    let o = w.d.overview();
    assert_eq!(o.state, "ready");
    assert_eq!(o.current.as_deref(), Some("tk2"));
    let names: Vec<_> = o.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["tk1", "tk2"]);
    assert_eq!(o.profiles[0].account.as_deref(), Some("aaa/a0a"));
    assert_eq!(
        o.profiles[0].pairs,
        [PairView {
            pair: "aaa/a0a".into(),
            cards: 1
        }]
    );
    assert!(o.profiles[1].current);
    assert_eq!(o.profiles[1].autosync.sources, ["tk1"]);
}

#[test]
fn a_display_name_is_kept_beside_the_account_and_cleared_to_the_backups() {
    let w = world();
    w.d.setup("tk1").unwrap();
    assert_eq!(w.d.label("tk1"), None);
    w.d.set_label("tk1", Some("  Công ty  ")).unwrap();
    assert_eq!(w.d.label("tk1").as_deref(), Some("Công ty"));
    assert_eq!(w.d.overview().profiles[0].label.as_deref(), Some("Công ty"));
    assert_eq!(
        code(w.d.set_label("tk1", Some("two\nlines"))),
        "desktopInvalidLabel"
    );
    w.d.set_label("tk1", None).unwrap();
    assert_eq!(w.d.label("tk1"), None);
    assert!(!w.backups_named("tk1.label-reset-").is_empty());
}

#[test]
fn no_claude_desktop_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let d = Desktop::new(Paths::new(dir.path(), true), FakeApp::default()).unwrap();
    assert_eq!(d.overview().state, "missing");
}

#[test]
fn the_real_app_folder_is_never_replaced_by_a_link() {
    let w = world();
    fs::create_dir_all(w.root().join("tk9")).unwrap();
    assert_eq!(code(w.d.point_to("tk9")), "desktopUnsafe");
    assert!(w.d.paths().app_data.join("Cookies").exists());
}
